//! Shared on-disk cache. The caller owns the location (the app passes
//! `~/.etchable/cache/lcsc/v1` via `store::paths`; this crate stays a pure
//! sourcing library with no path policy). Namespaces carry their own TTL
//! policy:
//! uuid-addressed `docs/` and `models/` are immutable, `numbers/` lasts a
//! week, `jlc/` a day, `search/` fifteen minutes (stock freshness is the
//! whole point of search). Writes are atomic (tmp + rename); `sweep` evicts
//! least-recently-used files past a byte budget.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

/// TTL per namespace; `None` = immutable.
pub fn ttl_for(namespace: &str) -> Option<Duration> {
    match namespace {
        "docs" | "models" => None,
        "numbers" => Some(Duration::from_secs(7 * 24 * 3600)),
        "jlc" => Some(Duration::from_secs(24 * 3600)),
        "search" => Some(Duration::from_secs(15 * 60)),
        _ => Some(Duration::from_secs(24 * 3600)),
    }
}

pub struct Cache {
    root: PathBuf,
    /// Injectable clock so TTL tests don't sleep.
    now: fn() -> SystemTime,
}

/// A cache hit: the bytes plus when they were stored (surfaced as `as_of`
/// in tool payloads).
pub struct Hit {
    pub bytes: Vec<u8>,
    pub stored_at: SystemTime,
}

impl Cache {
    pub fn open(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)
            .with_context(|| format!("cannot create cache dir {}", root.display()))?;
        Ok(Self {
            root: root.to_path_buf(),
            now: SystemTime::now,
        })
    }

    #[cfg(test)]
    pub fn with_clock(root: &Path, now: fn() -> SystemTime) -> Result<Self> {
        let mut cache = Self::open(root)?;
        cache.now = now;
        Ok(cache)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path_for(&self, namespace: &str, key: &str) -> PathBuf {
        // Keys are C-numbers, uuids, or query hashes — already path-safe;
        // sanitize defensively anyway.
        let safe: String = key
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' })
            .collect();
        self.root.join(namespace).join(safe)
    }

    /// Fresh-only get: expired entries read as misses (but are left on disk
    /// for `sweep` to reap, so a fetch failure can still fall back to them
    /// via [`Cache::get_stale`]).
    pub fn get(&self, namespace: &str, key: &str) -> Option<Hit> {
        let hit = self.get_stale(namespace, key)?;
        if let Some(ttl) = ttl_for(namespace) {
            let age = (self.now)().duration_since(hit.stored_at).unwrap_or_default();
            if age > ttl {
                return None;
            }
        }
        Some(hit)
    }

    /// Get ignoring TTL — the offline/failure fallback.
    pub fn get_stale(&self, namespace: &str, key: &str) -> Option<Hit> {
        let path = self.path_for(namespace, key);
        let meta = std::fs::metadata(&path).ok()?;
        let stored_at = meta.modified().ok()?;
        let bytes = std::fs::read(&path).ok()?;
        Some(Hit { bytes, stored_at })
    }

    /// Atomic write: tmp file in the same directory, then rename.
    pub fn put(&self, namespace: &str, key: &str, bytes: &[u8]) -> Result<()> {
        let path = self.path_for(namespace, key);
        let dir = path.parent().expect("namespaced path has a parent");
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("cache rename failed: {}", path.display()))?;
        Ok(())
    }

    /// Evict least-recently-modified files until total size fits the budget.
    pub fn sweep(&self, max_bytes: u64) -> Result<u64> {
        let mut files: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
        let mut total = 0u64;
        for ns in std::fs::read_dir(&self.root)? {
            let ns = ns?.path();
            if !ns.is_dir() {
                continue;
            }
            for f in std::fs::read_dir(&ns)? {
                let f = f?.path();
                let Ok(meta) = std::fs::metadata(&f) else { continue };
                if !meta.is_file() {
                    continue;
                }
                total += meta.len();
                files.push((f, meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
            }
        }
        if total <= max_bytes {
            return Ok(0);
        }
        files.sort_by_key(|(_, _, mtime)| *mtime);
        let mut evicted = 0u64;
        for (path, size, _) in files {
            if total - evicted <= max_bytes {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                evicted += size;
            }
        }
        Ok(evicted)
    }
}

pub const DEFAULT_SWEEP_BUDGET: u64 = 512 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    fn far_future() -> SystemTime {
        SystemTime::now() + Duration::from_secs(365 * 24 * 3600)
    }

    #[test]
    fn put_get_roundtrip_and_atomic_layout() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        cache.put("docs", "abc123", b"payload").unwrap();
        let hit = cache.get("docs", "abc123").unwrap();
        assert_eq!(hit.bytes, b"payload");
        // No tmp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("docs"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn ttl_expiry_reads_as_miss_but_stale_get_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::with_clock(dir.path(), far_future).unwrap();
        cache.put("search", "rp2040", b"old").unwrap();
        // A year from "now": search TTL (15 min) long expired.
        assert!(cache.get("search", "rp2040").is_none());
        assert_eq!(cache.get_stale("search", "rp2040").unwrap().bytes, b"old");
        // Immutable namespaces never expire.
        cache.put("docs", "uuid1", b"doc").unwrap();
        assert!(cache.get("docs", "uuid1").is_some());
    }

    #[test]
    fn sweep_evicts_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::open(dir.path()).unwrap();
        cache.put("models", "old", &[0u8; 1000]).unwrap();
        // Ensure distinct mtimes even on coarse filesystems.
        let old_path = dir.path().join("models/old");
        let past = std::time::SystemTime::now() - Duration::from_secs(3600);
        let f = std::fs::File::options().append(true).open(&old_path).unwrap();
        f.set_modified(past).unwrap();
        cache.put("models", "new", &[0u8; 1000]).unwrap();

        let evicted = cache.sweep(1500).unwrap();
        assert_eq!(evicted, 1000);
        assert!(!old_path.exists());
        assert!(dir.path().join("models/new").exists());
    }
}
