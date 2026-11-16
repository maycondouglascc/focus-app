//! F0 spike: event-driven foreground monitor. No polling, no injected DLL.
//! Prints "<exe> | <title>" after the foreground window is stable for 1.5 s.
#![windows_subsystem = "console"]

use std::cell::Cell;

mod db;

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, MAX_PATH};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId,
    InternalGetWindowText, KillTimer, SetTimer, TranslateMessage, EVENT_OBJECT_NAMECHANGE,
    EVENT_SYSTEM_FOREGROUND, MSG, OBJID_WINDOW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

const DEBOUNCE_MS: u32 = 1500;

thread_local! {
    static LAST: Cell<isize> = const { Cell::new(0) };
    static TIMER: Cell<usize> = const { Cell::new(0) };
}

/// Returns the exe path of the process owning `hwnd`, if it can be opened.
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
    // Restart the debounce; only a window that stays 1.5 s is reported.
    LAST.with(|l| l.set(hwnd.0 as isize));
    TIMER.with(|t| {
        if t.get() != 0 {
            let _ = KillTimer(None, t.get());
        }
        t.set(SetTimer(None, 0, DEBOUNCE_MS, None));
    });
}

fn main() {
    unsafe {
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
        eprintln!("focus spike: waiting for events (Ctrl+C to quit)");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_TIMER
                && msg.wParam.0 == TIMER.with(|t| t.get())
            {
                let _ = KillTimer(None, msg.wParam.0);
                TIMER.with(|t| t.set(0));
                let hwnd = HWND(LAST.with(|l| l.get()) as *mut _);
                if let Some(exe) = exe_path(hwnd) {
                    println!("{exe} | {}", title(hwnd));
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
