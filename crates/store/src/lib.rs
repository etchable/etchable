//! sea-orm/sqlite app state under `~/.etchable/state/` (docs/decisions/0005):
//! recent projects, agent sessions, and frontend-owned prefs.
//!
//! Single-connection pool on purpose: a desktop app needs no concurrency,
//! and one connection keeps pragma semantics trivial (sqlx's sqlite
//! defaults already give WAL + a 5 s busy timeout; WAL persists in the db
//! file). All methods are async and safe to call from any tauri task.
//!
//! Failure policy: a `Store` that fails to open — including a db touched by
//! a NEWER build, which the migrator refuses (its `seaql_migrations` rows
//! wouldn't match this build's embedded set) — is reported once and the app
//! runs without persistence: never bricked, never auto-wiped.

pub mod entities;
mod migrations;
pub mod paths;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result};
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectOptions, ConnectionTrait, Database,
    DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use sea_orm_migration::MigratorTrait;
use serde::Serialize;
use ts_rs::TS;

use entities::{agent_session, pref, project};

const MAX_PROJECTS: u64 = 50;
const MAX_SESSIONS_PER_WORKSPACE: u64 = 100;
const TITLE_MAX: usize = 80;

/// A recently opened project (the welcome screen's list).
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecentProject {
    pub root: String,
    pub name: String,
    pub board: Option<String>,
    /// Unix milliseconds (Date.now()-compatible).
    #[ts(type = "number")]
    pub last_opened_at: i64,
}

/// A resumable agent session for one workspace.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionSummary {
    pub session_id: String,
    pub workspace_root: String,
    /// First user message, truncated; set once.
    pub title: Option<String>,
    pub model: Option<String>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub last_used_at: i64,
}

/// Everything known about a session at record time. `claude --resume` forks
/// a NEW session id — `resumed_from` links the chain so superseded rows can
/// be hidden from listings.
#[derive(Debug, Clone, Default)]
pub struct NewSession {
    pub session_id: String,
    pub workspace_root: String,
    pub model: Option<String>,
    pub title: Option<String>,
    pub resumed_from: Option<String>,
}

#[derive(Clone)]
pub struct Store {
    conn: DatabaseConnection,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Store {
    /// Open (creating dirs and the db as needed) and migrate to the latest
    /// schema. The default location is `~/.etchable/state/etchable.sqlite3`.
    pub async fn open_default() -> Result<Self> {
        Self::open(&paths::state_dir().join("etchable.sqlite3")).await
    }

    pub async fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let mut options = ConnectOptions::new(url);
        options.max_connections(1).sqlx_logging(false);
        let conn = Database::connect(options)
            .await
            .with_context(|| format!("opening {}", db_path.display()))?;
        // sqlx's sqlite defaults already use WAL; make it explicit so the
        // file's journal mode never depends on driver defaults.
        conn.execute_unprepared("PRAGMA journal_mode=WAL;").await?;
        migrations::Migrator::up(&conn, None)
            .await
            .context("state db migration failed (db from a newer build?)")?;
        Ok(Self { conn })
    }

    // --- projects ----------------------------------------------------------

    pub async fn record_project_opened(
        &self,
        root: &str,
        name: &str,
        board: Option<&str>,
    ) -> Result<()> {
        self.record_project_opened_at(root, name, board, now_ms())
            .await
    }

    async fn record_project_opened_at(
        &self,
        root: &str,
        name: &str,
        board: Option<&str>,
        at: i64,
    ) -> Result<()> {
        let row = project::ActiveModel {
            root: Set(root.to_string()),
            name: Set(name.to_string()),
            board: Set(board.map(String::from)),
            last_opened_at: Set(at),
        };
        project::Entity::insert(row)
            .on_conflict(
                OnConflict::column(project::Column::Root)
                    .update_columns([
                        project::Column::Name,
                        project::Column::Board,
                        project::Column::LastOpenedAt,
                    ])
                    .to_owned(),
            )
            .exec(&self.conn)
            .await?;

        // Bound growth: keep the newest MAX_PROJECTS.
        let keep: Vec<String> = project::Entity::find()
            .order_by_desc(project::Column::LastOpenedAt)
            .limit(MAX_PROJECTS)
            .all(&self.conn)
            .await?
            .into_iter()
            .map(|p| p.root)
            .collect();
        project::Entity::delete_many()
            .filter(project::Column::Root.is_not_in(keep))
            .exec(&self.conn)
            .await?;
        Ok(())
    }

    pub async fn recent_projects(&self, limit: u64) -> Result<Vec<RecentProject>> {
        Ok(project::Entity::find()
            .order_by_desc(project::Column::LastOpenedAt)
            .limit(limit)
            .all(&self.conn)
            .await?
            .into_iter()
            .map(|p| RecentProject {
                root: p.root,
                name: p.name,
                board: p.board,
                last_opened_at: p.last_opened_at,
            })
            .collect())
    }

    pub async fn remove_recent_project(&self, root: &str) -> Result<()> {
        project::Entity::delete_by_id(root).exec(&self.conn).await?;
        Ok(())
    }

    // --- agent sessions ----------------------------------------------------

    pub async fn record_session_started(&self, session: &NewSession) -> Result<()> {
        self.record_session_started_at(session, now_ms()).await
    }

    async fn record_session_started_at(&self, s: &NewSession, at: i64) -> Result<()> {
        let title = s
            .title
            .as_deref()
            .map(|t| t.chars().take(TITLE_MAX).collect::<String>());

        // Read-modify-write instead of COALESCE-in-upsert: single-connection
        // store, and the set-once semantics read clearer as code.
        match agent_session::Entity::find_by_id(&s.session_id)
            .one(&self.conn)
            .await?
        {
            Some(existing) => {
                let mut row: agent_session::ActiveModel = existing.clone().into();
                if existing.title.is_none() {
                    row.title = Set(title);
                }
                if existing.model.is_none() {
                    row.model = Set(s.model.clone());
                }
                if existing.resumed_from.is_none() {
                    row.resumed_from = Set(s.resumed_from.clone());
                }
                row.last_used_at = Set(at);
                row.update(&self.conn).await?;
            }
            None => {
                agent_session::ActiveModel {
                    session_id: Set(s.session_id.clone()),
                    workspace_root: Set(s.workspace_root.clone()),
                    title: Set(title),
                    model: Set(s.model.clone()),
                    resumed_from: Set(s.resumed_from.clone()),
                    created_at: Set(at),
                    last_used_at: Set(at),
                }
                .insert(&self.conn)
                .await?;
            }
        }

        // Bound growth per workspace.
        let keep: Vec<String> = agent_session::Entity::find()
            .filter(agent_session::Column::WorkspaceRoot.eq(&s.workspace_root))
            .order_by_desc(agent_session::Column::LastUsedAt)
            .limit(MAX_SESSIONS_PER_WORKSPACE)
            .all(&self.conn)
            .await?
            .into_iter()
            .map(|r| r.session_id)
            .collect();
        agent_session::Entity::delete_many()
            .filter(agent_session::Column::WorkspaceRoot.eq(&s.workspace_root))
            .filter(agent_session::Column::SessionId.is_not_in(keep))
            .exec(&self.conn)
            .await?;
        Ok(())
    }

    pub async fn touch_session(&self, session_id: &str) -> Result<()> {
        self.touch_session_at(session_id, now_ms()).await
    }

    async fn touch_session_at(&self, session_id: &str, at: i64) -> Result<()> {
        if let Some(existing) = agent_session::Entity::find_by_id(session_id)
            .one(&self.conn)
            .await?
        {
            let mut row: agent_session::ActiveModel = existing.into();
            row.last_used_at = Set(at);
            row.update(&self.conn).await?;
        }
        Ok(())
    }

    /// Sessions for a workspace, newest first, hiding rows superseded by a
    /// resume (their forked successor carries the history forward).
    pub async fn sessions_for(
        &self,
        workspace_root: &str,
        limit: usize,
    ) -> Result<Vec<SessionSummary>> {
        let rows = agent_session::Entity::find()
            .filter(agent_session::Column::WorkspaceRoot.eq(workspace_root))
            .order_by_desc(agent_session::Column::LastUsedAt)
            .all(&self.conn)
            .await?;
        let superseded: BTreeSet<&str> = rows
            .iter()
            .filter_map(|r| r.resumed_from.as_deref())
            .collect();
        Ok(rows
            .iter()
            .filter(|r| !superseded.contains(r.session_id.as_str()))
            .take(limit)
            .map(|r| SessionSummary {
                session_id: r.session_id.clone(),
                workspace_root: r.workspace_root.clone(),
                title: r.title.clone(),
                model: r.model.clone(),
                created_at: r.created_at,
                last_used_at: r.last_used_at,
            })
            .collect())
    }

    // --- prefs -------------------------------------------------------------

    /// The whole prefs table — it is tiny and one invoke hydrates the UI.
    pub async fn get_prefs(&self) -> Result<BTreeMap<String, serde_json::Value>> {
        Ok(pref::Entity::find()
            .all(&self.conn)
            .await?
            .into_iter()
            .map(|row| {
                let value = serde_json::from_str(&row.value)
                    .unwrap_or_else(|_| serde_json::Value::String(row.value));
                (row.key, value)
            })
            .collect())
    }

    pub async fn set_pref(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        pref::Entity::insert(pref::ActiveModel {
            key: Set(key.to_string()),
            value: Set(value.to_string()),
        })
        .on_conflict(
            OnConflict::column(pref::Column::Key)
                .update_column(pref::Column::Value)
                .to_owned(),
        )
        .exec(&self.conn)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fresh() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("state/etchable.sqlite3"))
            .await
            .unwrap();
        (dir, store)
    }

    #[tokio::test]
    async fn fresh_open_migrates_and_reopen_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state/etchable.sqlite3");
        let store = Store::open(&db).await.unwrap();
        store.record_project_opened("/p", "p", None).await.unwrap();
        drop(store);
        // Reopen: migrations are a no-op, data survives.
        let store = Store::open(&db).await.unwrap();
        assert_eq!(store.recent_projects(10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_db_from_a_newer_build_is_refused_not_wiped() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("etchable.sqlite3");
        {
            let store = Store::open(&db).await.unwrap();
            store.record_project_opened("/p", "p", None).await.unwrap();
            // Simulate a newer build: an applied migration this build
            // doesn't know about.
            store
                .conn
                .execute_unprepared(
                    "INSERT INTO seaql_migrations (version, applied_at)
                     VALUES ('m9999_from_the_future', 0)",
                )
                .await
                .unwrap();
        }
        assert!(Store::open(&db).await.is_err());
        // The data is still there for the newer build that owns it.
        let url = format!("sqlite://{}?mode=rwc", db.display());
        let conn = Database::connect(url).await.unwrap();
        let n = project::Entity::find().all(&conn).await.unwrap().len();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn project_upsert_bumps_and_prunes() {
        let (_dir, store) = fresh().await;
        store
            .record_project_opened_at("/a", "alpha", Some("board.zen"), 100)
            .await
            .unwrap();
        store
            .record_project_opened_at("/a", "alpha-renamed", None, 200)
            .await
            .unwrap();
        let recents = store.recent_projects(10).await.unwrap();
        assert_eq!(recents.len(), 1);
        assert_eq!(recents[0].name, "alpha-renamed");
        assert_eq!(recents[0].board, None);
        assert_eq!(recents[0].last_opened_at, 200);

        for i in 0..(MAX_PROJECTS + 5) {
            store
                .record_project_opened_at(&format!("/p{i}"), "p", None, 300 + i as i64)
                .await
                .unwrap();
        }
        let recents = store.recent_projects(1000).await.unwrap();
        assert_eq!(recents.len(), MAX_PROJECTS as usize);
        assert!(recents.iter().all(|r| r.root != "/a"), "oldest pruned");
        assert_eq!(recents[0].root, format!("/p{}", MAX_PROJECTS + 4));

        store.remove_recent_project(&recents[0].root).await.unwrap();
        assert_eq!(
            store.recent_projects(1000).await.unwrap().len(),
            MAX_PROJECTS as usize - 1
        );
    }

    fn session(id: &str, ws: &str, title: Option<&str>, resumed_from: Option<&str>) -> NewSession {
        NewSession {
            session_id: id.into(),
            workspace_root: ws.into(),
            model: Some("opus".into()),
            title: title.map(Into::into),
            resumed_from: resumed_from.map(Into::into),
        }
    }

    #[tokio::test]
    async fn session_lifecycle_title_set_once_and_touch() {
        let (_dir, store) = fresh().await;
        store
            .record_session_started_at(&session("s1", "/ws", Some("make a board"), None), 100)
            .await
            .unwrap();
        // Re-record (a second init event): title must NOT be replaced.
        store
            .record_session_started_at(&session("s1", "/ws", Some("other title"), None), 150)
            .await
            .unwrap();
        store.touch_session_at("s1", 500).await.unwrap();

        let sessions = store.sessions_for("/ws", 10).await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title.as_deref(), Some("make a board"));
        assert_eq!(sessions[0].created_at, 100);
        assert_eq!(sessions[0].last_used_at, 500);
    }

    #[tokio::test]
    async fn resumed_sessions_hide_their_predecessor() {
        let (_dir, store) = fresh().await;
        store
            .record_session_started_at(&session("old", "/ws", Some("t"), None), 100)
            .await
            .unwrap();
        store
            .record_session_started_at(&session("new", "/ws", Some("t"), Some("old")), 200)
            .await
            .unwrap();
        let sessions = store.sessions_for("/ws", 10).await.unwrap();
        assert_eq!(sessions.len(), 1, "superseded row must be hidden");
        assert_eq!(sessions[0].session_id, "new");
        assert!(store.sessions_for("/other", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sessions_prune_per_workspace() {
        let (_dir, store) = fresh().await;
        for i in 0..(MAX_SESSIONS_PER_WORKSPACE + 3) {
            store
                .record_session_started_at(&session(&format!("s{i}"), "/ws", None, None), i as i64)
                .await
                .unwrap();
        }
        store
            .record_session_started_at(&session("elsewhere", "/other", None, None), 5)
            .await
            .unwrap();
        let ws_rows = agent_session::Entity::find()
            .filter(agent_session::Column::WorkspaceRoot.eq("/ws"))
            .all(&store.conn)
            .await
            .unwrap();
        assert_eq!(ws_rows.len(), MAX_SESSIONS_PER_WORKSPACE as usize);
        assert_eq!(store.sessions_for("/other", 10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn prefs_roundtrip_and_overwrite() {
        let (_dir, store) = fresh().await;
        store
            .set_pref("ui.panelWidth", &serde_json::json!(420.5))
            .await
            .unwrap();
        store
            .set_pref("ui.activeTab", &serde_json::json!("problems"))
            .await
            .unwrap();
        store
            .set_pref("ui.activeTab", &serde_json::json!("chat"))
            .await
            .unwrap();
        let prefs = store.get_prefs().await.unwrap();
        assert_eq!(prefs["ui.panelWidth"], serde_json::json!(420.5));
        assert_eq!(prefs["ui.activeTab"], serde_json::json!("chat"));
        assert_eq!(prefs.len(), 2);
    }
}
