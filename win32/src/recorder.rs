//! Turns confirmed foreground changes into activity runs in the database.
//!
//! A run is one stretch of the same app and window title. The current run is
//! inserted when it starts and its end is moved forward on `tick` (every 60 s)
//! and on every switch, so a power cut loses at most 60 s.
//! Excluded apps and window title keywords close the current run and record
//! nothing until focus moves elsewhere.

use crate::db::{Activity, Db};

/// What the monitor learned about the foreground window.
#[derive(Debug, Clone)]
pub struct Info {
    /// Stable identity: normalized exe path (AUMID / Store package later).
    pub key: String,
    /// Display name.
    pub app: String,
    /// Lowercase executable stem (e.g. `"msedge"`, `"1password"`).
    pub exe: String,
    /// Window title.
    pub detail: String,
    /// Process id of the real foreground window (0 in unit tests).
    pub pid: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Exclusions {
    pub apps: Vec<String>,
    pub title_keywords: Vec<String>,
}

impl Exclusions {
    pub fn matches(&self, info: &Info) -> bool {
        for pat in &self.apps {
            let p = pat.trim();
            if p.is_empty() {
                continue;
            }
            let p_stem = p
                .strip_suffix(".exe")
                .or_else(|| p.strip_suffix(".EXE"))
                .unwrap_or(p);
            if info.app.eq_ignore_ascii_case(p)
                || (!info.exe.is_empty()
                    && (info.exe.eq_ignore_ascii_case(p_stem)
                        || format!("{}.exe", info.exe).eq_ignore_ascii_case(p)))
            {
                return true;
            }
        }
        if !self.title_keywords.is_empty() && !info.detail.is_empty() {
            let title_lower = info.detail.to_lowercase();
            for kw in &self.title_keywords {
                let k = kw.trim();
                if !k.is_empty() && title_lower.contains(&k.to_lowercase()) {
                    return true;
                }
            }
        }
        false
    }
}

pub fn is_supported_browser_exe(exe: &str) -> bool {
    matches!(
        exe.to_ascii_lowercase().as_str(),
        "chrome" | "msedge" | "firefox" | "brave" | "vivaldi" | "opera"
    )
}

pub fn is_private_browser_title(exe: &str, title: &str) -> bool {
    if !is_supported_browser_exe(exe) {
        return false;
    }
    let lower = title.to_lowercase();
    [
        "inprivate",
        "incognito",
        "anônima",
        "anonima",
        "anônimo",
        "anonimo",
        "private browsing",
        "navegação privativa",
        "navegacao privativa",
        "(private)",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
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
    exclusions: Exclusions,
}

impl Recorder {
    pub fn set_exclusions(&mut self, exclusions: Exclusions) {
        self.exclusions = exclusions;
    }

    pub fn exclusions(&self) -> &Exclusions {
        &self.exclusions
    }

    /// The foreground window was confirmed at `at` (epoch ms, the time of the
    /// event, not of the debounce confirmation).
    /// Returns `Some(activity_id)` when a new run was inserted, `None` when
    /// an existing run was extended or the window was excluded.
    pub fn on_focus(&mut self, db: &Db, mut info: Info, at: i64) -> rusqlite::Result<Option<i64>> {
        if self.exclusions.matches(&info) {
            self.close(db, at)?;
            return Ok(None);
        }
        if is_private_browser_title(&info.exe, &info.detail) {
            info.detail.clear();
        }
        if let Some(run) = &self.current {
            if run.key == info.key && run.detail == info.detail {
                return Ok(None);
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
        Ok(Some(id))
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
            exe: key.to_ascii_lowercase(),
            detail: title.into(),
            pid: 0,
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

    #[test]
    fn exclusions_close_current_run_and_skip_matching_apps_and_titles() {
        let cases: &[(&str, &str, &str, &str, bool)] = &[
            ("1Password", "1password", "Vault - Main", "1Password", true),
            (
                "1Password",
                "1password",
                "Vault - Main",
                "1password.exe",
                true,
            ),
            ("KeePass 2", "keepass", "Passwords", "keepass", true),
            (
                "Microsoft Edge",
                "msedge",
                "Secret Bank Login",
                "Secret",
                true,
            ),
            (
                "Microsoft Edge",
                "msedge",
                "my private diary",
                "PRIVATE",
                true,
            ),
            ("Visual Studio Code", "code", "main.rs", "1Password", false),
        ];

        for &(app_name, exe, title, rule, should_exclude) in cases {
            let db = Db::open_in_memory().unwrap();
            let mut r = Recorder::default();
            r.set_exclusions(Exclusions {
                apps: vec![rule.into()],
                title_keywords: vec![rule.into()],
            });
            r.on_focus(&db, info("code", "work.rs"), 1_000).unwrap();
            let target = Info {
                key: format!(r"c:\apps\{exe}.exe"),
                app: app_name.into(),
                exe: exe.into(),
                detail: title.into(),
                pid: 42,
            };
            let inserted = r.on_focus(&db, target, 5_000).unwrap();
            if should_exclude {
                assert!(
                    inserted.is_none(),
                    "expected exclusion for {app_name}/{title} with {rule}"
                );
                let rows = db.list_activity(0, 100_000).unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].end, 5_000);
                assert!(!rows.iter().any(|row| row.detail.contains(title)));
            } else {
                assert!(inserted.is_some());
                assert_eq!(db.list_activity(0, 100_000).unwrap().len(), 2);
            }
        }
    }

    #[test]
    fn private_browser_windows_strip_title_and_never_store_it() {
        let db = Db::open_in_memory().unwrap();
        let mut r = Recorder::default();
        let edge_private = Info {
            key: r"c:\edge\msedge.exe".into(),
            app: "Microsoft Edge".into(),
            exe: "msedge".into(),
            detail: "Secret Page - [InPrivate] - Microsoft Edge".into(),
            pid: 100,
        };
        r.on_focus(&db, edge_private, 1_000).unwrap();
        let rows = db.list_activity(0, 100_000).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].app, "Microsoft Edge");
        assert!(rows[0].detail.is_empty());
        assert!(rows[0].url.is_empty());
    }
}
