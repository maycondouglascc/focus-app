//! Focus monitor: event-driven foreground tracking written to SQLite.
//! No polling and no injected DLL. A window must stay foreground for 1.5 s
//! before it is recorded; the run starts at the time of the event itself.
//! Idle, lock, sleep and clock changes pause recording (see presence.rs).
//!
//! Usage: focus.exe [--db <path>]   (default %LOCALAPPDATA%\Focus\focus.db)
#![windows_subsystem = "console"]

mod appinfo;
mod claude;
mod db;
mod presence;
mod recorder;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use presence::{Change, Presence, DISPLAY_OFF, IDLE, LOCKED, SLEEP};
use windows::core::{w, BOOL, GUID};
use windows::Win32::Foundation::{HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{RegisterPowerSettingNotification, POWERBROADCAST_SETTING};
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime;
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW, KillTimer,
    PostThreadMessageW, RegisterClassW, SetCoalescableTimer, SetTimer, TranslateMessage,
    DEVICE_NOTIFY_WINDOW_HANDLE, EVENT_OBJECT_NAMECHANGE, EVENT_SYSTEM_FOREGROUND, MSG,
    OBJID_WINDOW, WINDOW_EX_STYLE, WINDOW_STYLE, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
    WM_QUIT, WM_TIMER, WNDCLASSW,
};

const DEBOUNCE_MS: u32 = 1500;
/// Flush interval for the current run's end; coalescable so the CPU can sleep.
const FLUSH_MS: u32 = 60_000;
const FLUSH_TOLERANCE_MS: u32 = 10_000;
/// Idle checks: slow while the user is active, fast only while waiting for them to return.
const IDLE_CHECK_MS: u32 = 30_000;
const IDLE_CHECK_TOLERANCE_MS: u32 = 10_000;
const IDLE_RETURN_CHECK_MS: u32 = 3_000;
const IDLE_RETURN_TOLERANCE_MS: u32 = 1_000;
/// A wall-clock jump beyond the unbiased clock bigger than this means we slept.
const SLEEP_GAP_MS: i64 = 30_000;
const DEFAULT_IDLE_MINUTES: f64 = 3.0;

// Window messages and codes used by the hidden window (numeric to avoid feature sprawl).
const WM_QUERYENDSESSION: u32 = 0x0011;
const WM_ENDSESSION: u32 = 0x0016;
const WM_TIMECHANGE: u32 = 0x001E;
const WM_POWERBROADCAST: u32 = 0x0218;
const WM_WTSSESSION_CHANGE: u32 = 0x02B1;
const PBT_APMSUSPEND: usize = 0x0004;
const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;
const PBT_POWERSETTINGCHANGE: usize = 0x8013;
const WTS_CONSOLE_CONNECT: usize = 1;
const WTS_CONSOLE_DISCONNECT: usize = 2;
const WTS_REMOTE_CONNECT: usize = 3;
const WTS_REMOTE_DISCONNECT: usize = 4;
const WTS_SESSION_LOCK: usize = 7;
const WTS_SESSION_UNLOCK: usize = 8;
const GUID_CONSOLE_DISPLAY_STATE: GUID = GUID::from_u128(0x6fe69556_704a_47a0_8f24_c28d936fda47);

/// Thread id of the message loop, so Ctrl+C (another thread) can ask it to quit.
static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);

struct State {
    db: db::Db,
    recorder: recorder::Recorder,
    presence: Presence,
    idle_threshold_ms: u64,
    /// Clocks at the previous flush, to notice sleeps that sent no event.
    last_wall: i64,
    last_unbiased: i64,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static PENDING_HWND: Cell<isize> = const { Cell::new(0) };
    /// Epoch ms of the event that started the pending debounce.
    static PENDING_AT: Cell<i64> = const { Cell::new(0) };
    static DEBOUNCE_TIMER: Cell<usize> = const { Cell::new(0) };
    static FLUSH_TIMER: Cell<usize> = const { Cell::new(0) };
    static IDLE_TIMER: Cell<usize> = const { Cell::new(0) };
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Time the machine has been awake, in ms. Unlike the wall clock it does not
/// count sleep or hibernation.
fn unbiased_ms() -> i64 {
    let mut t = 0u64;
    unsafe {
        let _ = QueryUnbiasedInterruptTime(&mut t);
    }
    (t / 10_000) as i64 // 100 ns units
}

/// Milliseconds since the last keyboard or mouse input.
fn idle_now_ms() -> u64 {
    let mut li = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    unsafe {
        if GetLastInputInfo(&mut li).as_bool() {
            presence::idle_ms(GetTickCount(), li.dwTime)
        } else {
            0
        }
    }
}

/// Video calls, presentations and full-screen games count as presence even
/// without input, so they must not trigger the idle pause.
fn user_is_busy() -> bool {
    unsafe {
        matches!(
            SHQueryUserNotificationState(),
            Ok(s) if s == QUNS_BUSY || s == QUNS_PRESENTATION_MODE || s == QUNS_RUNNING_D3D_FULL_SCREEN
        )
    }
}

fn info_for(hwnd: HWND) -> Option<recorder::Info> {
    appinfo::info_for(hwnd, unsafe { GetCurrentProcessId() })
}

fn default_db_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Focus").join("focus.db")
}

fn db_path_from_args() -> PathBuf {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--db" {
            if let Some(p) = args.next() {
                return PathBuf::from(p);
            }
        }
    }
    default_db_path()
}

fn log_err(what: &str, e: rusqlite::Error) {
    // Never include window titles in logs; they are private.
    eprintln!("focus: {what} failed: {e}");
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

/// Opens a run for whatever is in the foreground right now.
fn resync(st: &mut State, at: i64) {
    if st.presence.paused() {
        return;
    }
    if let Some(info) = info_for(unsafe { GetForegroundWindow() }) {
        if let Err(e) = st.recorder.on_focus(&st.db, info, at) {
            log_err("resume", e);
        }
    }
}

/// Adds or clears a reason for being away. `at` is when it really began or
/// ended (idle starts in the past, at the last input).
fn set_reason(reason: u8, on: bool, at: i64) {
    with_state(|st| match st.presence.set(reason, on) {
        Change::Paused => {
            if let Err(e) = st.recorder.close(&st.db, at) {
                log_err("pause", e);
            }
        }
        Change::Resumed => resync(st, at),
        Change::None => {}
    });
}

fn reset_clock_baseline(st: &mut State) {
    st.last_wall = now_ms();
    st.last_unbiased = unbiased_ms();
}

unsafe extern "system" fn on_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if event == EVENT_OBJECT_NAMECHANGE
        && (id_object != OBJID_WINDOW.0 || hwnd != GetForegroundWindow())
    {
        return;
    }
    // Restart the debounce; only a window that stays 1.5 s is recorded.
    PENDING_HWND.with(|l| l.set(hwnd.0 as isize));
    PENDING_AT.with(|t| t.set(now_ms()));
    DEBOUNCE_TIMER.with(|t| {
        if t.get() != 0 {
            let _ = KillTimer(None, t.get());
        }
        t.set(SetTimer(None, 0, DEBOUNCE_MS, None));
    });
}

/// Ctrl+C / console close: ask the message loop to quit so the run is saved.
unsafe extern "system" fn on_ctrl(_kind: u32) -> BOOL {
    let _ = PostThreadMessageW(
        MAIN_THREAD.load(Ordering::SeqCst),
        WM_QUIT,
        WPARAM(0),
        LPARAM(0),
    );
    BOOL(1)
}

fn confirm_pending() {
    let hwnd = HWND(PENDING_HWND.with(|l| l.get()) as *mut _);
    let at = PENDING_AT.with(|t| t.get());
    let Some(info) = info_for(hwnd) else { return };
    with_state(|st| {
        if st.presence.paused() {
            return;
        }
        if let Err(e) = st.recorder.on_focus(&st.db, info, at) {
            log_err("record", e);
        }
    });
}

/// Every 60 s: persist the run's end, and notice sleeps that sent no event.
fn flush() {
    with_state(|st| {
        let (wall, unb) = (now_ms(), unbiased_ms());
        if presence::slept_between(st.last_wall, st.last_unbiased, wall, unb, SLEEP_GAP_MS) {
            // The machine slept: the run ended at the last instant we knew about.
            if let Err(e) = st.recorder.close(&st.db, st.last_wall) {
                log_err("close after sleep", e);
            }
            resync(st, wall);
        } else if let Err(e) = st.recorder.tick(&st.db, wall) {
            log_err("flush", e);
        }
        st.last_wall = wall;
        st.last_unbiased = unb;
    });
}

fn arm_idle_timer(fast: bool) {
    let (every, tolerance) = if fast {
        (IDLE_RETURN_CHECK_MS, IDLE_RETURN_TOLERANCE_MS)
    } else {
        (IDLE_CHECK_MS, IDLE_CHECK_TOLERANCE_MS)
    };
    IDLE_TIMER.with(|t| unsafe {
        if t.get() != 0 {
            let _ = KillTimer(None, t.get());
        }
        t.set(SetCoalescableTimer(None, 0, every, None, tolerance));
    });
}

fn check_idle() {
    let Some((threshold, is_idle)) = with_state(|st| (st.idle_threshold_ms, st.presence.has(IDLE)))
    else {
        return;
    };
    if threshold == 0 {
        return; // idle detection turned off in settings
    }
    let idle = idle_now_ms();
    let began = now_ms() - idle as i64; // the instant of the last input
    if !is_idle {
        if idle >= threshold && !user_is_busy() {
            set_reason(IDLE, true, began);
            arm_idle_timer(true);
        }
    } else if idle < threshold {
        set_reason(IDLE, false, began);
        arm_idle_timer(false);
        // Record the window that is already in the foreground at startup.
        with_state(|st| resync(st, now_ms()));
    }
}

fn on_session_change(code: usize) {
    let now = now_ms();
    match code {
        WTS_SESSION_LOCK | WTS_CONSOLE_DISCONNECT | WTS_REMOTE_DISCONNECT => {
            set_reason(LOCKED, true, now)
        }
        WTS_SESSION_UNLOCK | WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT => {
            set_reason(LOCKED, false, now)
        }
        _ => {}
    }
}

fn on_power(code: usize, lparam: LPARAM) {
    let now = now_ms();
    match code {
        PBT_APMSUSPEND => set_reason(SLEEP, true, now),
        PBT_APMRESUMEAUTOMATIC => {
            with_state(reset_clock_baseline);
            set_reason(SLEEP, false, now);
        }
        PBT_POWERSETTINGCHANGE => {
            // Modern Standby may never send APMSUSPEND, but the screen turns off.
            let s = lparam.0 as *const POWERBROADCAST_SETTING;
            if s.is_null() {
                return;
            }
            let (guid, len) = unsafe { ((*s).PowerSetting, (*s).DataLength) };
            if guid == GUID_CONSOLE_DISPLAY_STATE && len >= 1 {
                let state = unsafe { (*s).Data[0] }; // 0 off, 1 on, 2 dimmed
                set_reason(DISPLAY_OFF, state == 0, now);
            }
        }
        _ => {}
    }
}

/// The user or NTP changed the clock: end the run and start a fresh one.
fn on_time_change() {
    let now = now_ms();
    with_state(|st| {
        if let Err(e) = st.recorder.close(&st.db, now) {
            log_err("close on time change", e);
        }
        reset_clock_baseline(st);
        resync(st, now);
    });
}

/// Shutdown, restart or logoff: save everything; Windows gives a few seconds.
fn on_end_session() {
    with_state(|st| {
        if let Err(e) = st.recorder.close(&st.db, now_ms()) {
            log_err("close on shutdown", e);
        }
        if let Err(e) = st.db.checkpoint() {
            log_err("checkpoint", e);
        }
    });
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_WTSSESSION_CHANGE => on_session_change(wparam.0),
        WM_POWERBROADCAST => {
            on_power(wparam.0, lparam);
            return LRESULT(1); // TRUE: request granted
        }
        WM_TIMECHANGE => on_time_change(),
        WM_QUERYENDSESSION => return LRESULT(1),
        WM_ENDSESSION if wparam.0 != 0 => on_end_session(),
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// A hidden top-level window (never shown). Message-only windows would not
/// receive the shutdown and power broadcasts.
unsafe fn create_hidden_window() -> Option<HWND> {
    let hinstance = HINSTANCE(GetModuleHandleW(None).ok()?.0);
    let class = w!("FocusHiddenWindow");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance,
        lpszClassName: class,
        ..Default::default()
    };
    if RegisterClassW(&wc) == 0 {
        return None;
    }
    CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        w!("Focus"),
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

fn idle_threshold_ms(db: &db::Db) -> u64 {
    let minutes = db
        .get_setting("idleMinutes")
        .ok()
        .flatten()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(DEFAULT_IDLE_MINUTES);
    (minutes.max(0.0) * 60_000.0) as u64
}

fn main() {
    let path = db_path_from_args();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let db = match db::Db::open(&path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("focus: cannot open database at {}: {e}", path.display());
            std::process::exit(1);
        }
    };
    let threshold = idle_threshold_ms(&db);
    claude::spawn(path.clone());
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            db,
            recorder: recorder::Recorder::default(),
            presence: Presence::default(),
            idle_threshold_ms: threshold,
            last_wall: now_ms(),
            last_unbiased: unbiased_ms(),
        })
    });

    unsafe {
        MAIN_THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
        let _ = SetConsoleCtrlHandler(Some(on_ctrl), true);

        if let Some(hwnd) = create_hidden_window() {
            let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
            let _ = RegisterPowerSettingNotification(
                HANDLE(hwnd.0),
                &GUID_CONSOLE_DISPLAY_STATE,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            );
        } else {
            eprintln!("focus: could not create the event window; lock and sleep detection is off");
        }

        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(on_event),
            0,
            0,
            flags,
        );
        SetWinEventHook(
            EVENT_OBJECT_NAMECHANGE,
            EVENT_OBJECT_NAMECHANGE,
            None,
            Some(on_event),
            0,
            0,
            flags,
        );
        FLUSH_TIMER.with(|t| {
            t.set(SetCoalescableTimer(
                None,
                0,
                FLUSH_MS,
                None,
                FLUSH_TOLERANCE_MS,
            ))
        });
        arm_idle_timer(false);
        eprintln!("focus: recording to {} (Ctrl+C to quit)", path.display());

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_TIMER {
                let id = msg.wParam.0;
                if id == DEBOUNCE_TIMER.with(|t| t.get()) {
                    let _ = KillTimer(None, id);
                    DEBOUNCE_TIMER.with(|t| t.set(0));
                    confirm_pending();
                    continue;
                }
                if id == FLUSH_TIMER.with(|t| t.get()) {
                    flush();
                    continue;
                }
                if id == IDLE_TIMER.with(|t| t.get()) {
                    check_idle();
                    continue;
                }
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // Quit: end the current run, checkpoint the WAL and let SQLite tidy up.
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().take() {
            let mut rec = st.recorder;
            if let Err(e) = rec.close(&st.db, now_ms()) {
                log_err("close", e);
            }
            let _ = st.db.optimize();
            let _ = st.db.checkpoint();
        }
    });
}
