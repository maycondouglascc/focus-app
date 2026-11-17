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
    WM_GETOBJECT, WM_KEYDOWN, WM_KILLFOCUS, WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_NCCALCSIZE,
    WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSEMOVE, WM_PAINT,
    WM_POINTERCAPTURECHANGED, WM_POINTERDOWN, WM_POINTERENTER, WM_POINTERHWHEEL, WM_POINTERLEAVE,
    WM_POINTERUP, WM_POINTERUPDATE, WM_POINTERWHEEL, WM_SETFOCUS, WM_SETTINGCHANGE, WM_SIZE,
    WM_SYSCOLORCHANGE, WM_SYSKEYDOWN, WM_THEMECHANGED, WNDCLASSW, WS_CLIPCHILDREN,
    WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW,
};

use super::a11y::{self, A11yActionQueue, StaticActivation, WM_A11Y_ACTION};
use super::focus::{EscapeOutcome, FocusModel};
use super::frame::{self, Hit};
use super::input::{KeyCode, KeyEvent, PointerButton, PointerKind, PointerState};
use super::prefs::{self, SystemPrefs};
use super::render::{reset_shared, DrawError, FrameOpts, Hover, Renderer};
use super::screen::{Screen, ScreenEnv, ScreenHost};
use super::strings;
use super::theme::Palette;
use super::tree::Tree;
use super::widgets::text_field::{RichEditField, WM_RICHEDIT_BLUR, WM_RICHEDIT_FOCUS};

use accesskit::Action;
use accesskit_windows::Adapter;

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
    screen: Option<ScreenHost>,
    tree: Option<Tree>,
    focus: FocusModel,
    pointer: PointerState,
    a11y_queue: A11yActionQueue,
    a11y: Adapter,
    rich_fields: Vec<RichEditField>,
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

#[allow(dead_code)] // used by Select popups, RichEdit screens, and tests
pub fn hwnd() -> Option<HWND> {
    current()
}

/// Mounts a retained screen onto the open window and schedules a single paint.
#[allow(dead_code)] // wired to F3 screens and verified by F2 gallery tests
pub fn set_screen(screen: Box<dyn Screen>) -> bool {
    let Some(hwnd) = current() else {
        return false;
    };
    // SAFETY: `hwnd` is owned by this thread.
    let (dpi, (w, h)) = unsafe { (GetDpiForWindow(hwnd), client_size(hwnd)) };
    let scale = frame::scale(dpi);
    let mounted = with_ui(|u| {
        let Some(text) = u.renderer.as_ref().map(|r| r.text()) else {
            return false;
        };
        let env = ScreenEnv {
            width_dip: w as f32 / scale,
            height_dip: h as f32 / scale,
            hover: u.pointer.hovered,
            pressed: u.pointer.pressed,
            focused: u.focus.focused(),
            palette: &u.palette,
            text,
        };
        let host = ScreenHost::mount(screen, &env, &mut u.focus);
        u.screen = Some(host);
        sync_rich_fields(u);
        true
    })
    .unwrap_or(false);
    if mounted {
        // SAFETY: `hwnd` is live on this thread.
        unsafe {
            sync_a11y(hwnd);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
    mounted
}

fn active_tree(ui: &Ui) -> Option<&Tree> {
    ui.screen.as_ref().map(|h| h.tree()).or(ui.tree.as_ref())
}

fn rebuild_screen_if_dirty(ui: &mut Ui, hwnd: HWND) -> bool {
    let Some(host) = ui.screen.as_mut() else {
        return false;
    };
    let Some(text) = ui.renderer.as_ref().map(|r| r.text()) else {
        return false;
    };
    // SAFETY: `hwnd` is owned by this thread.
    let (dpi, (w, h)) = unsafe { (GetDpiForWindow(hwnd), client_size(hwnd)) };
    let scale = frame::scale(dpi);
    let env = ScreenEnv {
        width_dip: w as f32 / scale,
        height_dip: h as f32 / scale,
        hover: ui.pointer.hovered,
        pressed: ui.pointer.pressed,
        focused: ui.focus.focused(),
        palette: &ui.palette,
        text,
    };
    let rebuilt = host.rebuild_if_dirty(&env, &mut ui.focus);
    if rebuilt {
        sync_rich_fields(ui);
    }
    rebuilt
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
    super::popup::close();
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
    // Initialize AccessKit before `ShowWindow` so initial `WM_GETOBJECT` never races.
    let a11y_queue = A11yActionQueue::new(hwnd);
    let a11y = Adapter::new(hwnd, false, a11y_queue.handler());
    UI.with(|u| {
        *u.borrow_mut() = Some(Ui {
            hwnd,
            renderer,
            hover: Hover::None,
            prefs: sys_prefs,
            palette,
            mica,
            screen: None,
            tree: None,
            focus: FocusModel::new(),
            pointer: PointerState::default(),
            a11y_queue,
            a11y,
            rich_fields: Vec::new(),
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
                let hit = active_tree(u).and_then(|t| t.hit_test(x, y));
                let dirty = u.pointer.on_move(hit, x, y).visual_changed;
                if dirty {
                    if let Some(h) = u.screen.as_mut() {
                        h.mark_dirty();
                        rebuild_screen_if_dirty(u, hwnd);
                    }
                }
                dirty
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
            let (request_capture, focus_changed, visual_changed) = with_ui(|u| {
                let hit = active_tree(u).and_then(|t| t.hit_test(x, y));
                let out = u.pointer.on_down(pid, btn, hit, x, y);
                let focus_changed = match hit {
                    Some(id) => u.focus.focus_pointer(id),
                    None => u.focus.clear_focus(),
                };
                let dirty = out.visual_changed || focus_changed;
                if dirty {
                    if let Some(h) = u.screen.as_mut() {
                        h.mark_dirty();
                        rebuild_screen_if_dirty(u, hwnd);
                    }
                }
                (out.request_capture, focus_changed, dirty)
            })
            .unwrap_or((false, false, false));
            if request_capture && kind == PointerKind::Mouse {
                SetCapture(hwnd);
            }
            if focus_changed {
                sync_a11y(hwnd);
            }
            if visual_changed {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            true
        }
        WM_POINTERUP => {
            let Some((pid, _, _, x, y)) = query_pointer(hwnd, wparam) else {
                return false;
            };
            // On pointer-up, FIRSTBUTTON is cleared in pointerFlags; treat release as primary completion.
            let (release_capture, tree_changed, visual_changed) = with_ui(|u| {
                let hit = active_tree(u).and_then(|t| t.hit_test(x, y));
                let out = u.pointer.on_up(pid, PointerButton::Primary, hit, x, y);
                let mut tree_changed = false;
                if let Some(id) = out.clicked {
                    if let Some(h) = u.screen.as_mut() {
                        tree_changed = h.on_click(id);
                    }
                }
                let dirty = out.visual_changed || tree_changed;
                if dirty {
                    if let Some(h) = u.screen.as_mut() {
                        h.mark_dirty();
                        rebuild_screen_if_dirty(u, hwnd);
                    }
                }
                (out.release_capture, tree_changed, dirty)
            })
            .unwrap_or((false, false, false));
            if release_capture {
                let _ = ReleaseCapture();
            }
            if tree_changed {
                sync_a11y(hwnd);
            }
            if visual_changed {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            true
        }
        WM_POINTERLEAVE | WM_MOUSELEAVE => {
            let changed = with_ui(|u| {
                let dirty = u.pointer.on_leave().visual_changed;
                if dirty {
                    if let Some(h) = u.screen.as_mut() {
                        h.mark_dirty();
                        rebuild_screen_if_dirty(u, hwnd);
                    }
                }
                dirty
            })
            .unwrap_or(false);
            if changed {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            true
        }
        WM_POINTERCAPTURECHANGED | WM_CAPTURECHANGED => {
            let changed = with_ui(|u| {
                let dirty = u.pointer.on_capture_lost().visual_changed;
                if dirty {
                    if let Some(h) = u.screen.as_mut() {
                        h.mark_dirty();
                        rebuild_screen_if_dirty(u, hwnd);
                    }
                }
                dirty
            })
            .unwrap_or(false);
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
    let (scrolled, changed) = with_ui(|u| {
        let dip = if horizontal {
            u.pointer.wheel.push_horizontal_dip(delta, 48.0)
        } else {
            u.pointer.wheel.push_vertical_dip(delta, 48.0)
        };
        if !horizontal && dip.abs() > 0.01 {
            let dirty = u.screen.as_mut().is_some_and(|h| h.on_wheel(dip));
            if dirty {
                rebuild_screen_if_dirty(u, hwnd);
                return (true, true);
            }
        }
        (false, false)
    })
    .unwrap_or((false, false));
    if scrolled {
        sync_a11y(hwnd);
    }
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

unsafe fn sync_a11y(hwnd: HWND) {
    let dpi = GetDpiForWindow(hwnd);
    let client_px = client_size(hwnd);
    let scale = frame::scale(dpi);
    let queued = UI.with(|u| {
        let Ok(mut g) = u.try_borrow_mut() else {
            return None;
        };
        let ui = g.as_mut()?;
        let focused = ui.focus.focused();
        let update = a11y::build_tree_update(
            active_tree(ui),
            focused,
            scale,
            client_px,
            strings::APP_NAME,
        );
        ui.a11y.update_if_active(|| update)
    });
    // Raise outside any borrow of `UI` because UIA may synchronously send `WM_GETOBJECT`.
    if let Some(ev) = queued {
        ev.raise();
    }
}

unsafe fn on_a11y_action_msg(hwnd: HWND) {
    let requests = UI
        .with(|u| {
            u.try_borrow()
                .ok()
                .and_then(|g| g.as_ref().map(|ui| ui.a11y_queue.drain()))
        })
        .unwrap_or_default();
    if requests.is_empty() {
        return;
    }
    let mut focus_changed = false;
    let mut visual_changed = false;
    with_ui(|u| {
        for req in requests {
            let target = a11y::widget_id(req.target_node);
            match req.action {
                Action::Focus => {
                    focus_changed |= u.focus.focus_keyboard(target);
                    visual_changed = true;
                }
                Action::Click => {
                    if active_tree(u)
                        .and_then(|t| t.node_by_id(target).and_then(|n| t.data(n)))
                        .is_some_and(|d| d.focusable && !d.disabled)
                    {
                        focus_changed |= u.focus.focus_keyboard(target);
                    }
                    if let Some(h) = u.screen.as_mut() {
                        if h.on_click(target) {
                            rebuild_screen_if_dirty(u, hwnd);
                            focus_changed = true;
                        }
                    }
                    visual_changed = true;
                }
                _ => {}
            }
        }
    });
    if focus_changed {
        sync_a11y(hwnd);
    }
    if visual_changed {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

unsafe fn on_key_msg(hwnd: HWND, wparam: WPARAM) -> bool {
    let ev = translate_key(wparam);
    let (handled, focus_changed, changed) = with_ui(|u| match ev.key {
        KeyCode::Tab if !ev.ctrl && !ev.alt => {
            let prev = u.focus.focused();
            let prev_vis = u.focus.is_focus_visible();
            let next = u.focus.tab(ev.shift);
            let dirty = next != prev || u.focus.is_focus_visible() != prev_vis;
            if dirty {
                if let Some(h) = u.screen.as_mut() {
                    h.mark_dirty();
                    rebuild_screen_if_dirty(u, hwnd);
                }
            }
            (true, next != prev, dirty)
        }
        KeyCode::Escape => {
            if let Some(h) = u.screen.as_mut() {
                let prev = u.focus.focused();
                let (handled, dirty) = h.on_key(ev, &mut u.focus);
                if dirty {
                    rebuild_screen_if_dirty(u, hwnd);
                    return (handled, u.focus.focused() != prev, true);
                }
            }
            let prev = u.focus.focused();
            let out = u.focus.on_escape();
            let changed = out != EscapeOutcome::None;
            (changed, u.focus.focused() != prev, changed)
        }
        _ => {
            let prev = u.focus.focused();
            if let Some(h) = u.screen.as_mut() {
                let (handled, dirty) = h.on_key(ev, &mut u.focus);
                if dirty {
                    rebuild_screen_if_dirty(u, hwnd);
                }
                return (handled, u.focus.focused() != prev, dirty);
            }
            (false, false, false)
        }
    })
    .unwrap_or((false, false, false));
    if focus_changed {
        sync_a11y(hwnd);
    }
    if changed {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    handled
}

fn sync_rich_fields(ui: &Ui) {
    let Some(tree) = active_tree(ui) else {
        return;
    };
    let slots = tree.rich_text_slots();
    for field in &ui.rich_fields {
        if let Some((_, rect, hidden)) = slots.iter().find(|(id, _, _)| *id == field.slot_id()) {
            field.sync_bounds(*rect, *hidden);
        }
    }
}

unsafe fn on_size(hwnd: HWND) {
    let (w, h) = client_size(hwnd);
    let dpi = GetDpiForWindow(hwnd);
    let scale = frame::scale(dpi);
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
        if let Some(r) = u.renderer.as_ref() {
            let w_dip = w as f32 / scale;
            let h_dip = h as f32 / scale;
            if let Some(host) = u.screen.as_mut() {
                host.relayout(w_dip, h_dip, r.text());
            } else if let Some(tree) = u.tree.as_mut() {
                tree.layout(w_dip, h_dip, r.text());
            }
        }
        sync_rich_fields(u);
    });
    sync_a11y(hwnd);
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
        let tree_ref = u.screen.as_ref().map(|h| h.tree()).or(u.tree.as_ref());
        let focus_ring = tree_ref.and_then(|t| u.focus.focus_ring_rect(t));
        let res = match u.renderer.as_mut() {
            Some(r) => r.draw_frame(FrameOpts {
                dpi,
                w: w as f32 / scale,
                h: h as f32 / scale,
                hover,
                palette: &palette,
                mica,
                show_title_bar: true,
                tree: tree_ref,
                focus_ring,
            }),
            None => return false,
        };
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
    let dpi = GetDpiForWindow(hwnd);
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
        for field in &mut u.rich_fields {
            field.sync_theme(&palette, dpi, new_prefs.text_scale);
        }
        if changed {
            if let Some(h) = u.screen.as_mut() {
                h.mark_dirty();
                rebuild_screen_if_dirty(u, hwnd);
            }
        }
        changed
    })
    .unwrap_or(false);
    if changed {
        sync_a11y(hwnd);
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
        WM_GETOBJECT => {
            let dpi = GetDpiForWindow(hwnd);
            let client_px = client_size(hwnd);
            let scale = frame::scale(dpi);
            let result = UI.with(|u| {
                let Ok(mut g) = u.try_borrow_mut() else {
                    return None;
                };
                let ui = g.as_mut()?;
                let update = a11y::build_tree_update(
                    active_tree(ui),
                    ui.focus.focused(),
                    scale,
                    client_px,
                    strings::APP_NAME,
                );
                let mut activation = StaticActivation(Some(update));
                ui.a11y.handle_wm_getobject(wparam, lparam, &mut activation)
            });
            if let Some(res) = result {
                // Convert outside `UI` borrow because `UiaReturnRawElementProvider` can trigger nested `WM_GETOBJECT`.
                return res.into();
            }
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            let is_focused = msg == WM_SETFOCUS;
            let queued = UI.with(|u| {
                let Ok(mut g) = u.try_borrow_mut() else {
                    return None;
                };
                let ui = g.as_mut()?;
                ui.a11y.update_window_focus_state(is_focused)
            });
            if let Some(ev) = queued {
                ev.raise();
            }
        }
        m if m == WM_A11Y_ACTION => {
            on_a11y_action_msg(hwnd);
            return LRESULT(0);
        }
        m if m == WM_RICHEDIT_FOCUS || m == WM_RICHEDIT_BLUR => {
            let child = HWND(wparam.0 as *mut core::ffi::c_void);
            let focused_slot = with_ui(|u| {
                u.rich_fields
                    .iter()
                    .find(|f| f.hwnd() == child)
                    .map(|f| f.slot_id())
            })
            .flatten();
            if let Some(slot_id) = focused_slot {
                let changed = with_ui(|u| {
                    if m == WM_RICHEDIT_FOCUS {
                        u.focus.focus_pointer(slot_id)
                    } else if u.focus.focused() == Some(slot_id) {
                        u.focus.clear_focus()
                    } else {
                        false
                    }
                })
                .unwrap_or(false);
                if changed {
                    sync_a11y(hwnd);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            return LRESULT(0);
        }
        WM_PAINT => {
            on_paint(hwnd);
            return LRESULT(0);
        }
        WM_DESTROY => {
            super::popup::close();
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
    use super::super::focus::FocusScope;
    use super::super::render::D2DERR_RECREATE_TARGET;
    use super::super::text::TextSystem;
    use super::super::tree::{style, A11yInfo, A11yRole, HitMode, Leaf};
    use super::*;
    use windows::Win32::Graphics::Dxgi::DXGI_ERROR_DEVICE_REMOVED;
    use windows::Win32::UI::Accessibility::UiaRootObjectId;
    use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

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

    #[test]
    fn wm_getobject_and_tree_updates_expose_valid_uia_tree() {
        super::super::init();
        assert!(open());
        pump_until_painted(1);

        let text = TextSystem::new().expect("dwrite");
        let mut tree = Tree::new(style::column());
        let btn = tree.add_leaf(
            tree.root(),
            taffy::Style {
                size: style::size(100.0, 32.0),
                ..Default::default()
            },
            Leaf::Box,
        );
        tree.set_hit(btn, HitMode::Interactive);
        tree.set_focusable(btn, true);
        tree.set_a11y(
            btn,
            A11yInfo {
                role: A11yRole::Button,
                name: "Play".into(),
                clickable: true,
                requires_name: true,
                ..Default::default()
            },
        );
        tree.layout(400.0, 300.0, &text);
        let btn_id = tree.id(btn);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));

        let hwnd = with_ui(|u| {
            u.focus.sync_from_tree(&tree, |_, _| FocusScope::Main);
            u.tree = Some(tree);
            u.hwnd
        })
        .expect("ui open");

        // SAFETY: `hwnd` is our test window on the UI thread; `UiaRootObjectId` requests the UIA root provider.
        let lres = unsafe {
            SendMessageW(
                hwnd,
                WM_GETOBJECT,
                Some(WPARAM(0)),
                Some(LPARAM(UiaRootObjectId as isize)),
            )
        };
        assert_ne!(lres.0, 0, "WM_GETOBJECT must return non-null UIA provider");

        // Trigger focus + tree sync after adapter activation.
        with_ui(|u| {
            assert!(u.focus.focus_keyboard(btn_id));
        });
        // SAFETY: `hwnd` is live on this thread.
        unsafe { sync_a11y(hwnd) };
        pump();

        close();
        pump();
        reset_shared();
        assert!(!is_open());
    }

    #[test]
    fn idle_window_with_screen_does_not_redraw_in_a_loop() {
        super::super::init();
        assert!(open());
        pump_until_painted(1);

        let gallery = super::super::screen::WidgetGalleryScreen::new();
        assert!(set_screen(Box::new(gallery)));
        let paints_before = snapshot().unwrap().2;
        pump_until_painted(paints_before + 1);
        let paints_after_mount = snapshot().unwrap().2;
        assert!(paints_after_mount > paints_before);

        // Idle period with no input/events: paint count must remain completely flat (0% idle CPU).
        std::thread::sleep(std::time::Duration::from_millis(60));
        pump();
        let paints_after_idle = snapshot().unwrap().2;
        assert_eq!(
            paints_after_idle, paints_after_mount,
            "idle window must not run a continuous redraw loop"
        );

        close();
        pump();
        reset_shared();
        assert!(!is_open());
    }
}
