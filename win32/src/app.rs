//! Process lifecycle, single-instance coordination, and hidden tray window (`FocusTrayWindow`).
//!
//! `FocusTrayWindow` is a top-level hidden window (not `HWND_MESSAGE`, so it
//! receives broadcasts such as `TaskbarCreated` and `WM_ENDSESSION`). It stays
//! alive after the main Direct2D window closes; only `Focus.Quit` exits the loop.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HINSTANCE, HLOCAL, HWND, LPARAM,
    LRESULT, WPARAM,
};
use windows::Win32::Graphics::Gdi::UpdateWindow;
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateMutexW, GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::NIN_SELECT;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    FindWindowW, GetMessageW, GetWindowThreadProcessId, KillTimer, PostMessageW, PostQuitMessage,
    RegisterClassW, RegisterWindowMessageW, SendMessageTimeoutW, SetCoalescableTimer,
    TranslateMessage, MSG, SMTO_ABORTIFHUNG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WM_LBUTTONDBLCLK, WM_TIMER, WNDCLASSW,
};

use crate::claude;
use crate::db::Db;
use crate::db_thread::{DbHandle, DbThread};
use crate::locale::SystemLocale;
use crate::log::{self, Category};
use crate::monitor::{self, MonitorHandle};
use crate::qos;
use crate::tray::icon::IconState;
use crate::tray::Tray;
use crate::ui::{self, render, selftest::BgStore, strings, window};

pub const TRAY_WINDOW_CLASS: PCWSTR = w!("FocusTrayWindow");
pub const MSG_SHOW_NAME: PCWSTR = w!("Focus.Show");
pub const MSG_QUIT_NAME: PCWSTR = w!("Focus.Quit");

const WM_QUERYENDSESSION: u32 = 0x0011;
const WM_ENDSESSION: u32 = 0x0016;
const WM_APP_WINDOW_CLOSED: u32 = WM_APP + 30;
const WM_APP_TRAY_CALLBACK: u32 = WM_APP + 31;
/// Win32 `shellapi.h`: `NIN_KEYSELECT = NIN_SELECT | NINF_KEY (0x1)`.
const NIN_KEYSELECT: u32 = NIN_SELECT + 1;

const TRAY_TICK_TIMER_ID: usize = 1;
const TRAY_TICK_MS: u32 = 60_000;
const TRAY_TICK_TOLERANCE_MS: u32 = 10_000;

const TEST_INSTANCE_ENV: &str = "FOCUS_TEST_INSTANCE";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CliArgs {
    pub db: Option<PathBuf>,
    pub background: bool,
    pub quit: bool,
    pub version: bool,
    pub ui: bool,
    pub demo: bool,
    pub fresh: bool,
    pub screen: Option<String>,
    pub diag_drag: bool,
    pub ui_cycle: Option<u32>,
    pub all_screens: bool,
    pub ui_show: Option<u64>,
    pub gallery: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAction {
    PrintVersion,
    DiagDrag,
    UiCycle {
        count: u32,
        all_screens: bool,
    },
    UiShow {
        seconds: u64,
        gallery: bool,
        demo: bool,
        fresh: bool,
        db: Option<PathBuf>,
        screen: Option<String>,
    },
    Demo {
        fresh: bool,
        screen: Option<String>,
    },
    StandaloneUi {
        db: PathBuf,
        screen: Option<String>,
    },
    SendShow,
    SendQuit,
    ExitSilently,
    Run {
        db: PathBuf,
        open_window: bool,
    },
}

pub fn default_db_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Focus").join("focus.db")
}

pub fn parse_args(args: &[String]) -> CliArgs {
    let mut out = CliArgs::default();
    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--db" => {
                if let Some(p) = args.get(i + 1) {
                    out.db = Some(PathBuf::from(p));
                    i += 1;
                }
            }
            "--screen" => {
                if let Some(s) = args.get(i + 1) {
                    out.screen = Some(s.clone());
                    i += 1;
                }
            }
            "--background" => out.background = true,
            "--quit" => out.quit = true,
            "--version" => out.version = true,
            "--ui" => out.ui = true,
            "--demo" => out.demo = true,
            "--fresh" => out.fresh = true,
            "--diag-drag" => out.diag_drag = true,
            "--all-screens" => out.all_screens = true,
            "--gallery" => out.gallery = true,
            "--ui-cycle" => {
                let n = args
                    .get(i + 1)
                    .and_then(|s| s.parse::<u32>().ok())
                    .inspect(|_| i += 1)
                    .unwrap_or(50);
                out.ui_cycle = Some(n);
            }
            "--ui-show" => {
                let n = args
                    .get(i + 1)
                    .and_then(|s| s.parse::<u64>().ok())
                    .inspect(|_| i += 1)
                    .unwrap_or(10);
                out.ui_show = Some(n);
            }
            _ => {}
        }
        i += 1;
    }
    out
}

pub fn decide_action(already_running: bool, args: &CliArgs) -> AppAction {
    if args.version {
        return AppAction::PrintVersion;
    }
    if args.diag_drag {
        return AppAction::DiagDrag;
    }
    if let Some(count) = args.ui_cycle {
        return AppAction::UiCycle {
            count,
            all_screens: args.all_screens,
        };
    }
    if let Some(seconds) = args.ui_show {
        return AppAction::UiShow {
            seconds,
            gallery: args.gallery,
            demo: args.demo,
            fresh: args.fresh,
            db: args.db.clone(),
            screen: args.screen.clone(),
        };
    }
    if args.demo {
        return AppAction::Demo {
            fresh: args.fresh,
            screen: args.screen.clone(),
        };
    }
    if args.ui {
        return AppAction::StandaloneUi {
            db: args.db.clone().unwrap_or_else(default_db_path),
            screen: args.screen.clone(),
        };
    }
    if args.quit {
        return if already_running {
            AppAction::SendQuit
        } else {
            AppAction::ExitSilently
        };
    }
    if already_running {
        return if args.background {
            AppAction::ExitSilently
        } else {
            AppAction::SendShow
        };
    }
    AppAction::Run {
        db: args.db.clone().unwrap_or_else(default_db_path),
        open_window: !args.background,
    }
}

pub fn needs_single_instance(args: &CliArgs) -> bool {
    !args.version
        && !args.diag_drag
        && args.ui_cycle.is_none()
        && args.ui_show.is_none()
        && !args.demo
        && !args.ui
}

pub fn mutex_name_for_sid(sid: &str, instance_tag: Option<&str>) -> String {
    match instance_tag {
        Some(tag) if !tag.is_empty() => format!("Local\\Focus-{sid}-{tag}"),
        _ => format!("Local\\Focus-{sid}"),
    }
}

fn tray_window_title(instance_tag: Option<&str>) -> String {
    match instance_tag {
        Some(tag) if !tag.is_empty() => format!("Focus-{tag}"),
        _ => "Focus".into(),
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn current_user_sid() -> String {
    // SAFETY: queries the current process token for its user SID and frees the Win32-allocated string buffer.
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return "unknown".into();
        }
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
        if needed == 0 {
            let _ = CloseHandle(token);
            return "unknown".into();
        }
        let mut buf = vec![0u8; needed as usize];
        let ok = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            needed,
            &mut needed,
        );
        let _ = CloseHandle(token);
        if ok.is_err() {
            return "unknown".into();
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut sid_str = PWSTR::null();
        if ConvertSidToStringSidW(user.User.Sid, &mut sid_str).is_err() {
            return "unknown".into();
        }
        let res = sid_str.to_string().unwrap_or_else(|_| "unknown".into());
        let _ = windows::Win32::Foundation::LocalFree(Some(HLOCAL(sid_str.0.cast())));
        res
    }
}

pub struct InstanceMutex {
    handle: HANDLE,
}

impl InstanceMutex {
    pub fn acquire(instance_tag: Option<&str>) -> Result<(Self, bool), String> {
        let sid = current_user_sid();
        let name = mutex_name_for_sid(&sid, instance_tag);
        Self::acquire_named(&name)
    }

    pub fn acquire_named(name: &str) -> Result<(Self, bool), String> {
        let wide = to_wide(name);
        // SAFETY: `wide` is NUL-terminated; `GetLastError` immediately checks `ERROR_ALREADY_EXISTS` on the same thread.
        unsafe {
            let handle = CreateMutexW(None, false, PCWSTR(wide.as_ptr()))
                .map_err(|e| format!("CreateMutexW failed: {e}"))?;
            let already = GetLastError() == ERROR_ALREADY_EXISTS;
            Ok((Self { handle }, already))
        }
    }
}

impl Drop for InstanceMutex {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            // SAFETY: `self.handle` is an owned mutex handle from `CreateMutexW`.
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

pub fn test_instance_tag() -> Option<String> {
    std::env::var(TEST_INSTANCE_ENV)
        .ok()
        .filter(|s| !s.is_empty())
}

fn find_tray_window(instance_tag: Option<&str>) -> Option<HWND> {
    let title_wide = instance_tag.map(|tag| to_wide(&tray_window_title(Some(tag))));
    let title_ptr = title_wide
        .as_ref()
        .map_or(PCWSTR::null(), |w| PCWSTR(w.as_ptr()));
    // Wait briefly in case the primary instance acquired the mutex and is creating `FocusTrayWindow`.
    for _ in 0..100 {
        // SAFETY: `TRAY_WINDOW_CLASS` and optional `title_ptr` are valid NUL-terminated UTF-16 strings.
        if let Ok(hwnd) = unsafe { FindWindowW(TRAY_WINDOW_CLASS, title_ptr) } {
            if !hwnd.0.is_null() {
                return Some(hwnd);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

pub fn send_show(instance_tag: Option<&str>) -> i32 {
    let Some(hwnd) = find_tray_window(instance_tag) else {
        return 1;
    };
    // SAFETY: `MSG_SHOW_NAME` is a static wide literal; `hwnd` was returned by `FindWindowW`.
    unsafe {
        let msg = RegisterWindowMessageW(MSG_SHOW_NAME);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != 0 {
            let _ = AllowSetForegroundWindow(pid);
        }
        if SendMessageTimeoutW(
            hwnd,
            msg,
            WPARAM(0),
            LPARAM(0),
            SMTO_ABORTIFHUNG,
            2_000,
            None,
        )
        .0 == 0
        {
            let _ = PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0));
        }
    }
    0
}

pub fn send_quit(instance_tag: Option<&str>) -> i32 {
    let Some(hwnd) = find_tray_window(instance_tag) else {
        return 1;
    };
    // SAFETY: `MSG_QUIT_NAME` is a static wide literal; `PostMessageW` asks the primary instance to quit cleanly.
    unsafe {
        let msg = RegisterWindowMessageW(MSG_QUIT_NAME);
        let _ = PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0));
    }
    0
}

struct AppState {
    db_path: PathBuf,
    tray_hwnd: HWND,
    msg_show: u32,
    msg_quit: u32,
    db_handle: DbHandle,
    db_thread: Option<DbThread>,
    monitor: Option<MonitorHandle>,
    tray: Option<Tray>,
    tray_tick_armed: bool,
    quitting: bool,
}

thread_local! {
    static APP: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

fn system_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn compute_tray_presentation(db_path: &Path, db_handle: &DbHandle) -> (IconState, String, bool) {
    if db_handle.is_in_error() {
        return (IconState::Error, strings::TRAY_TIP_ERROR.to_string(), false);
    }
    let Ok(db) = Db::open_readonly(db_path) else {
        return (
            IconState::Stopped,
            strings::TRAY_TIP_IDLE.to_string(),
            false,
        );
    };
    let locale = SystemLocale::query();
    let paused = db
        .load_settings(&locale)
        .map(|s| !s.monitor_enabled)
        .unwrap_or(false);
    if let Ok(Some(running)) = db.running_entry() {
        let elapsed_ms = (system_now_ms() - running.start).max(0);
        let mins = (elapsed_ms / 60_000) as u64;
        let progress_steps = (mins % 60) as u8;
        let elapsed_str = if mins >= 60 {
            format!("{}h {:02}m", mins / 60, mins % 60)
        } else {
            format!("{}m", mins)
        };
        let label = running.description.trim();
        let tip = if label.is_empty() {
            format!("Focus — {elapsed_str}")
        } else {
            format!("Focus — {elapsed_str}: {label}")
        };
        return (IconState::Running { progress_steps }, tip, true);
    }
    if paused {
        (
            IconState::Paused,
            strings::TRAY_TIP_PAUSED.to_string(),
            false,
        )
    } else {
        (
            IconState::Stopped,
            strings::TRAY_TIP_IDLE.to_string(),
            false,
        )
    }
}

fn refresh_tray_state() {
    APP.with(|a| {
        let mut borrow = a.borrow_mut();
        let Some(st) = borrow.as_mut() else {
            return;
        };
        let (state, tip, timer_running) = compute_tray_presentation(&st.db_path, &st.db_handle);
        if let Some(ref mut tray) = st.tray {
            tray.set_state(state, &tip);
        }
        if timer_running && !st.tray_tick_armed {
            // SAFETY: `st.tray_hwnd` is owned by this UI thread; coalescable timer runs only while timer ticks.
            let id = unsafe {
                SetCoalescableTimer(
                    Some(st.tray_hwnd),
                    TRAY_TICK_TIMER_ID,
                    TRAY_TICK_MS,
                    None,
                    TRAY_TICK_TOLERANCE_MS,
                )
            };
            st.tray_tick_armed = id != 0;
        } else if !timer_running && st.tray_tick_armed {
            // SAFETY: `st.tray_hwnd` is owned by this UI thread.
            unsafe {
                let _ = KillTimer(Some(st.tray_hwnd), TRAY_TICK_TIMER_ID);
            }
            st.tray_tick_armed = false;
        }
    });
}

fn open_main_window() {
    let t0 = Instant::now();
    if window::is_open() {
        qos::set_background_qos(false);
        let _ = window::open();
        return;
    }
    let Some((db_path, db_handle)) = APP.with(|a| {
        a.borrow()
            .as_ref()
            .map(|s| (s.db_path.clone(), s.db_handle.clone()))
    }) else {
        return;
    };
    qos::set_background_qos(false);
    ui::init();
    render::warmup_d3d();
    let bg = BgStore::spawn_db_with_writer(&db_path, db_handle);
    if !window::open() {
        bg.abort();
        render::reset_shared();
        qos::set_background_qos(true);
        return;
    }
    match bg.finish_shell(None) {
        Ok(Some(screen)) => {
            if !window::set_screen(screen) {
                window::close();
                render::reset_shared();
                qos::set_background_qos(true);
                return;
            }
        }
        Ok(None) | Err(_) => {
            window::close();
            render::reset_shared();
            qos::set_background_qos(true);
            return;
        }
    }
    if let Some(hwnd) = window::hwnd() {
        // SAFETY: `hwnd` is our thread-owned main window created above.
        unsafe {
            let _ = UpdateWindow(hwnd);
        }
    }
    let first_frame_ms = t0.elapsed().as_secs_f64() * 1000.0;
    log::metric_ms(Category::Perf, "open", "first_frame_ms", first_frame_ms);
}

unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let (msg_show, msg_quit) = APP.with(|a| {
        a.borrow()
            .as_ref()
            .map_or((0, 0), |s| (s.msg_show, s.msg_quit))
    });
    match msg {
        m if msg_show != 0 && m == msg_show => {
            open_main_window();
            return LRESULT(0);
        }
        m if msg_quit != 0 && m == msg_quit => {
            APP.with(|a| {
                if let Some(st) = a.borrow_mut().as_mut() {
                    st.quitting = true;
                }
            });
            // SAFETY: called on the UI thread owning `FocusTrayWindow`'s `GetMessageW` loop.
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        m if m == WM_APP_TRAY_CALLBACK => {
            let event = (lparam.0 as usize & 0xFFFF) as u32;
            if event == NIN_SELECT || event == NIN_KEYSELECT || event == WM_LBUTTONDBLCLK {
                open_main_window();
            }
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TRAY_TICK_TIMER_ID => {
            refresh_tray_state();
            return LRESULT(0);
        }
        m if m == WM_APP_WINDOW_CLOSED => {
            let quitting = APP.with(|a| a.borrow().as_ref().is_some_and(|s| s.quitting));
            if !quitting {
                qos::set_background_qos(true);
                refresh_tray_state();
                log::event(Category::App, "window_closed");
            }
            return LRESULT(0);
        }
        WM_QUERYENDSESSION => return LRESULT(1),
        WM_ENDSESSION if wparam.0 != 0 => {
            APP.with(|a| {
                if let Some(st) = a.borrow().as_ref() {
                    if let Some(ref mon) = st.monitor {
                        mon.end_session_and_wait(Duration::from_millis(3_000));
                    }
                }
            });
            return LRESULT(0);
        }
        _ => {}
    }
    let handled = APP.with(|a| {
        a.borrow_mut()
            .as_mut()
            .and_then(|s| s.tray.as_mut())
            .is_some_and(|tray| tray.handle_env_message(msg, lparam))
    });
    if handled {
        return LRESULT(0);
    }
    // SAFETY: delegates unhandled messages to Win32 default window procedure.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

unsafe fn create_tray_window(instance_tag: Option<&str>) -> Option<HWND> {
    // SAFETY: called on the UI thread; class registration and window creation use valid static/UTF-16 strings.
    unsafe {
        let hinstance = HINSTANCE(GetModuleHandleW(None).ok()?.0);
        let wc = WNDCLASSW {
            lpfnWndProc: Some(tray_wnd_proc),
            hInstance: hinstance,
            lpszClassName: TRAY_WINDOW_CLASS,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let title = to_wide(&tray_window_title(instance_tag));
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            TRAY_WINDOW_CLASS,
            PCWSTR(title.as_ptr()),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinstance),
            None,
        )
        .ok()
    }
}

pub fn run_app(
    db_path: PathBuf,
    open_window: bool,
    instance_tag: Option<&str>,
    _mutex: InstanceMutex,
) -> i32 {
    log::init_default();

    // SAFETY: creates `FocusTrayWindow`, registers inter-instance messages, and runs the UI message pump.
    let (tray_hwnd, msg_show, msg_quit) = unsafe {
        let Some(hwnd) = create_tray_window(instance_tag) else {
            log::event(Category::App, "tray_window_failed");
            return 1;
        };
        (
            hwnd,
            RegisterWindowMessageW(MSG_SHOW_NAME),
            RegisterWindowMessageW(MSG_QUIT_NAME),
        )
    };
    window::set_close_notify(tray_hwnd, WM_APP_WINDOW_CLOSED);

    // Startup order: single-instance mutex -> tray icon -> DB writer -> monitor.
    let tray = Tray::add(tray_hwnd, WM_APP_TRAY_CALLBACK, strings::TRAY_TIP_IDLE);

    let mut db_thread = match DbThread::spawn(db_path.clone()) {
        Ok(t) => t,
        Err(e) => {
            log::event(Category::App, "db_start_failed");
            eprintln!("focus: cannot open database at {}: {e}", db_path.display());
            drop(tray);
            // SAFETY: `tray_hwnd` was created on this thread above.
            unsafe {
                let _ = DestroyWindow(tray_hwnd);
            }
            return 1;
        }
    };
    let db_handle = db_thread.handle();

    let monitor = match monitor::spawn(db_path.clone(), db_handle.clone()) {
        Ok(m) => m,
        Err(e) => {
            log::event(Category::App, "monitor_start_failed");
            eprintln!("focus: cannot open database at {}: {e}", db_path.display());
            db_thread.stop();
            drop(tray);
            // SAFETY: `tray_hwnd` was created on this thread above.
            unsafe {
                let _ = DestroyWindow(tray_hwnd);
            }
            return 1;
        }
    };
    claude::spawn(db_path.clone(), db_handle.clone());

    APP.with(|a| {
        *a.borrow_mut() = Some(AppState {
            db_path,
            tray_hwnd,
            msg_show,
            msg_quit,
            db_handle,
            db_thread: Some(db_thread),
            monitor: Some(monitor),
            tray: Some(tray),
            tray_tick_armed: false,
            quitting: false,
        });
    });
    refresh_tray_state();

    if open_window {
        open_main_window();
    } else {
        qos::set_background_qos(true);
    }

    // SAFETY: standard Win32 message loop for the UI thread; keeps running when `FocusMainWindow` closes.
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    if window::is_open() {
        window::close();
    }
    let state = APP.with(|a| a.borrow_mut().take());
    if let Some(mut st) = state {
        st.quitting = true;
        if st.tray_tick_armed {
            // SAFETY: `st.tray_hwnd` is owned by this UI thread.
            unsafe {
                let _ = KillTimer(Some(st.tray_hwnd), TRAY_TICK_TIMER_ID);
            }
        }
        if let Some(mut tray) = st.tray.take() {
            tray.remove();
        }
        if let Some(mut mon) = st.monitor.take() {
            mon.stop();
        }
        if let Some(mut db) = st.db_thread.take() {
            db.stop();
        }
        // SAFETY: `st.tray_hwnd` is owned by this thread and destroyed before process exit.
        unsafe {
            let _ = DestroyWindow(st.tray_hwnd);
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn parse_args_and_route_table() {
        let empty = parse_args(&s(&["focus.exe"]));
        assert!(needs_single_instance(&empty));
        assert_eq!(
            decide_action(false, &empty),
            AppAction::Run {
                db: default_db_path(),
                open_window: true,
            }
        );
        assert_eq!(decide_action(true, &empty), AppAction::SendShow);

        let bg = parse_args(&s(&["focus.exe", "--db", "tmp.db", "--background"]));
        assert!(needs_single_instance(&bg));
        assert_eq!(
            decide_action(false, &bg),
            AppAction::Run {
                db: PathBuf::from("tmp.db"),
                open_window: false,
            }
        );
        assert_eq!(decide_action(true, &bg), AppAction::ExitSilently);

        let quit = parse_args(&s(&["focus.exe", "--quit"]));
        assert!(needs_single_instance(&quit));
        assert_eq!(decide_action(false, &quit), AppAction::ExitSilently);
        assert_eq!(decide_action(true, &quit), AppAction::SendQuit);

        let ver = parse_args(&s(&["focus.exe", "--version"]));
        assert!(!needs_single_instance(&ver));
        assert_eq!(decide_action(true, &ver), AppAction::PrintVersion);

        let diag = parse_args(&s(&["focus.exe", "--diag-drag"]));
        assert!(!needs_single_instance(&diag));
        assert_eq!(decide_action(true, &diag), AppAction::DiagDrag);

        let demo = parse_args(&s(&[
            "focus.exe",
            "--demo",
            "--fresh",
            "--screen",
            "welcome",
        ]));
        assert!(!needs_single_instance(&demo));
        assert_eq!(
            decide_action(true, &demo),
            AppAction::Demo {
                fresh: true,
                screen: Some("welcome".into()),
            }
        );

        let cycle = parse_args(&s(&["focus.exe", "--ui-cycle", "25", "--all-screens"]));
        assert!(!needs_single_instance(&cycle));
        assert_eq!(
            decide_action(false, &cycle),
            AppAction::UiCycle {
                count: 25,
                all_screens: true,
            }
        );
    }

    #[test]
    fn single_instance_mutex_reports_second_holder_until_dropped() {
        let name = format!("Local\\Focus-unit-mutex-{}", std::process::id());
        let (first, already1) = InstanceMutex::acquire_named(&name).unwrap();
        assert!(!already1);

        let (second, already2) = InstanceMutex::acquire_named(&name).unwrap();
        assert!(already2);
        drop(second);
        drop(first);

        let (_third, already3) = InstanceMutex::acquire_named(&name).unwrap();
        assert!(!already3);
    }
}
