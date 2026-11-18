//! Focus: event-driven foreground tracking + native Win32 UI.
//!
//! Usage: focus.exe [--db <path>]   (default %LOCALAPPDATA%\Focus\focus.db)
#![windows_subsystem = "console"]

mod appinfo;
mod claude;
mod csv_export;
mod db;
mod demo;
mod format;
mod locale;
mod log;
mod monitor;
mod presence;
mod recorder;
mod stats;
mod time;
mod ui;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PostThreadMessageW, TranslateMessage, MSG, WM_QUIT,
};

static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);

fn default_db_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Focus").join("focus.db")
}

fn db_path_from_args(args: &[String]) -> PathBuf {
    arg_value(args, "--db")
        .map(PathBuf::from)
        .unwrap_or_else(default_db_path)
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Ctrl+C / console close: ask the message loop to quit so the run is saved.
unsafe extern "system" fn on_ctrl(_kind: u32) -> BOOL {
    let tid = MAIN_THREAD.load(Ordering::SeqCst);
    if tid != 0 {
        // SAFETY: `tid` is the main thread's message loop ID; posting `WM_QUIT` triggers clean shutdown.
        unsafe {
            let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
    BOOL(1)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let screen = arg_value(&args, "--screen");
    let db_override = arg_value(&args, "--db").map(PathBuf::from);
    let demo = has("--demo");
    let fresh = has("--fresh");

    if has("--diag-drag") {
        std::process::exit(ui::selftest::diag_drag());
    }
    if let Some(i) = args.iter().position(|a| a == "--ui-cycle") {
        let n = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(50);
        let all_screens = has("--all-screens");
        std::process::exit(ui::selftest::cycle(n, all_screens));
    }
    if let Some(i) = args.iter().position(|a| a == "--ui-show") {
        let n = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(10);
        let gallery = has("--gallery");
        std::process::exit(ui::selftest::show(
            n,
            gallery,
            demo,
            fresh,
            db_override.as_deref(),
            screen.as_deref(),
        ));
    }
    if demo {
        std::process::exit(ui::selftest::run_demo(fresh, screen.as_deref()));
    }
    if has("--ui") {
        let db_p = db_override.unwrap_or_else(default_db_path);
        std::process::exit(ui::selftest::run_db(&db_p, screen.as_deref()));
    }

    log::init_default();
    let path = db_path_from_args(&args);
    let mut mon = match monitor::spawn(path.clone()) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("focus: cannot open database at {}: {e}", path.display());
            std::process::exit(1);
        }
    };
    claude::spawn(path.clone());

    // SAFETY: registers Ctrl+C handler and runs the thread message pump until `WM_QUIT`.
    unsafe {
        MAIN_THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
        let _ = SetConsoleCtrlHandler(Some(on_ctrl), true);
        eprintln!("focus: recording to {} (Ctrl+C to quit)", path.display());
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    mon.stop();
}
