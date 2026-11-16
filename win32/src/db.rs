//! SQLite storage. Same schema and migrations as the Electron app, so a
//! focus.db from either opens in both. Append migrations; never edit one.
// Some accessors (settings, projects, entries) are used by later phases only.
#![allow(dead_code)]

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

/// Ordered schema steps; `PRAGMA user_version` is the number applied.
const MIGRATIONS: [&str; 3] = [
    "CREATE TABLE projects (
       id TEXT PRIMARY KEY,
       name TEXT NOT NULL,
       client TEXT NOT NULL DEFAULT '',
       color TEXT NOT NULL,
       rate REAL NOT NULL DEFAULT 0,
       currency TEXT NOT NULL,
       billable_default INTEGER NOT NULL DEFAULT 1,
       archived INTEGER NOT NULL DEFAULT 0,
       created_at INTEGER NOT NULL
     );
     CREATE TABLE entries (
       id TEXT PRIMARY KEY,
       project_id TEXT REFERENCES projects(id) ON DELETE SET NULL,
       description TEXT NOT NULL DEFAULT '',
       start INTEGER NOT NULL,
       end INTEGER,
       billable INTEGER NOT NULL DEFAULT 1,
       source TEXT NOT NULL DEFAULT 'manual'
     );
     CREATE INDEX entries_start ON entries(start);
     CREATE TABLE activity (
       id INTEGER PRIMARY KEY AUTOINCREMENT,
       kind TEXT NOT NULL,
       app TEXT NOT NULL,
       bundle_id TEXT NOT NULL DEFAULT '',
       detail TEXT NOT NULL DEFAULT '',
       context TEXT NOT NULL DEFAULT '',
       url TEXT NOT NULL DEFAULT '',
       start INTEGER NOT NULL,
       end INTEGER NOT NULL
     );
     CREATE INDEX activity_range ON activity(start, end);
     CREATE TABLE claude_files (
       path TEXT PRIMARY KEY,
       offset INTEGER NOT NULL,
       mtime INTEGER NOT NULL
     );
     CREATE TABLE claude_sessions (
       session_id TEXT PRIMARY KEY,
       project TEXT NOT NULL,
       title TEXT NOT NULL DEFAULT '',
       last_ts INTEGER NOT NULL,
       activity_id INTEGER
     );
     CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    // Sessions started by other programs through the Agent SDK are not your
    // work: remember which are automated and re-read the logs.
    "ALTER TABLE claude_sessions ADD COLUMN automated INTEGER NOT NULL DEFAULT 0;
     DELETE FROM activity WHERE kind = 'claude';
     DELETE FROM claude_files;
     DELETE FROM claude_sessions;",
    // An install that already has data has seen the welcome screen.
    "INSERT OR IGNORE INTO settings (key, value)
       SELECT 'onboarded', 'true'
       WHERE EXISTS (SELECT 1 FROM projects) OR EXISTS (SELECT 1 FROM entries) OR EXISTS (SELECT 1 FROM activity);",
];

/// Key in `settings` holding the longest activity run in ms; it lets range
/// queries cut the scan on the start index from both sides.
const MAX_DURATION_KEY: &str = "maxActivityMs";

#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    pub id: i64,
    pub kind: String,
    pub app: String,
    pub bundle_id: String,
    pub detail: String,
    pub context: String,
    pub url: String,
    pub start: i64,
    pub end: i64,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Db> {
        Self::init(Connection::open(path)?, true)
    }

    pub fn open_in_memory() -> rusqlite::Result<Db> {
        Self::init(Connection::open_in_memory()?, false)
    }

    fn init(conn: Connection, wal: bool) -> rusqlite::Result<Db> {
        if wal {
            conn.pragma_update(None, "journal_mode", "WAL")?;
        }
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_millis(2000))?;
        let mut db = Db { conn };
        db.migrate()?;
        Ok(db)
    }

    /// Runs each pending migration in its own transaction.
    fn migrate(&mut self) -> rusqlite::Result<()> {
        let version = self.user_version()?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn user_version(&self) -> rusqlite::Result<usize> {
        let v: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        Ok(v as usize)
    }

    /// Called before closing the connection.
    pub fn optimize(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch("PRAGMA optimize")
    }

    // ── Activity ─────────────────────────────────────────────────────────

    pub fn insert_activity(&self, a: &Activity) -> rusqlite::Result<i64> {
        if a.end < a.start {
            return Err(rusqlite::Error::InvalidParameterName(
                "end before start".into(),
            ));
        }
        self.conn.execute(
            "INSERT INTO activity (kind, app, bundle_id, detail, context, url, start, end)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                a.kind,
                a.app,
                a.bundle_id,
                a.detail,
                a.context,
                a.url,
                a.start,
                a.end
            ],
        )?;
        self.bump_max_duration(a.end - a.start)?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_activity_end(&self, id: i64, end: i64) -> rusqlite::Result<()> {
        let start: i64 =
            self.conn
                .query_row("SELECT start FROM activity WHERE id = ?", [id], |r| {
                    r.get(0)
                })?;
        self.conn
            .execute("UPDATE activity SET end = ? WHERE id = ?", params![end, id])?;
        self.bump_max_duration(end - start)
    }

    /// Activity overlapping [from, to). The extra `start >= from - max` bound
    /// lets the start index cut from both sides on large databases.
    pub fn list_activity(&self, from: i64, to: i64) -> rusqlite::Result<Vec<Activity>> {
        let lower = from.saturating_sub(self.max_duration()?);
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, app, bundle_id, detail, context, url, start, end FROM activity
             WHERE start >= ?1 AND start < ?2 AND end > ?3 AND app != 'Focus' ORDER BY start",
        )?;
        let rows = stmt.query_map(params![lower, to, from], |r| {
            Ok(Activity {
                id: r.get(0)?,
                kind: r.get(1)?,
                app: r.get(2)?,
                bundle_id: r.get(3)?,
                detail: r.get(4)?,
                context: r.get(5)?,
                url: r.get(6)?,
                start: r.get(7)?,
                end: r.get(8)?,
            })
        })?;
        rows.collect()
    }

    fn max_duration(&self) -> rusqlite::Result<i64> {
        Ok(self
            .get_setting(MAX_DURATION_KEY)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    fn bump_max_duration(&self, duration: i64) -> rusqlite::Result<()> {
        if duration > self.max_duration()? {
            self.set_setting(MAX_DURATION_KEY, &duration.to_string())?;
        }
        Ok(())
    }

    // ── Settings (values are JSON text, as in the Electron app) ──────────

    pub fn get_setting(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?", [key], |r| {
                r.get(0)
            })
            .optional()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(app: &str, start: i64, end: i64) -> Activity {
        Activity {
            id: 0,
            kind: "app".into(),
            app: app.into(),
            bundle_id: String::new(),
            detail: String::new(),
            context: String::new(),
            url: String::new(),
            start,
            end,
        }
    }

    #[test]
    fn applies_all_migrations() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.user_version().unwrap(), MIGRATIONS.len());
    }

    #[test]
    fn existing_data_marks_onboarded() {
        let mut db = Db::open_in_memory().unwrap();
        // Rewind to before migration 3 with data present, then migrate again.
        db.conn.execute("DELETE FROM settings", []).unwrap();
        db.insert_activity(&act("Code", 0, 1000)).unwrap();
        db.conn
            .execute("DELETE FROM settings WHERE key = ?", [MAX_DURATION_KEY])
            .unwrap();
        db.conn.pragma_update(None, "user_version", 2).unwrap();
        db.migrate().unwrap();
        assert_eq!(
            db.get_setting("onboarded").unwrap().as_deref(),
            Some("true")
        );
    }

    #[test]
    fn range_query_matches_naive_overlap() {
        let db = Db::open_in_memory().unwrap();
        db.insert_activity(&act("A", 0, 1_000)).unwrap();
        db.insert_activity(&act("B", 5_000, 9_000)).unwrap();
        db.insert_activity(&act("C", 20_000, 21_000)).unwrap();
        // B starts before the range and ends inside it: must be included.
        let got: Vec<_> = db
            .list_activity(6_000, 20_000)
            .unwrap()
            .into_iter()
            .map(|a| a.app)
            .collect();
        assert_eq!(got, ["B"]);
        let got: Vec<_> = db
            .list_activity(0, 100_000)
            .unwrap()
            .into_iter()
            .map(|a| a.app)
            .collect();
        assert_eq!(got, ["A", "B", "C"]);
    }

    #[test]
    fn hides_own_app_and_tracks_extended_runs() {
        let db = Db::open_in_memory().unwrap();
        let id = db.insert_activity(&act("Code", 100, 200)).unwrap();
        db.insert_activity(&act("Focus", 100, 200)).unwrap();
        db.set_activity_end(id, 50_000).unwrap();
        // Long run starting before the window must still be found.
        let got = db.list_activity(40_000, 60_000).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].app, "Code");
    }

    #[test]
    fn rejects_negative_duration() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.insert_activity(&act("X", 10, 5)).is_err());
    }

    #[test]
    fn settings_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.get_setting("idleMinutes").unwrap(), None);
        db.set_setting("idleMinutes", "5").unwrap();
        db.set_setting("idleMinutes", "7").unwrap();
        assert_eq!(db.get_setting("idleMinutes").unwrap().as_deref(), Some("7"));
    }
}
