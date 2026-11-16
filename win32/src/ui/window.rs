//! The main window: created on `open`, fully destroyed on `close` or when the
//! user closes it. Custom frame (see `frame.rs`), no standard non-client area,
//! drawn through a DirectComposition swap chain (see `render.rs`).

use std::cell::RefCell;

use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, ScreenToClient, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, IsWindow,
    IsZoomed, LoadCursorW, PeekMessageW, RegisterClassW, SetForegroundWindow, SetWindowPos,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, HTBOTTOM, HTBOTTOMLEFT,
    HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTCLOSE, HTLEFT, HTMAXBUTTON, HTMINBUTTON, HTRIGHT, HTTOP,
    HTTOPLEFT, HTTOPRIGHT, IDC_ARROW, MSG, NCCALCSIZE_PARAMS, PM_REMOVE, SM_CXPADDEDBORDER,
    SM_CXSIZEFRAME, SM_CYSIZEFRAME, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SWP_NOZORDER, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOW, WINDOW_EX_STYLE, WM_DESTROY,
    WM_DPICHANGED, WM_ERASEBKGND, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP,
    WM_NCMOUSEMOVE, WM_PAINT, WM_SIZE, WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW,
};

use super::frame::{self, Hit};
use super::render::{DrawError, Hover, Renderer};

/// `WM_NCMOUSELEAVE`
const WM_NCMOUSELEAVE: u32 = 0x02A2;

struct Ui {
    hwnd: HWND,
    /// `None` after a device loss or a failed creation; rebuilt on the next paint.
    renderer: Option<Renderer>,
    hover: Hover,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

fn with_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|u| u.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn current() -> Option<HWND> {
    UI.with(|u| u.try_borrow().ok().and_then(|g| g.as_ref().map(|u| u.hwnd)))
}

pub fn is_open() -> bool {
    // SAFETY: IsWindow accepts any handle value.
    current().is_some_and(|h| unsafe { IsWindow(Some(h)).as_bool() })
}

/// Opens the window, or brings the existing one to the front.
pub fn open() -> bool {
    if let Some(h) = current() {
        // SAFETY: `h` is a window of this thread; failure is harmless.
        if unsafe { IsWindow(Some(h)).as_bool() } {
            unsafe {
                let _ = SetForegroundWindow(h);
            }
            return true;
        }
    }
    // SAFETY: called on the UI thread; every handle comes from the calls below.
    unsafe { create().is_some() }
}

/// Destroys the window and every graphics resource if it is open.
pub fn close() {
    // Take the state out first: dropping the renderer releases the swap chain,
    // and WM_DESTROY must not find the state borrowed.
    let ui = UI.with(|u| u.borrow_mut().take());
    if let Some(ui) = ui {
        let hwnd = ui.hwnd;
        drop(ui);
        // SAFETY: `hwnd` is ours; DestroyWindow on a dead handle just fails.
        unsafe {
            let _ = DestroyWindow(hwnd);
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

/// Client size in physical pixels.
unsafe fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    (rc.right - rc.left, rc.bottom - rc.top)
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
        // No redirection bitmap: the content comes from DirectComposition.
        WINDOW_EX_STYLE(WS_EX_NOREDIRECTIONBITMAP.0),
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
    let (w, h) = client_size(hwnd);
    let renderer = match Renderer::new(hwnd, w as u32, h as u32) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("focus: cannot create the renderer: {e}");
            None
        }
    };
    UI.with(|u| {
        *u.borrow_mut() = Some(Ui {
            hwnd,
            renderer,
            hover: Hover::None,
        })
    });
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
    let _ = ScreenToClient(hwnd, &mut pt);
    let (w, h) = client_size(hwnd);
    let hit = frame::hit_test(
        pt.x,
        pt.y,
        w,
        h,
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

fn hover_for(hit: u32) -> Hover {
    match hit {
        HTMINBUTTON => Hover::Minimize,
        HTMAXBUTTON => Hover::Maximize,
        HTCLOSE => Hover::Close,
        _ => Hover::None,
    }
}

unsafe fn set_hover(hwnd: HWND, hover: Hover) {
    let changed = with_ui(|u| std::mem::replace(&mut u.hover, hover) != hover).unwrap_or(false);
    if changed {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

unsafe fn on_size(hwnd: HWND) {
    let (w, h) = client_size(hwnd);
    with_ui(|u| {
        if let Some(r) = u.renderer.as_mut() {
            match r.resize(w as u32, h as u32) {
                Ok(()) => {}
                // Rebuilt by the next paint.
                Err(_) => u.renderer = None,
            }
        }
    });
    let _ = InvalidateRect(Some(hwnd), None, false);
}

unsafe fn on_paint(hwnd: HWND) {
    // Validates the update region; the content comes from DirectComposition.
    let mut ps = PAINTSTRUCT::default();
    BeginPaint(hwnd, &mut ps);
    let _ = EndPaint(hwnd, &ps);

    let dpi = GetDpiForWindow(hwnd);
    let (w, h) = client_size(hwnd);
    let retry = with_ui(|u| {
        if u.renderer.is_none() {
            u.renderer = Renderer::new(hwnd, w as u32, h as u32).ok();
        }
        let hover = u.hover;
        let Some(r) = u.renderer.as_mut() else {
            return false;
        };
        match r.draw(dpi, w as f32 * 96.0 / dpi as f32, hover) {
            Ok(()) => false,
            Err(DrawError::DeviceLost) => {
                u.renderer = None;
                true
            }
            Err(DrawError::Other(e)) => {
                eprintln!("focus: draw failed: {e}");
                false
            }
        }
    })
    .unwrap_or(false);
    if retry {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
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
        WM_NCMOUSEMOVE => {
            let mut t = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE | TME_NONCLIENT,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut t);
            set_hover(hwnd, hover_for(wparam.0 as u32));
        }
        WM_NCMOUSELEAVE => set_hover(hwnd, Hover::None),
        // The caption buttons are drawn by us: act on the release, and keep the
        // default handler from running its own tracking loop on press.
        WM_NCLBUTTONDOWN if hover_for(wparam.0 as u32) != Hover::None => return LRESULT(0),
        WM_NCLBUTTONUP => match wparam.0 as u32 {
            HTCLOSE => {
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            }
            HTMINBUTTON => {
                let _ = ShowWindow(hwnd, SW_MINIMIZE);
                return LRESULT(0);
            }
            HTMAXBUTTON => {
                let _ = ShowWindow(
                    hwnd,
                    if IsZoomed(hwnd).as_bool() {
                        SW_RESTORE
                    } else {
                        SW_MAXIMIZE
                    },
                );
                return LRESULT(0);
            }
            _ => {}
        },
        WM_SIZE => on_size(hwnd),
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
            on_paint(hwnd);
            return LRESULT(0);
        }
        WM_DESTROY => {
            // Closed by the user (or by `close`, which already took the state):
            // drop the renderer with the window.
            let ui = UI.with(|u| {
                let mut g = u.borrow_mut();
                if g.as_ref().is_some_and(|u| u.hwnd == hwnd) {
                    g.take()
                } else {
                    None
                }
            });
            drop(ui);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
