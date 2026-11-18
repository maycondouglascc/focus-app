//! Focus: event-driven foreground tracking + native Win32 UI.
#![windows_subsystem = "windows"]

mod app;
mod appinfo;
mod backup;
mod claude;
mod crash;
mod csv_export;
mod db;
mod db_thread;
mod demo;
mod format;
mod locale;
mod log;
mod monitor;
mod presence;
mod qos;
mod recorder;
mod startup;
mod stats;
mod time;
mod tray;
mod ui;
mod url;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{GetFileType, FILE_TYPE_UNKNOWN};
use windows::Win32::System::Console::{
    AttachConsole, GetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
};

use app::{
    decide_action, needs_single_instance, parse_args, run_app, send_quit, send_show,
    test_instance_tag, AppAction, InstanceMutex,
};

fn attach_parent_console_if_needed() {
    let has_valid = |kind| {
        // SAFETY: queries standard handle validity without mutating state.
        unsafe {
            GetStdHandle(kind)
                .ok()
                .filter(|h: &HANDLE| !h.is_invalid() && !h.0.is_null())
                .is_some_and(|h| GetFileType(h) != FILE_TYPE_UNKNOWN)
        }
    };
    if !has_valid(STD_OUTPUT_HANDLE) && !has_valid(STD_ERROR_HANDLE) {
        // SAFETY: attaches to parent terminal only when no stdout/stderr pipe or file handle exists.
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

fn main() {
    crash::install();
    let raw_args: Vec<String> = std::env::args().collect();
    let parsed = parse_args(&raw_args);
    let tag = test_instance_tag();

    if !needs_single_instance(&parsed) {
        attach_parent_console_if_needed();
        let code = match decide_action(false, &parsed) {
            AppAction::PrintVersion => {
                println!("focus {}", env!("CARGO_PKG_VERSION"));
                0
            }
            AppAction::CrashTest => crash::trigger_crash_test(),
            AppAction::DiagDrag => ui::selftest::diag_drag(),
            AppAction::UiCycle { count, all_screens } => ui::selftest::cycle(count, all_screens),
            AppAction::TrayCycle(count) => ui::selftest::tray_cycle(count),
            AppAction::UiShow {
                seconds,
                gallery,
                demo,
                fresh,
                db,
                screen,
            } => ui::selftest::show(
                seconds,
                gallery,
                demo,
                fresh,
                db.as_deref(),
                screen.as_deref(),
            ),
            AppAction::Demo { fresh, screen } => ui::selftest::run_demo(fresh, screen.as_deref()),
            AppAction::StandaloneUi { db, screen } => ui::selftest::run_db(&db, screen.as_deref()),
            _ => 0,
        };
        std::process::exit(code);
    }

    let (mutex, already_running) = match InstanceMutex::acquire(tag.as_deref()) {
        Ok(res) => res,
        Err(e) => {
            log::init_default();
            log::event(log::Category::App, "mutex_acquire_failed");
            attach_parent_console_if_needed();
            eprintln!("focus: {e}");
            std::process::exit(1);
        }
    };

    let code = match decide_action(already_running, &parsed) {
        AppAction::SendShow => send_show(tag.as_deref()),
        AppAction::SendQuit => send_quit(tag.as_deref()),
        AppAction::ExitSilently => 0,
        AppAction::Run { db, open_window } => run_app(db, open_window, tag.as_deref(), mutex),
        _ => 0,
    };
    std::process::exit(code);
}
