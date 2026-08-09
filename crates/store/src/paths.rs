//! The `~/.etchable/` layout — the single authority for where the app puts
//! things outside a project:
//!
//! ```text
//! ~/.etchable/            ETCHABLE_HOME override
//!   cache/                disposable, safe to rm -rf; ETCHABLE_CACHE_DIR override
//!   state/                durable app state (etchable.sqlite3)
//!   runtime/              per-instance scratch (mcp-config-{pid}.json)
//! ```
//!
//! Deliberate XDG deviation: one predictable, deletable, documentable dir
//! in the `.cargo` / `.claude` tradition, identical on every platform.

use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    resolve_home(
        std::env::var_os("ETCHABLE_HOME").map(PathBuf::from),
        dirs::home_dir(),
    )
}

fn resolve_home(env_override: Option<PathBuf>, os_home: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = env_override {
        return dir;
    }
    os_home
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".etchable")
}

pub fn etchable_home() -> PathBuf {
    home()
}

/// Disposable cache root. `ETCHABLE_CACHE_DIR` keeps its historical meaning:
/// the directory that CONTAINS `lcsc/v1` (CI and tests point it at tmp).
pub fn cache_dir() -> PathBuf {
    resolve_cache(
        std::env::var_os("ETCHABLE_CACHE_DIR").map(PathBuf::from),
        home(),
    )
}

fn resolve_cache(env_override: Option<PathBuf>, home: PathBuf) -> PathBuf {
    env_override.unwrap_or_else(|| home.join("cache"))
}

/// Durable state (the sqlite db).
pub fn state_dir() -> PathBuf {
    home().join("state")
}

/// Per-instance scratch; files here are pid-suffixed and swept on startup.
pub fn runtime_dir() -> PathBuf {
    home().join("runtime")
}

/// One-time, best-effort move of the pre-0005 LCSC cache
/// (`~/Library/Caches/etchable/lcsc` and friends) into the new layout.
/// Every failure is swallowed — a cache is disposable, and a cross-volume
/// rename failure just means a cold start.
pub fn migrate_legacy_lcsc_cache() {
    let Some(old) = dirs::cache_dir().map(|d| d.join("etchable/lcsc")) else {
        return;
    };
    migrate_legacy_dir(&old, &cache_dir().join("lcsc"));
}

fn migrate_legacy_dir(old: &Path, new: &Path) {
    if !old.is_dir() || new.exists() {
        return;
    }
    if let Some(parent) = new.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::rename(old, new) {
        Ok(()) => tracing::info!("moved legacy cache {} -> {}", old.display(), new.display()),
        Err(e) => tracing::debug!("legacy cache not moved ({e}); starting cold"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_resolution_prefers_the_env_override() {
        assert_eq!(
            resolve_home(Some("/custom".into()), Some("/Users/x".into())),
            PathBuf::from("/custom")
        );
        assert_eq!(
            resolve_home(None, Some("/Users/x".into())),
            PathBuf::from("/Users/x/.etchable")
        );
    }

    #[test]
    fn cache_override_keeps_its_historical_meaning() {
        assert_eq!(
            resolve_cache(Some("/tmp/c".into()), "/h/.etchable".into()),
            PathBuf::from("/tmp/c")
        );
        assert_eq!(
            resolve_cache(None, "/h/.etchable".into()),
            PathBuf::from("/h/.etchable/cache")
        );
    }

    #[test]
    fn legacy_migration_moves_only_when_dest_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("old/lcsc");
        let new = tmp.path().join("new/cache/lcsc");
        std::fs::create_dir_all(old.join("v1")).unwrap();
        std::fs::write(old.join("v1/marker"), b"x").unwrap();

        migrate_legacy_dir(&old, &new);
        assert!(new.join("v1/marker").exists());
        assert!(!old.exists());

        // Second call: dest exists, old absent — a no-op, no panic.
        migrate_legacy_dir(&old, &new);
        assert!(new.join("v1/marker").exists());

        // Failure is swallowed: recreate old, point dest at an unwritable
        // location (a path under a FILE cannot be created).
        std::fs::create_dir_all(&old).unwrap();
        let blocked = tmp.path().join("file");
        std::fs::write(&blocked, b"f").unwrap();
        migrate_legacy_dir(&old, &blocked.join("nested/lcsc"));
        assert!(old.exists(), "old cache untouched on failure");
    }
}
