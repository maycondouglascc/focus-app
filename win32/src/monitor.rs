//! Foreground window monitor running on its own background thread.
//!
//! Uses out-of-context `SetWinEventHook` + a hidden top-level window (`FocusHiddenWindow`)
//! for power, session, clock, settings reload, URL results, and shutdown broadcasts so
//! modal UI loops never drop thread timers or events.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
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

use crate::appinfo::{self, WindowTarget};
use crate::db::{Db, Settings};
use crate::db_thread::{DbHandle, QueuedSink};
use crate::locale::SystemLocale;
use crate::log::{self, Category};
use crate::presence::{self, Change, Presence, DISPLAY_OFF, IDLE, LOCKED, SLEEP, USER_PAUSED};
use crate::qos;
use crate::recorder::{self, Exclusions};
use crate::url::UrlWorker;

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
const WM_APP_RELOAD_SETTINGS: u32 = WM_APP + 22;
const WM_APP_URL_READY: u32 = WM_APP + 23;

static MONITOR_HWND: AtomicIsize = AtomicIsize::new(0);

pub fn request_reload() {
    let raw = MONITOR_HWND.load(Ordering::SeqCst);
    if raw != 0 {
        // SAFETY: `raw` is a live `FocusHiddenWindow` owned by the monitor thread.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(raw as *mut _)),
                WM_APP_RELOAD_SETTINGS,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

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
    read_db: Db,
    sink: QueuedSink,
    handle: DbHandle,
    locale: SystemLocale,
    recorder: recorder::Recorder,
    presence: Presence,
    idle_threshold_ms: u64,
    keep_active_when_busy: bool,
    browser_tabs: bool,
    /// Clocks at the previous flush, to notice sleeps that sent no event.
    last_wall: i64,
    last_unbiased: i64,
    /// Last foreground HWND passed to `record_window` for the 60s hook watchdog.
    last_fg_hwnd: isize,
    url_worker: UrlWorker,
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

fn inspect_hwnd(hwnd: HWND) -> WindowTarget {
    // SAFETY: `GetCurrentProcessId` is a plain process-ID read.
    appinfo::inspect(hwnd, unsafe { GetCurrentProcessId() })
}

fn log_err(op: &'static str, e: rusqlite::Error) {
    // Never include window titles in logs; they are private.
    log::db_err(Category::Monitor, op, &e);
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

fn record_window(st: &mut State, hwnd: HWND, at: i64) {
    st.last_fg_hwnd = hwnd.0 as isize;
    if st.presence.paused() {
        return;
    }
    match inspect_hwnd(hwnd) {
        WindowTarget::OwnApp => {
            st.url_worker.invalidate();
            if let Err(e) = st.recorder.close(&st.sink, at) {
                log_err("close_own_app", e);
            }
        }
        WindowTarget::Unavailable => {}
        WindowTarget::Tracked(info) => {
            let is_browser = recorder::is_supported_browser_exe(&info.exe);
            let is_private = recorder::is_private_browser_title(&info.exe, &info.detail);
            let pid = info.pid;
            match st.recorder.on_focus(&st.sink, info, at) {
                Ok(Some(run_id)) => {
                    if st.browser_tabs && is_browser && !is_private {
                        st.url_worker.submit(run_id, hwnd.0 as isize, pid);
                    } else {
                        st.url_worker.invalidate();
                    }
                }
                Ok(None) => {
                    // Excluded windows close the active run; ensure no stale URL lands later.
                    if is_private || !is_browser {
                        st.url_worker.invalidate();
                    }
                }
                Err(e) => log_err("record", e),
            }
        }
    }
}

/// Opens a run for whatever is in the foreground right now.
fn resync(st: &mut State, at: i64) {
    if st.presence.paused() {
        return;
    }
    // SAFETY: `GetForegroundWindow` returns the current foreground HWND or null.
    let fg = unsafe { GetForegroundWindow() };
    record_window(st, fg, at);
}

fn apply_reason(st: &mut State, reason: u8, on: bool, at: i64) {
    match st.presence.set(reason, on) {
        Change::Paused => {
            st.url_worker.invalidate();
            if let Err(e) = st.recorder.close(&st.sink, at) {
                log_err("pause", e);
            }
        }
        Change::Resumed => resync(st, at),
        Change::None => {}
    }
}

/// Adds or clears a reason for being away. `at` is when it really began or
/// ended (idle starts in the past, at the last input).
fn set_reason(reason: u8, on: bool, at: i64) {
    with_state(|st| apply_reason(st, reason, on, at));
}

fn apply_settings(st: &mut State, s: Settings, at: i64) {
    st.idle_threshold_ms = (s.idle_minutes as u64) * 60_000;
    st.keep_active_when_busy = s.keep_active_when_busy;
    st.browser_tabs = s.browser_tabs;
    if !st.browser_tabs {
        st.url_worker.invalidate();
    }
    let next_excl = Exclusions {
        apps: s.excluded_apps,
        title_keywords: s.excluded_title_patterns,
    };
    let excl_changed = st.recorder.exclusions() != &next_excl;
    st.recorder.set_exclusions(next_excl);
    apply_reason(st, USER_PAUSED, !s.monitor_enabled, at);
    if st.idle_threshold_ms == 0 && st.presence.has(IDLE) {
        apply_reason(st, IDLE, false, at);
        arm_idle_timer(false);
    }
    if excl_changed && !st.presence.paused() {
        resync(st, at);
    }
}

fn reload_settings() {
    let now = now_ms();
    with_state(|st| {
        let s = st
            .read_db
            .load_settings(&st.locale)
            .unwrap_or_else(|_| Settings::defaults_for(&st.locale));
        apply_settings(st, s, now);
    });
}

fn apply_pending_urls() {
    with_state(|st| {
        for res in st.url_worker.drain_results() {
            st.handle.set_run_url(res.run_id, res.url, res.context);
        }
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
    with_state(|st| record_window(st, hwnd, at));
}

/// Every 60 s: persist the run's end, notice silent sleeps, and run the hook watchdog.
fn flush() {
    with_state(|st| {
        let (wall, unb) = (now_ms(), unbiased_ms());
        if presence::slept_between(st.last_wall, st.last_unbiased, wall, unb, SLEEP_GAP_MS) {
            // The machine slept: the run ended at the last instant we knew about.
            if let Err(e) = st.recorder.close(&st.sink, st.last_wall) {
                log_err("close_after_sleep", e);
            }
            resync(st, wall);
        } else {
            if let Err(e) = st.recorder.tick(&st.sink, wall) {
                log_err("flush", e);
            }
            // Hook watchdog: catch missed focus events if foreground changed silently.
            let pending = DEBOUNCE_TIMER.with(|t| t.get()) != 0;
            if !st.presence.paused() && !pending {
                // SAFETY: `GetForegroundWindow` returns the current foreground HWND or null.
                let fg = unsafe { GetForegroundWindow() };
                let fg_raw = fg.0 as isize;
                if fg_raw != 0 && fg_raw != st.last_fg_hwnd {
                    log::event(Category::Monitor, "watchdog_resync");
                    record_window(st, fg, wall);
                }
            }
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
    let Some((threshold, is_idle, keep_busy)) = with_state(|st| {
        (
            st.idle_threshold_ms,
            st.presence.has(IDLE),
            st.keep_active_when_busy,
        )
    }) else {
        return;
    };
    if threshold == 0 {
        if is_idle {
            set_reason(IDLE, false, now_ms());
            arm_idle_timer(false);
        }
        return; // idle detection turned off in settings
    }
    let idle = idle_now_ms();
    let began = now_ms() - idle as i64; // the instant of the last input
    if !is_idle {
        let busy = keep_busy && user_is_busy();
        if idle >= threshold && !busy {
            set_reason(IDLE, true, began);
            arm_idle_timer(true);
        }
    } else if idle < threshold {
        set_reason(IDLE, false, began);
        arm_idle_timer(false);
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
        if let Err(e) = st.recorder.close(&st.sink, now) {
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
            if let Err(e) = st.recorder.close(&st.sink, now_ms()) {
                log_err("close_on_shutdown", e);
            }
            if st.handle.checkpoint().is_err() {
                log::event(Category::Monitor, "checkpoint_failed");
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
        m if m == WM_APP_RELOAD_SETTINGS => {
            reload_settings();
            return LRESULT(0);
        }
        m if m == WM_APP_URL_READY => {
            apply_pending_urls();
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

fn run_thread(
    db_path: PathBuf,
    handle: DbHandle,
    end_session_sync: Arc<(Mutex<bool>, Condvar)>,
    ready_tx: mpsc::Sender<Result<isize, String>>,
) {
    qos::set_background_qos(true);
    let read_db = match Db::open_readonly(&db_path) {
        Ok(db) => db,
        Err(e) => {
            log::db_err(Category::Monitor, "open_db", &e);
            let _ = ready_tx.send(Err(e.to_string()));
            return;
        }
    };
    let locale = SystemLocale::query();
    let settings = read_db
        .load_settings(&locale)
        .unwrap_or_else(|_| Settings::defaults_for(&locale));

    // SAFETY: creates the monitor thread's hidden window, registers Win32 hooks/timers, and cleans them up before exit.
    unsafe {
        let mut power_notify: Option<HPOWERNOTIFY> = None;
        let hidden_hwnd = create_hidden_window();
        let hwnd_raw = hidden_hwnd.map_or(0, |h| h.0 as isize);
        MONITOR_HWND.store(hwnd_raw, Ordering::SeqCst);
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

        let url_worker = UrlWorker::spawn(hwnd_raw, WM_APP_URL_READY);
        let mut recorder = recorder::Recorder::default();
        recorder.set_exclusions(Exclusions {
            apps: settings.excluded_apps.clone(),
            title_keywords: settings.excluded_title_patterns.clone(),
        });
        let mut presence = Presence::default();
        if !settings.monitor_enabled {
            let _ = presence.set(USER_PAUSED, true);
        }

        let sink = QueuedSink::new(handle.clone());
        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                read_db,
                sink,
                handle,
                locale,
                recorder,
                presence,
                idle_threshold_ms: (settings.idle_minutes as u64) * 60_000,
                keep_active_when_busy: settings.keep_active_when_busy,
                browser_tabs: settings.browser_tabs,
                last_wall: now_ms(),
                last_unbiased: unbiased_ms(),
                last_fg_hwnd: 0,
                url_worker,
                end_session_sync,
                end_session_flushed: false,
            })
        });

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

        let _ = ready_tx.send(Ok(hwnd_raw));

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

        MONITOR_HWND.store(0, Ordering::SeqCst);
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

    // Quit: stop URL thread and flush the active run (`DbThread::stop` optimizes and checkpoints).
    STATE.with(|s| {
        if let Some(mut st) = s.borrow_mut().take() {
            st.url_worker.stop();
            if let Err(e) = st.recorder.close(&st.sink, now_ms()) {
                log_err("close", e);
            }
        }
    });
    log::event(Category::Monitor, "stopped");
}

pub fn spawn(db_path: PathBuf, handle: DbHandle) -> Result<MonitorHandle, String> {
    let (tx, rx) = mpsc::channel();
    let end_session_sync = Arc::new((Mutex::new(false), Condvar::new()));
    let sync_clone = Arc::clone(&end_session_sync);
    let join = std::thread::Builder::new()
        .name("focus-monitor".into())
        .spawn(move || run_thread(db_path, handle, sync_clone, tx))
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
