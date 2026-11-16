//! The main window: created on `open`, fully destroyed on `close` or when the
//! user closes it. Custom frame (see `frame.rs`), no standard non-client area.

use std::cell::Cell;

use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, FillRect, GetStockObject, ScreenToClient, HBRUSH, PAINTSTRUCT,
    WHITE_BRUSH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, IsWindow,
    IsZoomed, LoadCursorW, PeekMessageW, RegisterClassW, SetForegroundWindow, SetWindowPos,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, HTBOTTOM, HTBOTTOMLEFT,
    HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTCLOSE, HTLEFT, HTMAXBUTTON, HTMINBUTTON, HTRIGHT, HTTOP,
    HTTOPLEFT, HTTOPRIGHT, IDC_ARROW, MSG, NCCALCSIZE_PARAMS, PM_REMOVE, SM_CXPADDEDBORDER,
    SM_CXSIZEFRAME, SM_CYSIZEFRAME, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SW_SHOW, WINDOW_EX_STYLE, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND,
    WM_NCCALCSIZE, WM_NCHITTEST, WM_PAINT, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

use super::frame::{self, Hit};

thread_local! {
    static WINDOW: Cell<Option<HWND>> = const { Cell::new(None) };
}

pub fn is_open() -> bool {
    WINDOW
        .with(|w| w.get())
        .is_some_and(|h| unsafe { IsWindow(Some(h)).as_bool() })
}

/// Opens the window, or brings the existing one to the front.
pub fn open() -> bool {
    if let Some(h) = WINDOW.with(|w| w.get()) {
        if unsafe { IsWindow(Some(h)).as_bool() } {
            // SAFETY: `h` was just checked to be a live window of this thread.
            unsafe {
                let _ = SetForegroundWindow(h);
            }
            return true;
        }
    }
    // SAFETY: called on the UI thread; every handle comes from the calls below.
    unsafe { create().is_some() }
}

/// Destroys the window if it is open.
pub fn close() {
    if let Some(h) = WINDOW.with(|w| w.take()) {
        // SAFETY: `h` is ours; DestroyWindow on a dead handle just fails.
        unsafe {
            let _ = DestroyWindow(h);
        }
    }
}

/// Dispatches every pending message of this thread without blocking.
pub fn pump() {
    // SAFETY: standard message pump on the thread that owns the window.
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe fn create() -> Option<HWND> {
    let hinstance = HINSTANCE(GetModuleHandleW(None).ok()?.0);
    let class = w!("FocusMainWindow");
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance,
        hCursor: LoadCursorW(None, IDC_ARROW).ok()?,
        lpszClassName: class,
        ..Default::default()
    };
    // Registering twice fails with "class exists", which is fine.
    RegisterClassW(&wc);
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        w!("Focus"),
        WS_OVERLAPPEDWINDOW,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        1100,
        760,
        None,
        None,
        Some(hinstance),
        None,
    )
    .ok()?;
    WINDOW.with(|w| w.set(Some(hwnd)));
    // Keep the drop shadow of a frameless window.
    let margins = MARGINS {
        cyTopHeight: 1,
        ..Default::default()
    };
    let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    let _ = SetWindowPos(
        hwnd,
        None,
        0,
        0,
        0,
        0,
        SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
    let _ = ShowWindow(hwnd, SW_SHOW);
    Some(hwnd)
}

unsafe fn on_nccalcsize(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    if wparam.0 == 0 {
        return None;
    }
    // The client area is the whole window; only a maximized window overhangs the
    // screen by its (invisible) frame, which has to be cut off.
    if IsZoomed(hwnd).as_bool() {
        let dpi = GetDpiForWindow(hwnd);
        let params = &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS);
        let pad = GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
        let fx = GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) + pad;
        let fy = GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) + pad;
        let r = &mut params.rgrc[0];
        r.left += fx;
        r.right -= fx;
        r.top += fy;
        r.bottom -= fy;
    }
    Some(LRESULT(0))
}

unsafe fn on_nchittest(hwnd: HWND, lparam: LPARAM) -> LRESULT {
    let mut pt = POINT {
        x: (lparam.0 & 0xFFFF) as i16 as i32,
        y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
    };
    let mut rc = RECT::default();
    let _ = ScreenToClient(hwnd, &mut pt);
    let _ = GetClientRect(hwnd, &mut rc);
    let hit = frame::hit_test(
        pt.x,
        pt.y,
        rc.right - rc.left,
        rc.bottom - rc.top,
        GetDpiForWindow(hwnd),
        IsZoomed(hwnd).as_bool(),
    );
    LRESULT(match hit {
        Hit::Client => HTCLIENT,
        Hit::Caption => HTCAPTION,
        Hit::Minimize => HTMINBUTTON,
        Hit::Maximize => HTMAXBUTTON,
        Hit::Close => HTCLOSE,
        Hit::Top => HTTOP,
        Hit::Bottom => HTBOTTOM,
        Hit::Left => HTLEFT,
        Hit::Right => HTRIGHT,
        Hit::TopLeft => HTTOPLEFT,
        Hit::TopRight => HTTOPRIGHT,
        Hit::BottomLeft => HTBOTTOMLEFT,
        Hit::BottomRight => HTBOTTOMRIGHT,
    } as isize)
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCALCSIZE => {
            if let Some(r) = on_nccalcsize(hwnd, wparam, lparam) {
                return r;
            }
        }
        WM_NCHITTEST => return on_nchittest(hwnd, lparam),
        WM_DPICHANGED => {
            // lparam points at the rectangle Windows suggests for the new scale.
            let r = &*(lparam.0 as *const RECT);
            let _ = SetWindowPos(
                hwnd,
                None,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            // Placeholder surface until the Direct2D renderer lands.
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let brush = HBRUSH(GetStockObject(WHITE_BRUSH).0);
            FillRect(hdc, &ps.rcPaint, brush);
            let _ = EndPaint(hwnd, &ps);
            return LRESULT(0);
        }
        WM_DESTROY => {
            WINDOW.with(|w| {
                if w.get() == Some(hwnd) {
                    w.set(None);
                }
            });
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
