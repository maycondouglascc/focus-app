//! The main window: created on `open`, fully destroyed on `close` or when the
//! user closes it. Custom frame (see `frame.rs`), no standard non-client area,
//! drawn through a DirectComposition swap chain (see `render.rs`).

use std::cell::RefCell;

use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, ScreenToClient, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TME_NONCLIENT,
    TRACKMOUSEEVENT, VIRTUAL_KEY, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT,
    VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::Input::Pointer::{
    GetPointerInfo, POINTER_FLAG_FIRSTBUTTON, POINTER_FLAG_SECONDBUTTON, POINTER_INFO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, IsWindow,
    IsZoomed, LoadCursorW, PeekMessageW, RegisterClassW, SetForegroundWindow, SetWindowPos,
    ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, HTBOTTOM, HTBOTTOMLEFT,
    HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTCLOSE, HTLEFT, HTMAXBUTTON, HTMINBUTTON, HTRIGHT, HTTOP,
    HTTOPLEFT, HTTOPRIGHT, IDC_ARROW, MSG, NCCALCSIZE_PARAMS, PM_REMOVE, PT_PEN, PT_TOUCH,
    SM_CXPADDEDBORDER, SM_CXSIZEFRAME, SM_CYSIZEFRAME, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, SW_SHOW,
    WINDOW_EX_STYLE, WM_CAPTURECHANGED, WM_CHAR, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND,
    WM_KEYDOWN, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCLBUTTONDOWN,
    WM_NCLBUTTONUP, WM_NCMOUSEMOVE, WM_PAINT, WM_POINTERCAPTURECHANGED, WM_POINTERDOWN,
    WM_POINTERENTER, WM_POINTERHWHEEL, WM_POINTERLEAVE, WM_POINTERUP, WM_POINTERUPDATE,
    WM_POINTERWHEEL, WM_SETTINGCHANGE, WM_SIZE, WM_SYSCOLORCHANGE, WM_SYSKEYDOWN, WM_THEMECHANGED,
    WNDCLASSW, WS_CLIPCHILDREN, WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW,
};

use super::focus::{EscapeOutcome, FocusModel};
use super::frame::{self, Hit};
use super::input::{KeyCode, KeyEvent, PointerButton, PointerKind, PointerState};
use super::prefs::{self, SystemPrefs};
use super::render::{reset_shared, DrawError, FrameOpts, Hover, Renderer};
use super::theme::Palette;
use super::tree::Tree;

/// `WM_NCMOUSELEAVE`
const WM_NCMOUSELEAVE: u32 = 0x02A2;
/// `WM_MOUSELEAVE`
const WM_MOUSELEAVE: u32 = 0x02A3;

struct Ui {
    hwnd: HWND,
    /// `None` after a device loss or a failed creation; rebuilt on the next paint.
    renderer: Option<Renderer>,
    hover: Hover,
    prefs: SystemPrefs,
    palette: Palette,
    mica: bool,
    tree: Option<Tree>,
    focus: FocusModel,
    pointer: PointerState,
    paint_count: u64,
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
        reset_shared();
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
        WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
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
    let sys_prefs = SystemPrefs::query();
    let mica = prefs::apply_mica(hwnd, sys_prefs.high_contrast);
    let palette = Palette::for_system(sys_prefs.high_contrast);
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
        Ok(r) => {
            r.set_text_scale(sys_prefs.text_scale);
            Some(r)
        }
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
            prefs: sys_prefs,
            palette,
            mica,
            tree: None,
            focus: FocusModel::new(),
            pointer: PointerState::default(),
            paint_count: 0,
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

unsafe fn query_pointer(
    hwnd: HWND,
    wparam: WPARAM,
) -> Option<(u32, PointerKind, PointerButton, f32, f32)> {
    let pointer_id = (wparam.0 & 0xFFFF) as u32;
    let mut info = POINTER_INFO::default();
    GetPointerInfo(pointer_id, &mut info).ok()?;
    let mut pt = info.ptPixelLocation;
    let _ = ScreenToClient(hwnd, &mut pt);
    let scale = frame::scale(GetDpiForWindow(hwnd));
    let kind = match info.pointerType {
        PT_TOUCH => PointerKind::Touch,
        PT_PEN => PointerKind::Pen,
        _ => PointerKind::Mouse,
    };
    let button = if (info.pointerFlags.0 & POINTER_FLAG_FIRSTBUTTON.0) != 0 {
        PointerButton::Primary
    } else if (info.pointerFlags.0 & POINTER_FLAG_SECONDBUTTON.0) != 0 {
        PointerButton::Secondary
    } else {
        PointerButton::Other
    };
    Some((
        pointer_id,
        kind,
        button,
        pt.x as f32 / scale,
        pt.y as f32 / scale,
    ))
}

unsafe fn track_client_leave(hwnd: HWND) {
    let mut t = TRACKMOUSEEVENT {
        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    let _ = TrackMouseEvent(&mut t);
}

unsafe fn on_pointer_msg(hwnd: HWND, msg: u32, wparam: WPARAM) -> bool {
    match msg {
        WM_POINTERENTER | WM_POINTERUPDATE => {
            let Some((_, kind, _, x, y)) = query_pointer(hwnd, wparam) else {
                return false;
            };
            if kind == PointerKind::Mouse {
                track_client_leave(hwnd);
            }
            let changed = with_ui(|u| {
                let hit = u.tree.as_ref().and_then(|t| t.hit_test(x, y));
                u.pointer.on_move(hit, x, y).visual_changed
            })
            .unwrap_or(false);
            if changed {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            true
        }
        WM_POINTERDOWN => {
            let Some((pid, kind, btn, x, y)) = query_pointer(hwnd, wparam) else {
                return false;
            };
            let outcome = with_ui(|u| {
                let hit = u.tree.as_ref().and_then(|t| t.hit_test(x, y));
                let mut out = u.pointer.on_down(pid, btn, hit, x, y);
                let focus_changed = match hit {
                    Some(id) => u.focus.focus_pointer(id),
                    None => u.focus.clear_focus(),
                };
                out.visual_changed |= focus_changed;
                out
            })
            .unwrap_or_default();
            if outcome.request_capture && kind == PointerKind::Mouse {
                SetCapture(hwnd);
            }
            if outcome.visual_changed {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            true
        }
        WM_POINTERUP => {
            let Some((pid, _, _, x, y)) = query_pointer(hwnd, wparam) else {
                return false;
            };
            // On pointer-up, FIRSTBUTTON is cleared in pointerFlags; treat release as primary completion.
            let outcome = with_ui(|u| {
                let hit = u.tree.as_ref().and_then(|t| t.hit_test(x, y));
                u.pointer.on_up(pid, PointerButton::Primary, hit, x, y)
            })
            .unwrap_or_default();
            if outcome.release_capture {
                let _ = ReleaseCapture();
            }
            if outcome.visual_changed {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            true
        }
        WM_POINTERLEAVE | WM_MOUSELEAVE => {
            let changed = with_ui(|u| u.pointer.on_leave().visual_changed).unwrap_or(false);
            if changed {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            true
        }
        WM_POINTERCAPTURECHANGED | WM_CAPTURECHANGED => {
            let changed = with_ui(|u| u.pointer.on_capture_lost().visual_changed).unwrap_or(false);
            if changed {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            true
        }
        _ => false,
    }
}

unsafe fn on_wheel_msg(hwnd: HWND, msg: u32, wparam: WPARAM) -> bool {
    let delta = (wparam.0 >> 16) as i16;
    let horizontal = msg == WM_MOUSEHWHEEL || msg == WM_POINTERHWHEEL;
    let changed = with_ui(|u| {
        let dip = if horizontal {
            u.pointer.wheel.push_horizontal_dip(delta, 48.0)
        } else {
            u.pointer.wheel.push_vertical_dip(delta, 48.0)
        };
        let _ = dip;
        false
    })
    .unwrap_or(false);
    if changed {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    msg == WM_POINTERWHEEL || msg == WM_POINTERHWHEEL
}

fn translate_key(wparam: WPARAM) -> KeyEvent {
    let vk = VIRTUAL_KEY(wparam.0 as u16);
    let key = match vk {
        VK_TAB => KeyCode::Tab,
        VK_ESCAPE => KeyCode::Escape,
        VK_RETURN => KeyCode::Enter,
        VK_SPACE => KeyCode::Space,
        VK_LEFT => KeyCode::Left,
        VK_RIGHT => KeyCode::Right,
        VK_UP => KeyCode::Up,
        VK_DOWN => KeyCode::Down,
        VK_HOME => KeyCode::Home,
        VK_END => KeyCode::End,
        VK_PRIOR => KeyCode::PageUp,
        VK_NEXT => KeyCode::PageDown,
        other => KeyCode::Other(other.0),
    };
    // SAFETY: `GetKeyState` queries the current thread's keyboard state.
    let (shift, ctrl, alt) = unsafe {
        (
            GetKeyState(VK_SHIFT.0 as i32) < 0,
            GetKeyState(VK_CONTROL.0 as i32) < 0,
            GetKeyState(VK_MENU.0 as i32) < 0,
        )
    };
    KeyEvent {
        key,
        shift,
        ctrl,
        alt,
    }
}

unsafe fn on_key_msg(hwnd: HWND, wparam: WPARAM) -> bool {
    let ev = translate_key(wparam);
    let (handled, changed) = with_ui(|u| match ev.key {
        KeyCode::Tab if !ev.ctrl && !ev.alt => {
            let prev = u.focus.focused();
            let prev_vis = u.focus.is_focus_visible();
            let next = u.focus.tab(ev.shift);
            (true, next != prev || u.focus.is_focus_visible() != prev_vis)
        }
        KeyCode::Escape => {
            let out = u.focus.on_escape();
            (out != EscapeOutcome::None, out != EscapeOutcome::None)
        }
        _ => (false, false),
    })
    .unwrap_or((false, false));
    if changed {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    handled
}

unsafe fn on_size(hwnd: HWND) {
    let (w, h) = client_size(hwnd);
    with_ui(|u| {
        if let Some(r) = u.renderer.as_mut() {
            match r.resize(w as u32, h as u32) {
                Ok(()) => {}
                // Rebuilt by the next paint.
                Err(_) => {
                    u.renderer = None;
                    reset_shared();
                }
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
    let scale = frame::scale(dpi);
    let retry = with_ui(|u| {
        if u.renderer.is_none() {
            u.renderer = Renderer::new(hwnd, w as u32, h as u32).ok();
            if let Some(r) = u.renderer.as_ref() {
                r.set_text_scale(u.prefs.text_scale);
            }
        }
        let hover = u.hover;
        let palette = u.palette;
        let mica = u.mica;
        let tree = u.tree.take();
        let focus_ring = tree.as_ref().and_then(|t| u.focus.focus_ring_rect(t));
        let res = match u.renderer.as_mut() {
            Some(r) => r.draw_frame(FrameOpts {
                dpi,
                w: w as f32 / scale,
                h: h as f32 / scale,
                hover,
                palette: &palette,
                mica,
                show_title_bar: true,
                tree: tree.as_ref(),
                focus_ring,
            }),
            None => return false,
        };
        u.tree = tree;
        match res {
            Ok(()) => {
                u.paint_count += 1;
                false
            }
            Err(DrawError::DeviceLost) => {
                u.renderer = None;
                reset_shared();
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

unsafe fn on_system_prefs_changed(hwnd: HWND) {
    let new_prefs = SystemPrefs::query();
    let changed = with_ui(|u| {
        let mica = prefs::apply_mica(hwnd, new_prefs.high_contrast);
        let palette = Palette::for_system(new_prefs.high_contrast);
        let text_changed = u
            .renderer
            .as_ref()
            .is_some_and(|r| r.set_text_scale(new_prefs.text_scale));
        let changed =
            u.prefs != new_prefs || u.mica != mica || u.palette != palette || text_changed;
        u.prefs = new_prefs;
        u.mica = mica;
        u.palette = palette;
        changed
    })
    .unwrap_or(false);
    if changed {
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
        WM_SETTINGCHANGE | WM_THEMECHANGED | WM_SYSCOLORCHANGE => {
            on_system_prefs_changed(hwnd);
        }
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
        WM_POINTERENTER
        | WM_POINTERLEAVE
        | WM_POINTERDOWN
        | WM_POINTERUPDATE
        | WM_POINTERUP
        | WM_POINTERCAPTURECHANGED
        | WM_CAPTURECHANGED
        | WM_MOUSELEAVE => {
            if on_pointer_msg(hwnd, msg, wparam) {
                return LRESULT(0);
            }
        }
        WM_POINTERWHEEL | WM_POINTERHWHEEL | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            if on_wheel_msg(hwnd, msg, wparam) {
                return LRESULT(0);
            }
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if on_key_msg(hwnd, wparam) {
                return LRESULT(0);
            }
        }
        WM_CHAR => {}
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
            reset_shared();
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::super::render::D2DERR_RECREATE_TARGET;
    use super::*;
    use windows::Win32::Graphics::Dxgi::DXGI_ERROR_DEVICE_REMOVED;

    fn snapshot() -> Option<(HWND, u64, u64)> {
        with_ui(|u| {
            let gen = u.renderer.as_ref().map(|r| r.generation()).unwrap_or(0);
            (u.hwnd, gen, u.paint_count)
        })
    }

    fn pump_until_painted(min_paints: u64) {
        for _ in 0..50 {
            pump();
            if snapshot().is_some_and(|(_, g, p)| g > 0 && p >= min_paints) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn device_loss_recreates_renderer_and_draws_again() {
        super::super::init();
        assert!(open());
        pump_until_painted(1);
        let (hwnd, gen0, paints0) = snapshot().expect("window opened");
        assert!(gen0 > 0);
        assert!(paints0 >= 1);

        for hr in [D2DERR_RECREATE_TARGET, DXGI_ERROR_DEVICE_REMOVED] {
            let (_, gen_before, paints_before) = snapshot().unwrap();
            with_ui(|u| {
                if let Some(r) = u.renderer.as_mut() {
                    r.inject_draw_error(hr);
                }
            });
            // SAFETY: `hwnd` is our test window owned by this thread.
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            pump_until_painted(paints_before + 1);
            let (_, gen_after, paints_after) = snapshot().unwrap();
            assert!(
                gen_after > gen_before,
                "expected renderer generation to increase after {hr:?}"
            );
            assert!(
                paints_after > paints_before,
                "expected redraw after recovery from {hr:?}"
            );
        }

        close();
        pump();
        reset_shared();
        assert!(!is_open());
    }
}
