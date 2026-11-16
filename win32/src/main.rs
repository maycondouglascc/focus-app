//! Focus monitor: event-driven foreground tracking written to SQLite.
//! No polling and no injected DLL. A window must stay foreground for 1.5 s
//! before it is recorded; the run starts at the time of the event itself.
//!
//! Usage: focus.exe [--db <path>]   (default %LOCALAPPDATA%\Focus\focus.db)
#![windows_subsystem = "console"]

mod db;
mod recorder;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, MAX_PATH, WPARAM};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId,
    InternalGetWindowText, KillTimer, PostThreadMessageW, SetCoalescableTimer, SetTimer,
    TranslateMessage, EVENT_OBJECT_NAMECHANGE, EVENT_SYSTEM_FOREGROUND, MSG, OBJID_WINDOW,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_QUIT, WM_TIMER,
};

const DEBOUNCE_MS: u32 = 1500;
/// Flush interval for the current run's end; coalescable so the CPU can sleep.
const FLUSH_MS: u32 = 60_000;
const FLUSH_TOLERANCE_MS: u32 = 10_000;

/// Thread id of the message loop, so Ctrl+C (another thread) can ask it to quit.
static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);

struct State {
    db: db::Db,
    recorder: recorder::Recorder,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static PENDING_HWND: Cell<isize> = const { Cell::new(0) };
    /// Epoch ms of the event that started the pending debounce.
    static PENDING_AT: Cell<i64> = const { Cell::new(0) };
    static DEBOUNCE_TIMER: Cell<usize> = const { Cell::new(0) };
    static FLUSH_TIMER: Cell<usize> = const { Cell::new(0) };
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Returns the exe path of the process owning `hwnd`, if it can be opened.
/// None also for our own process, which must never count as work.
fn exe_path(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == GetCurrentProcessId() {
            return None;
        }
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; MAX_PATH as usize];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        ok.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// InternalGetWindowText never sends WM_GETTEXT, so a hung app cannot block us.
fn title(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { InternalGetWindowText(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// "C:\Program Files\X\code.exe" -> "code".
fn app_name(path: &str) -> String {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    file.strip_suffix(".exe")
        .or_else(|| file.strip_suffix(".EXE"))
        .unwrap_or(file)
        .to_string()
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
    let Some(exe) = exe_path(hwnd) else { return };
    let info = recorder::Info {
        key: exe.to_lowercase(),
        app: app_name(&exe),
        detail: title(hwnd),
    };
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            if let Err(e) = st.recorder.on_focus(&st.db, info, at) {
                log_err("record", e);
            }
        }
    });
}

fn flush() {
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            if let Err(e) = st.recorder.tick(&st.db, now_ms()) {
                log_err("flush", e);
            }
        }
    });
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
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            db,
            recorder: recorder::Recorder::default(),
        })
    });

    unsafe {
        MAIN_THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
        let _ = SetConsoleCtrlHandler(Some(on_ctrl), true);

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
        eprintln!("focus: recording to {} (Ctrl+C to quit)", path.display());

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_TIMER {
                if msg.wParam.0 == DEBOUNCE_TIMER.with(|t| t.get()) {
                    let _ = KillTimer(None, msg.wParam.0);
                    DEBOUNCE_TIMER.with(|t| t.set(0));
                    confirm_pending();
                    continue;
                }
                if msg.wParam.0 == FLUSH_TIMER.with(|t| t.get()) {
                    flush();
                    continue;
                }
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // Quit: end the current run and let SQLite tidy up.
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().take() {
            let mut rec = st.recorder;
            if let Err(e) = rec.close(&st.db, now_ms()) {
                log_err("close", e);
            }
            let _ = st.db.optimize();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::app_name;

    #[test]
    fn app_name_strips_directory_and_extension() {
        assert_eq!(
            app_name(r"C:\Program Files\Mozilla Firefox\firefox.exe"),
            "firefox"
        );
        assert_eq!(app_name("code.EXE"), "code");
        assert_eq!(app_name("noext"), "noext");
    }
}
