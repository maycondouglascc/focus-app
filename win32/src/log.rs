//! Privacy-safe rotating file logger (`%LOCALAPPDATA%\Focus\logs\focus.log`).
//!
//! Rotates at 1 MB keeping one backup (`focus.1.log`). The public API only
//! accepts static category/operation literals and numeric/boolean values, so
//! window titles, URLs, and app names can never be written to disk.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LOG_BYTES: u64 = 1_048_576;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    App,
    Monitor,
    Db,
    Claude,
    Url,
    Tray,
    Startup,
    Crash,
    Backup,
    Perf,
}

impl Category {
    const fn as_str(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Monitor => "monitor",
            Self::Db => "db",
            Self::Claude => "claude",
            Self::Url => "url",
            Self::Tray => "tray",
            Self::Startup => "startup",
            Self::Crash => "crash",
            Self::Backup => "backup",
            Self::Perf => "perf",
        }
    }
}

pub struct LogFile {
    path: PathBuf,
    backup_path: PathBuf,
    max_bytes: u64,
}

impl LogFile {
    pub fn new(path: PathBuf, max_bytes: u64) -> Self {
        let backup_path = backup_path_for(&path);
        Self {
            path,
            backup_path,
            max_bytes: max_bytes.max(1),
        }
    }

    pub fn write_line(&self, line: &str) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(meta) = fs::metadata(&self.path) {
            if meta.len() >= self.max_bytes {
                let _ = fs::remove_file(&self.backup_path);
                let _ = fs::rename(&self.path, &self.backup_path);
            }
        }
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

fn backup_path_for(path: &Path) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("focus");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("log");
    let file_name = format!("{stem}.1.{ext}");
    match path.parent() {
        Some(p) => p.join(file_name),
        None => PathBuf::from(file_name),
    }
}

static LOGGER: OnceLock<Mutex<LogFile>> = OnceLock::new();

pub fn default_log_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Focus").join("logs")
}

pub fn default_log_path() -> PathBuf {
    default_log_dir().join("focus.log")
}

pub fn init_default() {
    init_at(default_log_path());
}

pub fn init_at(path: PathBuf) {
    let _ = LOGGER.get_or_init(|| Mutex::new(LogFile::new(path, MAX_LOG_BYTES)));
}

fn format_utc_stamp(now: SystemTime) -> String {
    let ms = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let secs = ms.div_euclid(1000);
    let rem_ms = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let day_secs = secs.rem_euclid(86_400);
    let (y, m, d) = crate::time::civil_from_days(days);
    let h = day_secs / 3600;
    let mi = (day_secs % 3600) / 60;
    let s = day_secs % 60;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{rem_ms:03}Z")
}

fn emit(line: String) {
    if let Some(lock) = LOGGER.get() {
        if let Ok(guard) = lock.lock() {
            guard.write_line(&line);
        }
    }
}

pub fn event(cat: Category, op: &'static str) {
    let stamp = format_utc_stamp(SystemTime::now());
    emit(format!("{stamp} [{}] {op}", cat.as_str()));
}

#[allow(dead_code)]
pub fn code(cat: Category, op: &'static str, code: i64) {
    let stamp = format_utc_stamp(SystemTime::now());
    emit(format!("{stamp} [{}] {op} code={code}", cat.as_str()));
}

#[allow(dead_code)]
pub fn flag(cat: Category, op: &'static str, field: &'static str, value: bool) {
    let stamp = format_utc_stamp(SystemTime::now());
    emit(format!("{stamp} [{}] {op} {field}={value}", cat.as_str()));
}

#[allow(dead_code)]
pub fn metric_ms(cat: Category, op: &'static str, field: &'static str, ms: f64) {
    let stamp = format_utc_stamp(SystemTime::now());
    emit(format!("{stamp} [{}] {op} {field}={ms:.1}", cat.as_str()));
}

pub fn db_err(cat: Category, op: &'static str, err: &rusqlite::Error) {
    let stamp = format_utc_stamp(SystemTime::now());
    let (kind, ext_code) = match err {
        rusqlite::Error::SqliteFailure(f, _) => ("sqlite", f.extended_code),
        rusqlite::Error::QueryReturnedNoRows => ("no_rows", 0),
        rusqlite::Error::InvalidParameterName(_) => ("invalid_param", 0),
        _ => ("other", 0),
    };
    emit(format!(
        "{stamp} [{}] {op} err={kind} sqlite_code={ext_code}",
        cat.as_str()
    ));
}

#[allow(dead_code)]
pub fn open_log_reader_hint() -> Option<File> {
    File::open(default_log_path()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_at_threshold_and_keeps_one_backup() {
        let dir = std::env::temp_dir().join(format!("focus-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let log_path = dir.join("focus.log");
        let lf = LogFile::new(log_path.clone(), 120);

        for i in 0..8 {
            lf.write_line(&format!(
                "2026-11-18T10:00:0{i}.000Z [monitor] tick code={i}"
            ));
        }

        assert!(log_path.exists());
        assert!(dir.join("focus.1.log").exists());
        let active = fs::read_to_string(&log_path).unwrap();
        let rotated = fs::read_to_string(dir.join("focus.1.log")).unwrap();
        assert!(!active.is_empty());
        assert!(!rotated.is_empty());
        assert!(active.len() < 200);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn formats_utc_timestamp_cleanly() {
        let t = UNIX_EPOCH + std::time::Duration::from_millis(1_794_842_539_123);
        assert_eq!(format_utc_stamp(t), "2026-11-16T15:22:19.123Z");
    }
}
