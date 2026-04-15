//! MRU / frecency tracking for launcher sources.
//!
//! Records `(source, item_key)` activations to a SQLite database and exposes a
//! frecency score as a launcher weight bonus. The DB is opened once at startup
//! and fully mirrored into an in-memory `HashMap` so the hot ranking path
//! (`weight()` called per keystroke) never touches SQLite.
//!
//! Frecency = `ln_1p(count) / (1 + days_since_last_used * 0.5)`. Monotonic in
//! count, decays with age. Converted to an integer weight bonus capped at 100.
//!
//! Design: see the project spec. Two key invariants:
//!
//! * Read path is lock-free on SQLite. Every query hits the `RwLock<HashMap>`.
//! * Write path does both — updates the map and persists via an UPSERT.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};

const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS usage (\
  source     TEXT NOT NULL,\
  item_key   TEXT NOT NULL,\
  count      INTEGER NOT NULL DEFAULT 0,\
  last_used  INTEGER NOT NULL,\
  PRIMARY KEY (source, item_key)\
) WITHOUT ROWID;\
CREATE INDEX IF NOT EXISTS usage_source_recent ON usage(source, last_used DESC);\
";

/// Integer weight bonus cap — rooted in launcher weight scale where windows
/// baseline at 100. Matches spec.
const MAX_BONUS: i32 = 100;

#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("xdg: $HOME and $XDG_STATE_HOME both unset")]
    NoStateDir,
}

/// Frecency score. Pure function so tests can exercise the curve without a DB.
pub fn frecency(count: u32, last_used: i64, now: i64) -> f32 {
    let days = ((now - last_used) as f32 / 86400.0).max(0.0);
    (count as f32).ln_1p() / (1.0 + days * 0.5)
}

fn score_to_bonus(score: f32) -> i32 {
    let raw = (score * 50.0).round() as i32;
    raw.clamp(0, MAX_BONUS)
}

/// Shared tracker: read-mostly in-memory map + SQLite persistence.
pub struct UsageTracker {
    inner: RwLock<HashMap<(String, String), (u32, i64)>>,
    db: Mutex<Connection>,
    /// Captured at construction so tests can freeze "now" by constructing
    /// with `open_at`; production always uses wall-clock seconds.
    now: i64,
}

impl UsageTracker {
    /// Open from the XDG state path: `$XDG_STATE_HOME/zofi/usage.db` or
    /// fallback `$HOME/.local/state/zofi/usage.db`. Creates parent dirs.
    pub fn open() -> Result<Self, UsageError> {
        let path = default_db_path()?;
        Self::open_at(&path)
    }

    /// Open at a specific path — primarily for tests.
    pub fn open_at(path: &Path) -> Result<Self, UsageError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    /// In-memory DB, never persists. Used as a fallback when the on-disk path
    /// cannot be initialized — the launcher still works, MRU is disabled for
    /// the session.
    pub fn open_in_memory() -> Result<Self, UsageError> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self, UsageError> {
        // WAL for crash-safe concurrent-ish writes; we only have one writer but
        // readers (if any ever come) get snapshot isolation.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "user_version", 1)?;
        conn.execute_batch(SCHEMA)?;

        // Preload every row so `frecency_bonus` is lock-free on the hot path.
        let mut stmt = conn.prepare("SELECT source, item_key, count, last_used FROM usage")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u32>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        let mut map = HashMap::new();
        for row in rows {
            let (src, key, count, last) = row?;
            map.insert((src, key), (count, last));
        }
        drop(stmt);

        Ok(Self {
            inner: RwLock::new(map),
            db: Mutex::new(conn),
            now: now_secs(),
        })
    }

    /// Integer weight bonus for (source, key). Returns 0 for unknown pairs.
    pub fn frecency_bonus(&self, source: &str, key: &str) -> i32 {
        let map = self.inner.read().unwrap();
        let Some(&(count, last_used)) = map.get(&(source.to_string(), key.to_string())) else {
            return 0;
        };
        score_to_bonus(frecency(count, last_used, self.now))
    }

    /// Record an activation. Bumps the in-memory counter and persists via
    /// UPSERT. Errors are logged but not surfaced — the user's launch must
    /// succeed even if the DB write fails (e.g. read-only disk).
    pub fn record(&self, source: &str, key: &str) {
        let now = now_secs();
        {
            let mut map = self.inner.write().unwrap();
            let entry = map
                .entry((source.to_string(), key.to_string()))
                .or_insert((0, now));
            entry.0 = entry.0.saturating_add(1);
            entry.1 = now;
        }
        let db = self.db.lock().unwrap();
        if let Err(e) = db.execute(
            "INSERT INTO usage (source, item_key, count, last_used) VALUES (?1, ?2, 1, ?3) \
             ON CONFLICT(source, item_key) DO UPDATE SET count = count + 1, last_used = excluded.last_used",
            params![source, key, now],
        ) {
            tracing::warn!("usage record failed: {e}");
        }
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn default_db_path() -> Result<PathBuf, UsageError> {
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
        let p = PathBuf::from(xdg);
        if !p.as_os_str().is_empty() {
            return Ok(p.join("zofi").join("usage.db"));
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home);
        if !p.as_os_str().is_empty() {
            return Ok(p.join(".local/state/zofi/usage.db"));
        }
    }
    Err(UsageError::NoStateDir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // ------------------------------------------------------------------
    // Pure frecency curve.
    // ------------------------------------------------------------------

    #[test]
    fn frecency_zero_when_count_zero() {
        assert_eq!(frecency(0, 0, 0), 0.0);
        // Any age, any now: still zero because ln_1p(0) = 0.
        assert_eq!(frecency(0, 1000, 2000), 0.0);
    }

    #[test]
    fn frecency_decreases_with_age() {
        // Fixed count; older last_used should score lower.
        let now = 10 * 86400; // t=10 days
        let fresh = frecency(5, now, now);
        let old = frecency(5, 0, now);
        assert!(fresh > old, "fresh={fresh}, old={old}");
    }

    #[test]
    fn frecency_increases_with_count() {
        let now = 0;
        let one = frecency(1, 0, now);
        let ten = frecency(10, 0, now);
        assert!(ten > one, "ten={ten}, one={one}");
    }

    // ------------------------------------------------------------------
    // DB-backed record + frecency_bonus behaviour.
    // ------------------------------------------------------------------

    fn fresh_tracker() -> (tempfile::TempDir, UsageTracker) {
        let dir = tempdir().unwrap();
        let t = UsageTracker::open_at(&dir.path().join("usage.db")).unwrap();
        (dir, t)
    }

    #[test]
    fn record_inserts_new_row() {
        let (_dir, t) = fresh_tracker();
        t.record("apps", "firefox");
        // The map should now carry the new key with count 1.
        let bonus = t.frecency_bonus("apps", "firefox");
        assert!(bonus > 0, "bonus={bonus}");
    }

    #[test]
    fn record_increments_existing_count() {
        let (_dir, t) = fresh_tracker();
        t.record("apps", "firefox");
        let b1 = t.frecency_bonus("apps", "firefox");
        for _ in 0..5 {
            t.record("apps", "firefox");
        }
        let b2 = t.frecency_bonus("apps", "firefox");
        assert!(b2 > b1, "b1={b1}, b2={b2}");
    }

    #[test]
    fn record_persists_across_reopens() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("usage.db");
        {
            let t = UsageTracker::open_at(&path).unwrap();
            for _ in 0..3 {
                t.record("apps", "kitty");
            }
        }
        let t2 = UsageTracker::open_at(&path).unwrap();
        let b = t2.frecency_bonus("apps", "kitty");
        assert!(b > 0, "reopened bonus={b}");
    }

    #[test]
    fn frecency_bonus_returns_zero_for_unknown() {
        let (_dir, t) = fresh_tracker();
        assert_eq!(t.frecency_bonus("apps", "never-seen"), 0);
        assert_eq!(t.frecency_bonus("windows", "firefox"), 0);
    }

    #[test]
    fn frecency_bonus_capped_at_100() {
        let (_dir, t) = fresh_tracker();
        // Even after absurd usage, the bonus must not exceed MAX_BONUS.
        for _ in 0..10_000 {
            t.record("apps", "hot");
        }
        assert_eq!(t.frecency_bonus("apps", "hot"), MAX_BONUS);
    }

    #[test]
    fn open_creates_missing_directory() {
        let dir = tempdir().unwrap();
        let deep = dir.path().join("a/b/c/usage.db");
        assert!(!deep.parent().unwrap().exists());
        let t = UsageTracker::open_at(&deep).unwrap();
        t.record("apps", "x");
        assert!(deep.parent().unwrap().exists());
    }

    #[test]
    fn unique_per_source() {
        // Same key in different sources must not share state.
        let (_dir, t) = fresh_tracker();
        t.record("apps", "code");
        let apps_b = t.frecency_bonus("apps", "code");
        let windows_b = t.frecency_bonus("windows", "code");
        assert!(apps_b > 0);
        assert_eq!(windows_b, 0);
    }

    #[test]
    fn in_memory_open_works() {
        // For the fallback path in main.rs when state dir is unreachable.
        let t = UsageTracker::open_in_memory().unwrap();
        t.record("apps", "x");
        assert!(t.frecency_bonus("apps", "x") > 0);
    }
}
