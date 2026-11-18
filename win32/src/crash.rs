//! Unattended crash minidumps (`%LOCALAPPDATA%\Focus\crashes\focus-*.dmp`)
//! and privacy-safe panic hook.
//!
//! `dbghelp.dll` is delay-loaded in `build.rs` so normal execution never loads
//! it into memory. Keeps at most the 5 newest minidump files.

use std::fs;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use windows::core::BOOL;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::Debug::{
    MiniDumpNormal, MiniDumpWriteDump, SetUnhandledExceptionFilter, EXCEPTION_POINTERS,
    MINIDUMP_EXCEPTION_INFORMATION,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId,
};

use crate::log::{self, Category};
use crate::time::civil_from_days;

const MAX_DUMPS: usize = 5;
const TEST_CRASH_DIR_ENV: &str = "FOCUS_TEST_CRASH_DIR";

static CRASH_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn default_crash_dir() -> PathBuf {
    if let Some(custom) = std::env::var_os(TEST_CRASH_DIR_ENV) {
        if !custom.is_empty() {
            return PathBuf::from(custom);
        }
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Focus").join("crashes")
}

fn format_utc_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hh = rem / 3_600;
    let mm = (rem % 3_600) / 60;
    let ss = rem % 60;
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}")
}

/// Keeps only the `keep` newest files matching `.ext` (by descending filename / timestamp).
pub fn prune_old_files(dir: &Path, ext: &str, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(String, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|s| s.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        })
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().into_owned();
            Some((name, p))
        })
        .collect();
    if files.len() <= keep {
        return;
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in files.into_iter().skip(keep) {
        let _ = fs::remove_file(path);
    }
}

unsafe extern "system" fn unhandled_filter(ep: *const EXCEPTION_POINTERS) -> i32 {
    let Some(dir) = CRASH_DIR.get() else {
        return 0; // EXCEPTION_CONTINUE_SEARCH
    };
    let _ = fs::create_dir_all(dir);
    // SAFETY: queries current process ID only.
    let pid = unsafe { GetCurrentProcessId() };
    let path = dir.join(format!("focus-{}-{pid}.dmp", format_utc_stamp()));
    if let Ok(file) = fs::File::create(&path) {
        let mut mei = MINIDUMP_EXCEPTION_INFORMATION {
            // SAFETY: queries current thread ID only.
            ThreadId: unsafe { GetCurrentThreadId() },
            ExceptionPointers: ep as *mut EXCEPTION_POINTERS,
            ClientPointers: BOOL(0),
        };
        let exc_ptr = if ep.is_null() {
            None
        } else {
            Some(&mei as *const MINIDUMP_EXCEPTION_INFORMATION)
        };
        // SAFETY: `file` handle is owned and open for writing; delay-loaded `dbghelp.dll` writes minidump.
        unsafe {
            let _ = MiniDumpWriteDump(
                GetCurrentProcess(),
                pid,
                HANDLE(file.as_raw_handle()),
                MiniDumpNormal,
                exc_ptr,
                None,
                None,
            );
        }
        let _ = &mut mei;
    }
    prune_old_files(dir, "dmp", MAX_DUMPS);
    log::event(Category::Crash, "minidump_written");
    1 // EXCEPTION_EXECUTE_HANDLER: exit immediately without blocking in WerFault UI
}

/// Installs the Windows unhandled exception minidump filter and privacy-safe panic hook.
pub fn install() {
    let dir = default_crash_dir();
    let _ = fs::create_dir_all(&dir);
    let _ = CRASH_DIR.set(dir);

    std::panic::set_hook(Box::new(|_| {
        // Log only static markers so window titles/URLs can never leak via panic payloads.
        log::event(Category::Crash, "panic");
    }));

    // SAFETY: registers our static process-wide unhandled exception filter.
    unsafe {
        SetUnhandledExceptionFilter(Some(unhandled_filter));
    }
}

/// Triggers an intentional null-pointer write for crash-dump verification (`--crash-test`).
pub fn trigger_crash_test() -> ! {
    install();
    // SAFETY: intentional null write requested exclusively by hidden `--crash-test` flag.
    unsafe {
        std::ptr::write_volatile(std::ptr::null_mut::<u8>(), 1);
    }
    std::process::abort()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_old_files_keeps_five_newest_dumps() {
        let dir = std::env::temp_dir().join(format!("focus-crash-prune-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let names = [
            "focus-20261110-100000-1.dmp",
            "focus-20261111-100000-1.dmp",
            "focus-20261112-100000-1.dmp",
            "focus-20261113-100000-1.dmp",
            "focus-20261114-100000-1.dmp",
            "focus-20261115-100000-1.dmp",
            "focus-20261116-100000-1.dmp",
            "keep-unrelated.txt",
        ];
        for n in names {
            fs::write(dir.join(n), b"x").unwrap();
        }
        prune_old_files(&dir, "dmp", 5);

        assert!(!dir.join("focus-20261110-100000-1.dmp").exists());
        assert!(!dir.join("focus-20261111-100000-1.dmp").exists());
        assert!(dir.join("focus-20261112-100000-1.dmp").exists());
        assert!(dir.join("focus-20261116-100000-1.dmp").exists());
        assert!(dir.join("keep-unrelated.txt").exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
