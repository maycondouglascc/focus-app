//! Scheduled SQLite backup (`VACUUM INTO`, keep 7) and weekly integrity check (`PRAGMA quick_check`).

use std::fs;
use std::path::{Path, PathBuf};

use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN, THREAD_MODE_BACKGROUND_END,
};

use crate::crash::prune_old_files;
use crate::db::Db;
use crate::log::{self, Category};
use crate::time::civil_from_days;

pub const DAY_MS: i64 = 86_400_000;
pub const WEEK_MS: i64 = 7 * DAY_MS;
const MAX_BACKUPS: usize = 7;
const KEY_LAST_BACKUP: &str = "lastBackupAt";
const KEY_LAST_QUICK_CHECK: &str = "lastQuickCheckAt";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HousekeepingReport {
    pub backup_ran: bool,
    pub quick_check_failed: bool,
}

pub fn default_backup_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Focus").join("backups")
}

pub fn should_run_backup(last_backup_ms: i64, dirty_since_backup: bool, now_ms: i64) -> bool {
    dirty_since_backup && (last_backup_ms <= 0 || now_ms - last_backup_ms >= DAY_MS)
}

pub fn should_run_quick_check(last_check_ms: i64, now_ms: i64) -> bool {
    last_check_ms <= 0 || now_ms - last_check_ms >= WEEK_MS
}

fn format_backup_filename(now_ms: i64) -> String {
    let secs = now_ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hh = rem / 3_600;
    let mm = (rem % 3_600) / 60;
    let ss = rem % 60;
    format!("focus-{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}.db")
}

/// Executes `VACUUM INTO` inside `THREAD_MODE_BACKGROUND_BEGIN..END` so I/O stays low priority.
pub fn run_vacuum_into(db: &Db, dest: &Path) -> rusqlite::Result<()> {
    if let Some(dir) = dest.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::remove_file(dest);

    // SAFETY: temporarily enters low CPU+I/O background mode on the current worker thread and always restores.
    let entered =
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN).is_ok() };
    let res = db.vacuum_into(dest);
    if entered {
        // SAFETY: exits the `THREAD_MODE_BACKGROUND_BEGIN` scope entered on this thread above.
        unsafe {
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_END);
        }
    }
    res
}

pub fn run_housekeeping(
    db: &Db,
    backup_dir: &Path,
    dirty_since_backup: &mut bool,
    now_ms: i64,
) -> HousekeepingReport {
    let mut report = HousekeepingReport::default();

    let last_backup = db.get_setting_i64(KEY_LAST_BACKUP).unwrap_or(0);
    if should_run_backup(last_backup, *dirty_since_backup, now_ms) {
        let dest = backup_dir.join(format_backup_filename(now_ms));
        match run_vacuum_into(db, &dest) {
            Ok(()) => {
                *dirty_since_backup = false;
                let _ = db.set_setting_i64(KEY_LAST_BACKUP, now_ms);
                prune_old_files(backup_dir, "db", MAX_BACKUPS);
                log::event(Category::Backup, "backup_ok");
                report.backup_ran = true;
            }
            Err(e) => {
                log::db_err(Category::Backup, "backup_failed", &e);
            }
        }
    }

    let last_check = db.get_setting_i64(KEY_LAST_QUICK_CHECK).unwrap_or(0);
    if should_run_quick_check(last_check, now_ms) {
        match db.quick_check() {
            Ok(true) => {
                let _ = db.set_setting_i64(KEY_LAST_QUICK_CHECK, now_ms);
                log::event(Category::Backup, "quick_check_ok");
            }
            Ok(false) => {
                let _ = db.set_setting_i64(KEY_LAST_QUICK_CHECK, now_ms);
                log::event(Category::Backup, "quick_check_corrupt");
                report.quick_check_failed = true;
            }
            Err(e) => {
                log::db_err(Category::Backup, "quick_check_err", &e);
            }
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Activity;

    #[test]
    fn backup_schedule_and_rotation_and_quick_check() {
        assert!(!should_run_backup(0, false, DAY_MS));
        assert!(should_run_backup(0, true, DAY_MS));
        assert!(!should_run_backup(100_000, true, 100_000 + DAY_MS - 1));
        assert!(should_run_backup(100_000, true, 100_000 + DAY_MS));

        assert!(should_run_quick_check(0, 1000));
        assert!(!should_run_quick_check(1000, 1000 + WEEK_MS - 1));
        assert!(should_run_quick_check(1000, 1000 + WEEK_MS));

        let dir = std::env::temp_dir().join(format!("focus-backup-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("main.db");
        let backups = dir.join("backups");

        let db = Db::open(&db_path).unwrap();
        db.insert_activity(&Activity {
            id: 0,
            kind: "app".into(),
            app: "Code".into(),
            bundle_id: "code".into(),
            detail: "main.rs".into(),
            context: String::new(),
            url: String::new(),
            start: 10_000,
            end: 20_000,
        })
        .unwrap();

        let mut dirty = true;
        let t0 = 1_700_000_000_000i64;
        let rep1 = run_housekeeping(&db, &backups, &mut dirty, t0);
        assert!(rep1.backup_ran);
        assert!(!rep1.quick_check_failed);
        assert!(!dirty);

        // Skipping when clean even after 2 days.
        let rep2 = run_housekeeping(&db, &backups, &mut dirty, t0 + 2 * DAY_MS);
        assert!(!rep2.backup_ran);

        // Pre-seed 8 dummy backups + run one more to confirm pruning keeps 7 newest.
        for i in 1..=8 {
            fs::write(backups.join(format!("focus-2020010{i}-000000.db")), b"old").unwrap();
        }
        dirty = true;
        let rep3 = run_housekeeping(&db, &backups, &mut dirty, t0 + 3 * DAY_MS);
        assert!(rep3.backup_ran);
        let kept: Vec<_> = fs::read_dir(&backups)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("db"))
            .collect();
        assert_eq!(kept.len(), 7);

        drop(db);
        let _ = fs::remove_dir_all(&dir);
    }
}
