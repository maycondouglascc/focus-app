//! Turns confirmed foreground changes into activity runs in the database.
//!
//! A run is one stretch of the same app and window title. The current run is
//! inserted when it starts and its end is moved forward on `tick` (every 60 s)
//! and on every switch, so a power cut loses at most 60 s.

use crate::db::{Activity, Db};

/// What the monitor learned about the foreground window.
#[derive(Debug, Clone)]
pub struct Info {
    /// Stable identity: normalized exe path (AUMID later).
    pub key: String,
    /// Display name.
    pub app: String,
    /// Window title.
    pub detail: String,
}

struct Run {
    id: i64,
    start: i64,
    key: String,
    detail: String,
    end: i64,
}

#[derive(Default)]
pub struct Recorder {
    current: Option<Run>,
}

impl Recorder {
    /// The foreground window was confirmed at `at` (epoch ms, the time of the
    /// event, not of the debounce confirmation).
    pub fn on_focus(&mut self, db: &Db, info: Info, at: i64) -> rusqlite::Result<()> {
        if let Some(run) = &self.current {
            if run.key == info.key && run.detail == info.detail {
                return Ok(());
            }
        }
        self.close(db, at)?;
        let id = db.insert_activity(&Activity {
            id: 0,
            kind: "app".into(),
            app: info.app,
            bundle_id: info.key.clone(),
            detail: info.detail.clone(),
            context: String::new(),
            url: String::new(),
            start: at,
            end: at,
        })?;
        self.current = Some(Run {
            id,
            start: at,
            key: info.key,
            detail: info.detail,
            end: at,
        });
        Ok(())
    }

    /// Periodic flush of the current run's end.
    pub fn tick(&mut self, db: &Db, now: i64) -> rusqlite::Result<()> {
        if let Some(run) = &mut self.current {
            if now > run.end {
                db.set_activity_end(run.id, now)?;
                run.end = now;
            }
        }
        Ok(())
    }

    /// Ends the current run at `at` (switch, lock, suspend, shutdown). `at`
    /// may be earlier than the last flush: idle starts at the last input, so
    /// the end is moved back, never before the run's own start.
    pub fn close(&mut self, db: &Db, at: i64) -> rusqlite::Result<()> {
        if let Some(run) = self.current.take() {
            let end = at.max(run.start);
            if end != run.end {
                db.set_activity_end(run.id, end)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(key: &str, title: &str) -> Info {
        Info {
            key: key.into(),
            app: key.into(),
            detail: title.into(),
        }
    }

    #[test]
    fn switching_closes_previous_run_at_the_event_time() {
        let db = Db::open_in_memory().unwrap();
        let mut r = Recorder::default();
        r.on_focus(&db, info("code", "a.rs"), 1_000).unwrap();
        r.on_focus(&db, info("chrome", "Docs"), 9_000).unwrap();
        r.close(&db, 20_000).unwrap();
        let rows = db.list_activity(0, 100_000).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0].app.as_str(), rows[0].start, rows[0].end),
            ("code", 1_000, 9_000)
        );
        assert_eq!(
            (rows[1].app.as_str(), rows[1].start, rows[1].end),
            ("chrome", 9_000, 20_000)
        );
    }

    #[test]
    fn same_app_and_title_extends_instead_of_splitting() {
        let db = Db::open_in_memory().unwrap();
        let mut r = Recorder::default();
        r.on_focus(&db, info("code", "a.rs"), 1_000).unwrap();
        r.on_focus(&db, info("code", "a.rs"), 5_000).unwrap();
        r.tick(&db, 60_000).unwrap();
        let rows = db.list_activity(0, 100_000).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].end, 60_000);
    }

    #[test]
    fn title_change_in_same_app_starts_a_new_run() {
        let db = Db::open_in_memory().unwrap();
        let mut r = Recorder::default();
        r.on_focus(&db, info("chrome", "Inbox"), 1_000).unwrap();
        r.on_focus(&db, info("chrome", "Docs"), 4_000).unwrap();
        assert_eq!(db.list_activity(0, 100_000).unwrap().len(), 2);
    }

    #[test]
    fn tick_never_moves_the_end_backwards() {
        let db = Db::open_in_memory().unwrap();
        let mut r = Recorder::default();
        r.on_focus(&db, info("code", "a.rs"), 10_000).unwrap();
        r.tick(&db, 50_000).unwrap();
        r.tick(&db, 40_000).unwrap();
        assert_eq!(db.list_activity(0, 100_000).unwrap()[0].end, 50_000);
    }

    #[test]
    fn close_moves_the_end_back_for_retroactive_idle() {
        let db = Db::open_in_memory().unwrap();
        let mut r = Recorder::default();
        r.on_focus(&db, info("code", "a.rs"), 10_000).unwrap();
        r.tick(&db, 70_000).unwrap(); // flushed while the user was already away
        r.close(&db, 40_000).unwrap(); // last input was at 40 s
        assert_eq!(db.list_activity(0, 100_000).unwrap()[0].end, 40_000);
    }

    #[test]
    fn close_never_ends_before_the_start() {
        let db = Db::open_in_memory().unwrap();
        let mut r = Recorder::default();
        r.on_focus(&db, info("code", "a.rs"), 10_000).unwrap();
        r.close(&db, 5_000).unwrap();
        assert_eq!(db.list_activity(0, 100_000).unwrap()[0].end, 10_000);
    }

    #[test]
    fn close_without_a_run_is_a_no_op() {
        let db = Db::open_in_memory().unwrap();
        Recorder::default().close(&db, 5).unwrap();
        assert!(db.list_activity(0, 100).unwrap().is_empty());
    }
}
