//! SQLite storage. Same schema and migrations as the Electron app, so a
//! focus.db from either opens in both. Append migrations; never edit one.
// Some accessors (settings, projects, entries) are used by later phases only.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use windows::Win32::System::Com::CoCreateGuid;

use crate::locale::SystemLocale;

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

/// Progress of one Claude Code session log, as in the `claude_sessions` table.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClaudeSession {
    /// Started by a program through the Agent SDK, not typed by the user.
    pub automated: bool,
    pub project: String,
    pub title: String,
    pub last_ts: i64,
    pub activity_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub client: String,
    pub color: String,
    pub rate: f64,
    pub currency: String,
    pub billable_default: bool,
    pub archived: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectInput {
    pub name: String,
    pub client: String,
    pub color: String,
    pub rate: f64,
    pub currency: String,
    pub billable_default: bool,
    pub archived: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectPatch {
    pub name: Option<String>,
    pub client: Option<String>,
    pub color: Option<String>,
    pub rate: Option<f64>,
    pub currency: Option<String>,
    pub billable_default: Option<bool>,
    pub archived: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProjectSummary {
    pub month_ms: i64,
    pub total_ms: i64,
    pub earned: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProjectAllTimeStats {
    pub first_start: Option<i64>,
    pub total_ms: i64,
    pub earned: f64,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimeEntry {
    pub id: String,
    pub project_id: Option<String>,
    pub description: String,
    pub start: i64,
    /// `None` while the timer is running.
    pub end: Option<i64>,
    pub billable: bool,
    /// `"manual" | "calendar" | "timer" | "activity"`
    pub source: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntryInput {
    pub project_id: Option<String>,
    pub description: String,
    pub start: i64,
    pub end: Option<i64>,
    pub billable: bool,
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntryPatch {
    pub project_id: Option<Option<String>>,
    pub description: Option<String>,
    pub start: Option<i64>,
    pub end: Option<Option<i64>>,
    pub billable: Option<bool>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimerStartInput {
    pub project_id: Option<String>,
    pub description: String,
    pub billable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FirstDayPref {
    #[default]
    System,
    Monday,
    Sunday,
}

impl FirstDayPref {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Monday => "monday",
            Self::Sunday => "sunday",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "monday" => Self::Monday,
            "sunday" => Self::Sunday,
            _ => Self::System,
        }
    }

    pub fn resolve_mon0(self, sys: &SystemLocale) -> u8 {
        match self {
            Self::System => sys.first_day_mon0,
            Self::Monday => 0,
            Self::Sunday => 6,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimeFormatPref {
    #[default]
    System,
    H24,
    H12,
}

impl TimeFormatPref {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::H24 => "24h",
            Self::H12 => "12h",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "24h" => Self::H24,
            "12h" => Self::H12,
            _ => Self::System,
        }
    }

    pub fn is_24h(self, sys: &SystemLocale) -> bool {
        match self {
            Self::System => sys.is_24h,
            Self::H24 => true,
            Self::H12 => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub currency: String,
    pub monitor_enabled: bool,
    pub idle_minutes: u32,
    pub browser_tabs: bool,
    pub claude_sessions: bool,
    pub day_start_hour: u32,
    pub onboarded: bool,
    pub first_day_of_week: FirstDayPref,
    pub time_format: TimeFormatPref,
    pub keep_active_when_busy: bool,
    pub excluded_apps: Vec<String>,
    pub excluded_title_patterns: Vec<String>,
    pub csv_standard: bool,
}

impl Settings {
    pub fn defaults_for(locale: &SystemLocale) -> Self {
        Self {
            currency: locale.currency.clone(),
            monitor_enabled: true,
            idle_minutes: 3,
            browser_tabs: true,
            claude_sessions: true,
            day_start_hour: 8,
            onboarded: false,
            first_day_of_week: FirstDayPref::System,
            time_format: TimeFormatPref::System,
            keep_active_when_busy: true,
            excluded_apps: vec!["1Password".into(), "Bitwarden".into(), "KeePass".into()],
            excluded_title_patterns: Vec::new(),
            csv_standard: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsPatch {
    pub currency: Option<String>,
    pub monitor_enabled: Option<bool>,
    pub idle_minutes: Option<u32>,
    pub browser_tabs: Option<bool>,
    pub claude_sessions: Option<bool>,
    pub day_start_hour: Option<u32>,
    pub onboarded: Option<bool>,
    pub first_day_of_week: Option<FirstDayPref>,
    pub time_format: Option<TimeFormatPref>,
    pub keep_active_when_busy: Option<bool>,
    pub excluded_apps: Option<Vec<String>>,
    pub excluded_title_patterns: Option<Vec<String>>,
    pub csv_standard: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CurrentActivity {
    pub app: String,
    pub detail: String,
    pub since: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MonitorStatus {
    pub enabled: bool,
    pub idle: bool,
    pub current: Option<CurrentActivity>,
    pub claude_last_scan: Option<i64>,
}

pub fn new_uuid() -> String {
    // SAFETY: `CoCreateGuid` uses Win32 RPC UUID generation and returns an owned GUID value.
    let g = unsafe { CoCreateGuid() }.unwrap_or_default();
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g.data1,
        g.data2,
        g.data3,
        g.data4[0],
        g.data4[1],
        g.data4[2],
        g.data4[3],
        g.data4[4],
        g.data4[5],
        g.data4[6],
        g.data4[7],
    )
}

pub fn system_now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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

    /// Folds the WAL back into the main file (shutdown, end of session).
    pub fn checkpoint(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
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

    fn activity_lower_bound(&self, from: i64) -> rusqlite::Result<Option<i64>> {
        match self.get_setting(MAX_DURATION_KEY)? {
            Some(v) => {
                let max_d = v.parse::<i64>().unwrap_or(0).max(0);
                Ok(Some(from.saturating_sub(max_d)))
            }
            // Databases created by the Electron app did not maintain `maxActivityMs`;
            // avoid missing runs that started before `from` by omitting the lower index hint.
            None => Ok(None),
        }
    }

    /// Activity overlapping [from, to). The extra `start >= from - max` bound
    /// lets the start index cut from both sides on large databases.
    pub fn list_activity(&self, from: i64, to: i64) -> rusqlite::Result<Vec<Activity>> {
        let mut out = Vec::new();
        self.visit_activity(from, to, |a| {
            out.push(a);
        })?;
        Ok(out)
    }

    /// Streams activity rows overlapping `[from, to)` without requiring callers
    /// that only compute aggregates to retain every row in memory.
    pub fn visit_activity(
        &self,
        from: i64,
        to: i64,
        mut f: impl FnMut(Activity),
    ) -> rusqlite::Result<()> {
        let map_row = |r: &rusqlite::Row<'_>| -> rusqlite::Result<Activity> {
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
        };
        if let Some(lower) = self.activity_lower_bound(from)? {
            let mut stmt = self.conn.prepare(
                "SELECT id, kind, app, bundle_id, detail, context, url, start, end FROM activity
                 WHERE start >= ?1 AND start < ?2 AND end > ?3 AND app != 'Focus' ORDER BY start",
            )?;
            let rows = stmt.query_map(params![lower, to, from], map_row)?;
            for row in rows {
                f(row?);
            }
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT id, kind, app, bundle_id, detail, context, url, start, end FROM activity
                 WHERE start < ?1 AND end > ?2 AND app != 'Focus' ORDER BY start",
            )?;
            let rows = stmt.query_map(params![to, from], map_row)?;
            for row in rows {
                f(row?);
            }
        }
        Ok(())
    }

    /// Computes wall-clock union milliseconds of `kind = 'app'` activity in `[from, to)`
    /// in O(1) memory directly from SQLite's sorted stream.
    pub fn active_union_ms(&self, from: i64, to: i64) -> rusqlite::Result<i64> {
        let mut total = 0i64;
        let mut cur: Option<(i64, i64)> = None;
        let mut feed = |s: i64, e: i64| {
            let a = s.max(from);
            let b = e.min(to);
            if b <= a {
                return;
            }
            match cur {
                Some((cs, ce)) if a <= ce => cur = Some((cs, ce.max(b))),
                Some((cs, ce)) => {
                    total += ce - cs;
                    cur = Some((a, b));
                }
                None => cur = Some((a, b)),
            }
        };
        if let Some(lower) = self.activity_lower_bound(from)? {
            let mut stmt = self.conn.prepare(
                "SELECT start, end FROM activity
                 WHERE kind = 'app' AND start >= ?1 AND start < ?2 AND end > ?3 AND app != 'Focus'
                 ORDER BY start",
            )?;
            let rows = stmt.query_map(params![lower, to, from], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })?;
            for r in rows {
                let (s, e) = r?;
                feed(s, e);
            }
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT start, end FROM activity
                 WHERE kind = 'app' AND start < ?1 AND end > ?2 AND app != 'Focus'
                 ORDER BY start",
            )?;
            let rows = stmt.query_map(params![to, from], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })?;
            for r in rows {
                let (s, e) = r?;
                feed(s, e);
            }
        }
        if let Some((cs, ce)) = cur {
            total += ce - cs;
        }
        Ok(total)
    }

    pub fn earliest_activity_start(&self) -> rusqlite::Result<Option<i64>> {
        self.conn.query_row(
            "SELECT MIN(start) FROM activity WHERE app != 'Focus'",
            [],
            |r| r.get(0),
        )
    }

    pub fn clear_activity(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "DELETE FROM activity;
             DELETE FROM claude_files;
             DELETE FROM claude_sessions;
             DELETE FROM settings WHERE key = 'maxActivityMs';",
        )
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

    pub fn set_activity_detail(&self, id: i64, detail: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE activity SET detail = ? WHERE id = ?",
            params![detail, id],
        )?;
        Ok(())
    }

    // ── Projects ─────────────────────────────────────────────────────────

    pub fn list_projects(&self) -> rusqlite::Result<Vec<Project>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, client, color, rate, currency, billable_default, archived, created_at
             FROM projects ORDER BY archived, created_at",
        )?;
        let rows = stmt.query_map([], map_project)?;
        rows.collect()
    }

    pub fn get_project(&self, id: &str) -> rusqlite::Result<Project> {
        self.conn.query_row(
            "SELECT id, name, client, color, rate, currency, billable_default, archived, created_at
             FROM projects WHERE id = ?",
            [id],
            map_project,
        )
    }

    pub fn create_project(&self, input: &ProjectInput, now_ms: i64) -> rusqlite::Result<Project> {
        let name = input.name.trim();
        if name.is_empty() {
            return Err(rusqlite::Error::InvalidParameterName(
                "project name is empty".into(),
            ));
        }
        let id = new_uuid();
        self.conn.execute(
            "INSERT INTO projects (id, name, client, color, rate, currency, billable_default, archived, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                id,
                name,
                input.client.trim(),
                input.color,
                input.rate.max(0.0),
                input.currency,
                i64::from(input.billable_default),
                i64::from(input.archived),
                now_ms,
            ],
        )?;
        self.get_project(&id)
    }

    pub fn update_project(&self, id: &str, patch: &ProjectPatch) -> rusqlite::Result<Project> {
        let mut p = self.get_project(id)?;
        if let Some(v) = &patch.name {
            p.name = v.clone();
        }
        if let Some(v) = &patch.client {
            p.client = v.clone();
        }
        if let Some(v) = &patch.color {
            p.color = v.clone();
        }
        if let Some(v) = patch.rate {
            p.rate = v;
        }
        if let Some(v) = &patch.currency {
            p.currency = v.clone();
        }
        if let Some(v) = patch.billable_default {
            p.billable_default = v;
        }
        if let Some(v) = patch.archived {
            p.archived = v;
        }
        let name = p.name.trim();
        if name.is_empty() {
            return Err(rusqlite::Error::InvalidParameterName(
                "project name is empty".into(),
            ));
        }
        self.conn.execute(
            "UPDATE projects SET name = ?, client = ?, color = ?, rate = ?, currency = ?, billable_default = ?, archived = ?
             WHERE id = ?",
            params![
                name,
                p.client.trim(),
                p.color,
                p.rate.max(0.0),
                p.currency,
                i64::from(p.billable_default),
                i64::from(p.archived),
                id,
            ],
        )?;
        self.get_project(id)
    }

    pub fn delete_project(&self, id: &str) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM projects WHERE id = ?", [id])?;
        Ok(())
    }

    /// Aggregates per-project month/all-time/earned stats inside SQLite without loading all entries.
    pub fn project_summaries(
        &self,
        month_from: i64,
        month_to: i64,
        now_ms: i64,
    ) -> rusqlite::Result<HashMap<String, ProjectSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.project_id,
                    SUM(MAX(0, COALESCE(e.end, ?1) - e.start)) AS total_ms,
                    SUM(MAX(0, MIN(COALESCE(e.end, ?1), ?3) - MAX(e.start, ?2))) AS month_ms,
                    SUM(CASE WHEN e.billable = 1 THEN (MAX(0, COALESCE(e.end, ?1) - e.start) / 3600000.0) * p.rate ELSE 0.0 END) AS earned
             FROM entries e
             JOIN projects p ON p.id = e.project_id
             WHERE e.project_id IS NOT NULL
             GROUP BY e.project_id",
        )?;
        let rows = stmt.query_map(params![now_ms, month_from, month_to], |r| {
            Ok((
                r.get::<_, String>(0)?,
                ProjectSummary {
                    total_ms: r.get(1)?,
                    month_ms: r.get(2)?,
                    earned: r.get(3)?,
                },
            ))
        })?;
        rows.collect()
    }

    pub fn project_all_time_stats(
        &self,
        project_id: &str,
        now_ms: i64,
    ) -> rusqlite::Result<ProjectAllTimeStats> {
        self.conn.query_row(
            "SELECT MIN(e.start),
                    COALESCE(SUM(MAX(0, COALESCE(e.end, ?2) - e.start)), 0),
                    COALESCE(SUM(CASE WHEN e.billable = 1 THEN (MAX(0, COALESCE(e.end, ?2) - e.start) / 3600000.0) * p.rate ELSE 0.0 END), 0.0),
                    COUNT(*)
             FROM entries e
             JOIN projects p ON p.id = e.project_id
             WHERE e.project_id = ?1",
            params![project_id, now_ms],
            |r| {
                Ok(ProjectAllTimeStats {
                    first_start: r.get(0)?,
                    total_ms: r.get(1)?,
                    earned: r.get(2)?,
                    count: r.get::<_, i64>(3)?.max(0) as usize,
                })
            },
        )
    }

    // ── Time entries ─────────────────────────────────────────────────────

    pub fn list_entries(&self, from: i64, to: i64) -> rusqlite::Result<Vec<TimeEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, description, start, end, billable, source FROM entries
             WHERE start < ?1 AND (end IS NULL OR end > ?2) ORDER BY start",
        )?;
        let rows = stmt.query_map(params![to, from], map_entry)?;
        rows.collect()
    }

    pub fn get_entry(&self, id: &str) -> rusqlite::Result<TimeEntry> {
        self.conn.query_row(
            "SELECT id, project_id, description, start, end, billable, source FROM entries WHERE id = ?",
            [id],
            map_entry,
        )
    }

    pub fn has_any_entries(&self) -> rusqlite::Result<bool> {
        let count: i64 = self
            .conn
            .query_row("SELECT EXISTS(SELECT 1 FROM entries)", [], |r| r.get(0))?;
        Ok(count != 0)
    }

    pub fn earliest_entry_start(
        &self,
        project_id: Option<&str>,
        no_project_only: bool,
    ) -> rusqlite::Result<Option<i64>> {
        if no_project_only {
            self.conn.query_row(
                "SELECT MIN(start) FROM entries WHERE project_id IS NULL",
                [],
                |r| r.get(0),
            )
        } else if let Some(pid) = project_id {
            self.conn.query_row(
                "SELECT MIN(start) FROM entries WHERE project_id = ?",
                [pid],
                |r| r.get(0),
            )
        } else {
            self.conn
                .query_row("SELECT MIN(start) FROM entries", [], |r| r.get(0))
        }
    }

    pub fn create_entry(&self, input: &EntryInput) -> rusqlite::Result<TimeEntry> {
        check_entry_times(input.start, input.end)?;
        let id = new_uuid();
        self.conn.execute(
            "INSERT INTO entries (id, project_id, description, start, end, billable, source)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                id,
                input.project_id,
                input.description.trim(),
                input.start,
                input.end,
                i64::from(input.billable),
                input.source,
            ],
        )?;
        self.get_entry(&id)
    }

    pub fn update_entry(&self, id: &str, patch: &EntryPatch) -> rusqlite::Result<TimeEntry> {
        let mut e = self.get_entry(id)?;
        if let Some(pid) = &patch.project_id {
            e.project_id = pid.clone();
        }
        if let Some(desc) = &patch.description {
            e.description = desc.clone();
        }
        if let Some(s) = patch.start {
            e.start = s;
        }
        if let Some(end) = patch.end {
            e.end = end;
        }
        if let Some(b) = patch.billable {
            e.billable = b;
        }
        if let Some(src) = &patch.source {
            e.source = src.clone();
        }
        check_entry_times(e.start, e.end)?;
        self.conn.execute(
            "UPDATE entries SET project_id = ?, description = ?, start = ?, end = ?, billable = ?, source = ?
             WHERE id = ?",
            params![
                e.project_id,
                e.description.trim(),
                e.start,
                e.end,
                i64::from(e.billable),
                e.source,
                id,
            ],
        )?;
        self.get_entry(id)
    }

    pub fn delete_entry(&self, id: &str) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM entries WHERE id = ?", [id])?;
        Ok(())
    }

    pub fn running_entry(&self) -> rusqlite::Result<Option<TimeEntry>> {
        self.conn
            .query_row(
                "SELECT id, project_id, description, start, end, billable, source
                 FROM entries WHERE end IS NULL ORDER BY start DESC LIMIT 1",
                [],
                map_entry,
            )
            .optional()
    }

    pub fn start_timer(&self, input: &TimerStartInput, now_ms: i64) -> rusqlite::Result<TimeEntry> {
        let _ = self.stop_all_timers(now_ms)?;
        self.create_entry(&EntryInput {
            project_id: input.project_id.clone(),
            description: input.description.clone(),
            start: now_ms,
            end: None,
            billable: input.billable,
            source: "timer".into(),
        })
    }

    /// Ends every running entry at `now_ms`; timer entries under 1 minute are dropped as accidental.
    pub fn stop_all_timers(&self, now_ms: i64) -> rusqlite::Result<Option<TimeEntry>> {
        let Some(running) = self.running_entry()? else {
            return Ok(None);
        };
        let end = now_ms.max(running.start + 1000);
        self.conn
            .execute("UPDATE entries SET end = ? WHERE end IS NULL", [end])?;
        self.conn.execute(
            "DELETE FROM entries WHERE end IS NOT NULL AND end - start < 60000 AND source = 'timer'",
            [],
        )?;
        self.get_entry(&running.id).optional()
    }

    // ── Claude Code logs ─────────────────────────────────────────────────

    pub fn claude_session(&self, id: &str) -> rusqlite::Result<Option<ClaudeSession>> {
        self.conn
            .query_row(
                "SELECT automated, project, title, last_ts, activity_id FROM claude_sessions WHERE session_id = ?",
                [id],
                |r| {
                    Ok(ClaudeSession {
                        automated: r.get::<_, i64>(0)? != 0,
                        project: r.get(1)?,
                        title: r.get(2)?,
                        last_ts: r.get(3)?,
                        activity_id: r.get(4)?,
                    })
                },
            )
            .optional()
    }

    pub fn save_claude_session(&self, id: &str, s: &ClaudeSession) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO claude_sessions (session_id, automated, project, title, last_ts, activity_id)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(session_id) DO UPDATE SET automated = excluded.automated,
               project = excluded.project, title = excluded.title,
               last_ts = excluded.last_ts, activity_id = excluded.activity_id",
            params![id, s.automated as i64, s.project, s.title, s.last_ts, s.activity_id],
        )?;
        Ok(())
    }

    /// (byte offset, mtime in ms) already consumed for a log file.
    pub fn claude_file(&self, path: &str) -> rusqlite::Result<Option<(i64, i64)>> {
        self.conn
            .query_row(
                "SELECT offset, mtime FROM claude_files WHERE path = ?",
                [path],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
    }

    pub fn save_claude_file(&self, path: &str, offset: i64, mtime: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO claude_files (path, offset, mtime) VALUES (?, ?, ?)
             ON CONFLICT(path) DO UPDATE SET offset = excluded.offset, mtime = excluded.mtime",
            params![path, offset, mtime],
        )?;
        Ok(())
    }

    /// Runs `f` in one transaction: many log lines become one disk sync.
    pub fn in_transaction<T, E: From<rusqlite::Error>>(
        &self,
        f: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        self.conn.execute_batch("BEGIN")?;
        match f() {
            Ok(v) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(v)
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
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

    pub fn load_settings(&self, locale: &SystemLocale) -> rusqlite::Result<Settings> {
        let mut s = Settings::defaults_for(locale);
        let mut stmt = self.conn.prepare("SELECT key, value FROM settings")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (k, raw) = row?;
            let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };
            match k.as_str() {
                "currency" => {
                    if let Some(v) = val.as_str() {
                        s.currency = v.to_string();
                    }
                }
                "monitorEnabled" => {
                    if let Some(v) = val.as_bool() {
                        s.monitor_enabled = v;
                    }
                }
                "idleMinutes" => {
                    if let Some(v) = val.as_u64() {
                        s.idle_minutes = v.min(120) as u32;
                    }
                }
                "browserTabs" => {
                    if let Some(v) = val.as_bool() {
                        s.browser_tabs = v;
                    }
                }
                "claudeSessions" => {
                    if let Some(v) = val.as_bool() {
                        s.claude_sessions = v;
                    }
                }
                "dayStartHour" => {
                    if let Some(v) = val.as_u64() {
                        s.day_start_hour = v.min(12) as u32;
                    }
                }
                "onboarded" => {
                    if let Some(v) = val.as_bool() {
                        s.onboarded = v;
                    }
                }
                "firstDayOfWeek" => {
                    if let Some(v) = val.as_str() {
                        s.first_day_of_week = FirstDayPref::from_str(v);
                    }
                }
                "timeFormat" => {
                    if let Some(v) = val.as_str() {
                        s.time_format = TimeFormatPref::from_str(v);
                    }
                }
                "keepActiveWhenBusy" => {
                    if let Some(v) = val.as_bool() {
                        s.keep_active_when_busy = v;
                    }
                }
                "excludedApps" => {
                    if let Some(arr) = val.as_array() {
                        s.excluded_apps = arr
                            .iter()
                            .filter_map(|x| x.as_str().map(|t| t.trim().to_string()))
                            .filter(|t| !t.is_empty())
                            .collect();
                    }
                }
                "excludedTitlePatterns" => {
                    if let Some(arr) = val.as_array() {
                        s.excluded_title_patterns = arr
                            .iter()
                            .filter_map(|x| x.as_str().map(|t| t.trim().to_string()))
                            .filter(|t| !t.is_empty())
                            .collect();
                    }
                }
                "csvStandard" => {
                    if let Some(v) = val.as_bool() {
                        s.csv_standard = v;
                    }
                }
                _ => {}
            }
        }
        Ok(s)
    }

    pub fn save_settings(
        &self,
        patch: &SettingsPatch,
        locale: &SystemLocale,
    ) -> rusqlite::Result<Settings> {
        self.in_transaction(|| -> rusqlite::Result<()> {
            if let Some(v) = &patch.currency {
                self.set_setting("currency", &serde_json::to_string(v).unwrap())?;
            }
            if let Some(v) = patch.monitor_enabled {
                self.set_setting("monitorEnabled", &serde_json::to_string(&v).unwrap())?;
            }
            if let Some(v) = patch.idle_minutes {
                self.set_setting("idleMinutes", &serde_json::to_string(&v).unwrap())?;
            }
            if let Some(v) = patch.browser_tabs {
                self.set_setting("browserTabs", &serde_json::to_string(&v).unwrap())?;
            }
            if let Some(v) = patch.claude_sessions {
                self.set_setting("claudeSessions", &serde_json::to_string(&v).unwrap())?;
            }
            if let Some(v) = patch.day_start_hour {
                self.set_setting("dayStartHour", &serde_json::to_string(&v).unwrap())?;
            }
            if let Some(v) = patch.onboarded {
                self.set_setting("onboarded", &serde_json::to_string(&v).unwrap())?;
            }
            if let Some(v) = patch.first_day_of_week {
                self.set_setting(
                    "firstDayOfWeek",
                    &serde_json::to_string(v.as_str()).unwrap(),
                )?;
            }
            if let Some(v) = patch.time_format {
                self.set_setting("timeFormat", &serde_json::to_string(v.as_str()).unwrap())?;
            }
            if let Some(v) = patch.keep_active_when_busy {
                self.set_setting("keepActiveWhenBusy", &serde_json::to_string(&v).unwrap())?;
            }
            if let Some(v) = &patch.excluded_apps {
                self.set_setting("excludedApps", &serde_json::to_string(v).unwrap())?;
            }
            if let Some(v) = &patch.excluded_title_patterns {
                self.set_setting("excludedTitlePatterns", &serde_json::to_string(v).unwrap())?;
            }
            if let Some(v) = patch.csv_standard {
                self.set_setting("csvStandard", &serde_json::to_string(&v).unwrap())?;
            }
            Ok(())
        })?;
        self.load_settings(locale)
    }
}

fn check_entry_times(start: i64, end: Option<i64>) -> rusqlite::Result<()> {
    if let Some(e) = end {
        if e <= start {
            return Err(rusqlite::Error::InvalidParameterName(
                "End must be after start".into(),
            ));
        }
    }
    Ok(())
}

fn map_project(r: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        name: r.get(1)?,
        client: r.get(2)?,
        color: r.get(3)?,
        rate: r.get(4)?,
        currency: r.get(5)?,
        billable_default: r.get::<_, i64>(6)? != 0,
        archived: r.get::<_, i64>(7)? != 0,
        created_at: r.get(8)?,
    })
}

fn map_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<TimeEntry> {
    Ok(TimeEntry {
        id: r.get(0)?,
        project_id: r.get(1)?,
        description: r.get(2)?,
        start: r.get(3)?,
        end: r.get(4)?,
        billable: r.get::<_, i64>(5)? != 0,
        source: r.get(6)?,
    })
}

// ── Shared store abstraction (real SQLite DbStore vs DemoStore) ──────────

/// Unified storage interface shared by all F3 screens across real SQLite
/// databases and `--demo` in-memory mode.
pub trait Store {
    fn now_ms(&self) -> i64;
    fn is_demo(&self) -> bool;
    fn system_locale(&self) -> &SystemLocale;

    fn list_projects(&self) -> Result<Vec<Project>, String>;
    fn get_project(&self, id: &str) -> Result<Project, String>;
    fn create_project(&mut self, input: ProjectInput) -> Result<Project, String>;
    fn update_project(&mut self, id: &str, patch: ProjectPatch) -> Result<Project, String>;
    fn delete_project(&mut self, id: &str) -> Result<(), String>;
    fn project_summaries(
        &self,
        month_from: i64,
        month_to: i64,
    ) -> Result<HashMap<String, ProjectSummary>, String>;
    fn project_all_time_stats(&self, project_id: &str) -> Result<ProjectAllTimeStats, String>;

    fn list_entries(&self, from: i64, to: i64) -> Result<Vec<TimeEntry>, String>;
    fn has_any_entries(&self) -> Result<bool, String>;
    fn earliest_entry_start(
        &self,
        project_id: Option<&str>,
        no_project_only: bool,
    ) -> Result<Option<i64>, String>;
    fn create_entry(&mut self, input: EntryInput) -> Result<TimeEntry, String>;
    fn update_entry(&mut self, id: &str, patch: EntryPatch) -> Result<TimeEntry, String>;
    fn delete_entry(&mut self, id: &str) -> Result<(), String>;
    fn running_entry(&self) -> Result<Option<TimeEntry>, String>;
    fn start_timer(&mut self, input: TimerStartInput) -> Result<TimeEntry, String>;
    fn stop_timer(&mut self) -> Result<Option<TimeEntry>, String>;

    fn list_activity(&self, from: i64, to: i64) -> Result<Vec<Activity>, String>;
    fn visit_activity(&self, from: i64, to: i64, f: &mut dyn FnMut(Activity))
        -> Result<(), String>;
    fn active_union_ms(&self, from: i64, to: i64) -> Result<i64, String>;
    fn earliest_activity_start(&self) -> Result<Option<i64>, String>;
    fn clear_activity(&mut self) -> Result<(), String>;

    fn get_settings(&self) -> Result<Settings, String>;
    fn save_settings(&mut self, patch: SettingsPatch) -> Result<Settings, String>;
    fn monitor_status(&self) -> MonitorStatus;
}

pub type SharedStore = Rc<RefCell<Box<dyn Store>>>;

pub fn shared_store(store: Box<dyn Store>) -> SharedStore {
    Rc::new(RefCell::new(store))
}

pub struct DbStore {
    db: Db,
    locale: SystemLocale,
}

impl DbStore {
    pub fn new(db: Db, locale: SystemLocale) -> Self {
        Self { db, locale }
    }
}

fn db_err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

impl Store for DbStore {
    fn now_ms(&self) -> i64 {
        system_now_ms()
    }

    fn is_demo(&self) -> bool {
        false
    }

    fn system_locale(&self) -> &SystemLocale {
        &self.locale
    }

    fn list_projects(&self) -> Result<Vec<Project>, String> {
        self.db.list_projects().map_err(db_err)
    }

    fn get_project(&self, id: &str) -> Result<Project, String> {
        self.db.get_project(id).map_err(db_err)
    }

    fn create_project(&mut self, input: ProjectInput) -> Result<Project, String> {
        self.db
            .create_project(&input, self.now_ms())
            .map_err(db_err)
    }

    fn update_project(&mut self, id: &str, patch: ProjectPatch) -> Result<Project, String> {
        self.db.update_project(id, &patch).map_err(db_err)
    }

    fn delete_project(&mut self, id: &str) -> Result<(), String> {
        self.db.delete_project(id).map_err(db_err)
    }

    fn project_summaries(
        &self,
        month_from: i64,
        month_to: i64,
    ) -> Result<HashMap<String, ProjectSummary>, String> {
        self.db
            .project_summaries(month_from, month_to, self.now_ms())
            .map_err(db_err)
    }

    fn project_all_time_stats(&self, project_id: &str) -> Result<ProjectAllTimeStats, String> {
        self.db
            .project_all_time_stats(project_id, self.now_ms())
            .map_err(db_err)
    }

    fn list_entries(&self, from: i64, to: i64) -> Result<Vec<TimeEntry>, String> {
        self.db.list_entries(from, to).map_err(db_err)
    }

    fn has_any_entries(&self) -> Result<bool, String> {
        self.db.has_any_entries().map_err(db_err)
    }

    fn earliest_entry_start(
        &self,
        project_id: Option<&str>,
        no_project_only: bool,
    ) -> Result<Option<i64>, String> {
        self.db
            .earliest_entry_start(project_id, no_project_only)
            .map_err(db_err)
    }

    fn create_entry(&mut self, input: EntryInput) -> Result<TimeEntry, String> {
        self.db.create_entry(&input).map_err(db_err)
    }

    fn update_entry(&mut self, id: &str, patch: EntryPatch) -> Result<TimeEntry, String> {
        self.db.update_entry(id, &patch).map_err(db_err)
    }

    fn delete_entry(&mut self, id: &str) -> Result<(), String> {
        self.db.delete_entry(id).map_err(db_err)
    }

    fn running_entry(&self) -> Result<Option<TimeEntry>, String> {
        self.db.running_entry().map_err(db_err)
    }

    fn start_timer(&mut self, input: TimerStartInput) -> Result<TimeEntry, String> {
        self.db.start_timer(&input, self.now_ms()).map_err(db_err)
    }

    fn stop_timer(&mut self) -> Result<Option<TimeEntry>, String> {
        self.db.stop_all_timers(self.now_ms()).map_err(db_err)
    }

    fn list_activity(&self, from: i64, to: i64) -> Result<Vec<Activity>, String> {
        self.db.list_activity(from, to).map_err(db_err)
    }

    fn visit_activity(
        &self,
        from: i64,
        to: i64,
        f: &mut dyn FnMut(Activity),
    ) -> Result<(), String> {
        self.db.visit_activity(from, to, f).map_err(db_err)
    }

    fn active_union_ms(&self, from: i64, to: i64) -> Result<i64, String> {
        self.db.active_union_ms(from, to).map_err(db_err)
    }

    fn earliest_activity_start(&self) -> Result<Option<i64>, String> {
        self.db.earliest_activity_start().map_err(db_err)
    }

    fn clear_activity(&mut self) -> Result<(), String> {
        self.db.clear_activity().map_err(db_err)
    }

    fn get_settings(&self) -> Result<Settings, String> {
        self.db.load_settings(&self.locale).map_err(db_err)
    }

    fn save_settings(&mut self, patch: SettingsPatch) -> Result<Settings, String> {
        self.db.save_settings(&patch, &self.locale).map_err(db_err)
    }

    fn monitor_status(&self) -> MonitorStatus {
        let enabled = self
            .db
            .load_settings(&self.locale)
            .map(|s| s.monitor_enabled)
            .unwrap_or(true);
        MonitorStatus {
            enabled,
            idle: false,
            current: None,
            claude_last_scan: None,
        }
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

    #[test]
    fn projects_entries_timers_and_sql_summaries_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        let loc = SystemLocale::pt_br();
        let p = db
            .create_project(
                &ProjectInput {
                    name: "Northwind".into(),
                    client: "Traders".into(),
                    color: "#d9488a".into(),
                    rate: 150.0,
                    currency: "BRL".into(),
                    billable_default: true,
                    archived: false,
                },
                1_000,
            )
            .unwrap();
        assert_eq!(p.currency, "BRL");
        assert_eq!(db.list_projects().unwrap().len(), 1);

        let e1 = db
            .create_entry(&EntryInput {
                project_id: Some(p.id.clone()),
                description: "Design".into(),
                start: 10_000,
                end: Some(10_000 + 3_600_000),
                billable: true,
                source: "calendar".into(),
            })
            .unwrap();
        assert_eq!(db.list_entries(0, 100_000).unwrap().len(), 1);
        assert_eq!(db.list_entries(50_000_000, 60_000_000).unwrap().len(), 0);

        let summaries = db.project_summaries(0, 5_000_000, 5_000_000).unwrap();
        let s = summaries.get(&p.id).unwrap();
        assert_eq!(s.total_ms, 3_600_000);
        assert!((s.earned - 150.0).abs() < 1e-6);

        // Accidental timer (< 60s) is dropped when stopped.
        let t_short = db
            .start_timer(
                &TimerStartInput {
                    project_id: Some(p.id.clone()),
                    description: "Oops".into(),
                    billable: true,
                },
                20_000_000,
            )
            .unwrap();
        assert_eq!(db.running_entry().unwrap().unwrap().id, t_short.id);
        assert!(db.stop_all_timers(20_010_000).unwrap().is_none());
        assert!(db.running_entry().unwrap().is_none());

        // Timer >= 60s is kept on stop.
        let t_long = db
            .start_timer(
                &TimerStartInput {
                    project_id: Some(p.id.clone()),
                    description: "Deep work".into(),
                    billable: true,
                },
                30_000_000,
            )
            .unwrap();
        let stopped = db.stop_all_timers(30_120_000).unwrap().unwrap();
        assert_eq!(stopped.id, t_long.id);
        assert_eq!(stopped.end, Some(30_120_000));

        // Deleting a project clears project_id on entries via ON DELETE SET NULL.
        db.delete_project(&p.id).unwrap();
        assert!(db.get_entry(&e1.id).unwrap().project_id.is_none());

        // Settings JSON roundtrip respects locale defaults.
        let def = db.load_settings(&loc).unwrap();
        assert_eq!(def.currency, "BRL");
        let saved = db
            .save_settings(
                &SettingsPatch {
                    onboarded: Some(true),
                    idle_minutes: Some(5),
                    csv_standard: Some(true),
                    ..Default::default()
                },
                &loc,
            )
            .unwrap();
        assert!(saved.onboarded);
        assert_eq!(saved.idle_minutes, 5);
        assert!(saved.csv_standard);
    }

    #[test]
    fn active_union_ms_streams_overlapping_runs_without_double_counting() {
        let db = Db::open_in_memory().unwrap();
        db.insert_activity(&act("Code", 10_000, 30_000)).unwrap();
        db.insert_activity(&act("Edge", 20_000, 50_000)).unwrap();
        db.insert_activity(&Activity {
            id: 0,
            kind: "claude".into(),
            app: "Claude Code".into(),
            bundle_id: String::new(),
            detail: "Task".into(),
            context: "repo".into(),
            url: String::new(),
            start: 0,
            end: 100_000,
        })
        .unwrap();
        assert_eq!(db.active_union_ms(0, 100_000).unwrap(), 40_000);
        db.clear_activity().unwrap();
        assert_eq!(db.active_union_ms(0, 100_000).unwrap(), 0);
    }
}
