//! Foreground window monitor running on its own background thread.
//!
//! Uses out-of-context `SetWinEventHook` + a hidden top-level window (`FocusHiddenWindow`)
//! for power, session, clock, and shutdown broadcasts so modal UI loops never drop
//! thread timers or events.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::core::{w, GUID};
use windows::Win32::Foundation::{HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{
    RegisterPowerSettingNotification, UnregisterPowerSettingNotification, HPOWERNOTIFY,
    POWERBROADCAST_SETTING,
};
use windows::Win32::System::RemoteDesktop::{
    WTSRegisterSessionNotification, WTSUnRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow,
    GetMessageW, KillTimer, PostMessageW, PostQuitMessage, RegisterClassW, SetCoalescableTimer,
    SetTimer, TranslateMessage, DEVICE_NOTIFY_WINDOW_HANDLE, EVENT_OBJECT_NAMECHANGE,
    EVENT_SYSTEM_FOREGROUND, MSG, OBJID_WINDOW, WINDOW_EX_STYLE, WINDOW_STYLE,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_APP, WM_TIMER, WNDCLASSW,
};

use crate::appinfo;
use crate::db::Db;
use crate::log::{self, Category};
use crate::presence::{self, Change, Presence, DISPLAY_OFF, IDLE, LOCKED, SLEEP};
use crate::recorder;

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

const WM_APP_MONITOR_STOP: u32 = WM_APP + 20;
const WM_APP_END_SESSION: u32 = WM_APP + 21;

pub struct MonitorHandle {
    hwnd_raw: isize,
    end_session_sync: Arc<(Mutex<bool>, Condvar)>,
    join: Option<JoinHandle<()>>,
}

impl MonitorHandle {
    pub fn stop(&mut self) {
        if self.hwnd_raw != 0 {
            let hwnd = HWND(self.hwnd_raw as *mut _);
            // SAFETY: `hwnd` belongs to the monitor thread and accepts posted `WM_APP` stop notifications.
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_APP_MONITOR_STOP, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        self.hwnd_raw = 0;
    }

    #[allow(dead_code)]
    pub fn end_session_and_wait(&self, timeout: Duration) {
        if self.hwnd_raw != 0 {
            let hwnd = HWND(self.hwnd_raw as *mut _);
            // SAFETY: `hwnd` belongs to the monitor thread; posting ensures flush even if UI received `WM_ENDSESSION` first.
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_APP_END_SESSION, WPARAM(0), LPARAM(0));
            }
        }
        let (lock, cvar) = &*self.end_session_sync;
        if let Ok(guard) = lock.lock() {
            let _ = cvar.wait_timeout_while(guard, timeout, |done| !*done);
        }
    }
}

impl Drop for MonitorHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

struct State {
    db: Db,
    recorder: recorder::Recorder,
    presence: Presence,
    idle_threshold_ms: u64,
    /// Clocks at the previous flush, to notice sleeps that sent no event.
    last_wall: i64,
    last_unbiased: i64,
    end_session_sync: Arc<(Mutex<bool>, Condvar)>,
    end_session_flushed: bool,
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
    // SAFETY: `t` is a valid stack u64 out-pointer for `QueryUnbiasedInterruptTime`.
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
    // SAFETY: `li` has `cbSize` matching `LASTINPUTINFO`; `GetTickCount` is always valid.
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
    // SAFETY: `SHQueryUserNotificationState` takes no borrowed buffers and queries shell state.
    unsafe {
        matches!(
            SHQueryUserNotificationState(),
            Ok(s) if s == QUNS_BUSY || s == QUNS_PRESENTATION_MODE || s == QUNS_RUNNING_D3D_FULL_SCREEN
        )
    }
}

fn info_for(hwnd: HWND) -> Option<recorder::Info> {
    // SAFETY: `GetCurrentProcessId` is a plain process-ID read.
    appinfo::info_for(hwnd, unsafe { GetCurrentProcessId() })
}

fn log_err(op: &'static str, e: rusqlite::Error) {
    // Never include window titles in logs; they are private.
    log::db_err(Category::Monitor, op, &e);
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

/// Opens a run for whatever is in the foreground right now.
fn resync(st: &mut State, at: i64) {
    if st.presence.paused() {
        return;
    }
    // SAFETY: `GetForegroundWindow` returns the current foreground HWND or null.
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
    // SAFETY: runs on the monitor thread inside `DispatchMessageW`; calls thread-local Win32 timer APIs.
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
                log_err("close_after_sleep", e);
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
    IDLE_TIMER.with(|t| {
        // SAFETY: thread-owned coalescable timer with `hwnd = None` on the monitor message loop.
        unsafe {
            if t.get() != 0 {
                let _ = KillTimer(None, t.get());
            }
            t.set(SetCoalescableTimer(None, 0, every, None, tolerance));
        }
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
        // Record the window that is already in the foreground when returning from idle.
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
            // SAFETY: Windows passes a valid `POWERBROADCAST_SETTING` pointer in `lparam` for `PBT_POWERSETTINGCHANGE`.
            let (guid, len) = unsafe { ((*s).PowerSetting, (*s).DataLength) };
            if guid == GUID_CONSOLE_DISPLAY_STATE && len >= 1 {
                // SAFETY: `DataLength >= 1` guarantees `Data[0]` is readable inside the variable-length struct.
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
            log_err("close_on_time_change", e);
        }
        reset_clock_baseline(st);
        resync(st, now);
    });
}

/// Shutdown, restart or logoff: save everything; Windows gives a few seconds.
fn on_end_session() {
    with_state(|st| {
        if !st.end_session_flushed {
            if let Err(e) = st.recorder.close(&st.db, now_ms()) {
                log_err("close_on_shutdown", e);
            }
            if let Err(e) = st.db.checkpoint() {
                log_err("checkpoint", e);
            }
            st.end_session_flushed = true;
        }
        let (lock, cvar) = &*st.end_session_sync;
        if let Ok(mut done) = lock.lock() {
            *done = true;
            cvar.notify_all();
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
        WM_ENDSESSION if wparam.0 != 0 => {
            on_end_session();
            return LRESULT(0);
        }
        m if m == WM_APP_END_SESSION => {
            on_end_session();
            return LRESULT(0);
        }
        m if m == WM_APP_MONITOR_STOP => {
            // SAFETY: runs on the monitor thread's message loop and breaks cleanly out of `GetMessageW`.
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: forwards unhandled messages to the default Win32 window procedure.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// A hidden top-level window (never shown). Message-only windows would not
/// receive the shutdown and power broadcasts.
unsafe fn create_hidden_window() -> Option<HWND> {
    // SAFETY: called on the monitor thread; module handle and class registration live for the process.
    unsafe {
        let hinstance = HINSTANCE(GetModuleHandleW(None).ok()?.0);
        let class = w!("FocusHiddenWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
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
}

fn idle_threshold_ms(db: &Db) -> u64 {
    let minutes = db
        .get_setting("idleMinutes")
        .ok()
        .flatten()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(DEFAULT_IDLE_MINUTES);
    (minutes.max(0.0) * 60_000.0) as u64
}

fn run_thread(
    db_path: PathBuf,
    end_session_sync: Arc<(Mutex<bool>, Condvar)>,
    ready_tx: mpsc::Sender<Result<isize, String>>,
) {
    if let Some(dir) = db_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let db = match Db::open(&db_path) {
        Ok(db) => db,
        Err(e) => {
            log::db_err(Category::Monitor, "open_db", &e);
            let _ = ready_tx.send(Err(e.to_string()));
            return;
        }
    };
    let threshold = idle_threshold_ms(&db);
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            db,
            recorder: recorder::Recorder::default(),
            presence: Presence::default(),
            idle_threshold_ms: threshold,
            last_wall: now_ms(),
            last_unbiased: unbiased_ms(),
            end_session_sync,
            end_session_flushed: false,
        })
    });

    // SAFETY: creates the monitor thread's hidden window, registers Win32 hooks/timers, and cleans them up before exit.
    unsafe {
        let mut power_notify: Option<HPOWERNOTIFY> = None;
        let hidden_hwnd = create_hidden_window();
        if let Some(hwnd) = hidden_hwnd {
            let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
            power_notify = RegisterPowerSettingNotification(
                HANDLE(hwnd.0),
                &GUID_CONSOLE_DISPLAY_STATE,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            )
            .ok();
        } else {
            log::event(Category::Monitor, "hidden_window_failed");
        }

        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        let hook_fg = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(on_event),
            0,
            0,
            flags,
        );
        let hook_name = SetWinEventHook(
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
        with_state(|st| resync(st, now_ms()));
        log::event(Category::Monitor, "started");

        let _ = ready_tx.send(Ok(hidden_hwnd.map_or(0, |h| h.0 as isize)));

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

        if !hook_fg.is_invalid() {
            let _ = UnhookWinEvent(hook_fg);
        }
        if !hook_name.is_invalid() {
            let _ = UnhookWinEvent(hook_name);
        }
        for cell in [&DEBOUNCE_TIMER, &FLUSH_TIMER, &IDLE_TIMER] {
            cell.with(|t| {
                if t.get() != 0 {
                    let _ = KillTimer(None, t.get());
                    t.set(0);
                }
            });
        }
        if let Some(pn) = power_notify {
            let _ = UnregisterPowerSettingNotification(pn);
        }
        if let Some(hwnd) = hidden_hwnd {
            let _ = WTSUnRegisterSessionNotification(hwnd);
            let _ = DestroyWindow(hwnd);
        }
    }

    // Quit: end the current run, optimize and checkpoint the WAL.
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
    log::event(Category::Monitor, "stopped");
}

pub fn spawn(db_path: PathBuf) -> Result<MonitorHandle, String> {
    let (tx, rx) = mpsc::channel();
    let end_session_sync = Arc::new((Mutex::new(false), Condvar::new()));
    let sync_clone = Arc::clone(&end_session_sync);
    let join = std::thread::Builder::new()
        .name("focus-monitor".into())
        .spawn(move || run_thread(db_path, sync_clone, tx))
        .map_err(|e| format!("failed to spawn monitor thread: {e}"))?;

    match rx.recv() {
        Ok(Ok(hwnd_raw)) => Ok(MonitorHandle {
            hwnd_raw,
            end_session_sync,
            join: Some(join),
        }),
        Ok(Err(e)) => {
            let _ = join.join();
            Err(e)
        }
        Err(_) => {
            let _ = join.join();
            Err("monitor thread exited before initialization".into())
        }
    }
}
