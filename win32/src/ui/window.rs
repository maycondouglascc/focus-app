//! The main window: created on `open`, fully destroyed on `close` or when the
//! user closes it. Custom frame (see `frame.rs`), no standard non-client area,
//! drawn through a DirectComposition swap chain (see `render.rs`).

use std::cell::{Cell, RefCell};

use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, ScreenToClient, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetFocus, GetKeyState, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE,
    TME_NONCLIENT, TRACKMOUSEEVENT, VIRTUAL_KEY, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME,
    VK_LEFT, VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::Input::Pointer::{
    GetPointerInfo, POINTER_FLAG_FIRSTBUTTON, POINTER_FLAG_SECONDBUTTON, POINTER_INFO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, IsWindow,
    IsZoomed, KillTimer, LoadCursorW, PeekMessageW, RegisterClassW, SetForegroundWindow, SetTimer,
    SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, HTBOTTOM,
    HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTCLOSE, HTLEFT, HTMAXBUTTON, HTMINBUTTON,
    HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, IDC_ARROW, MSG, NCCALCSIZE_PARAMS, PM_REMOVE, PT_PEN,
    PT_TOUCH, SIZE_MINIMIZED, SM_CXPADDEDBORDER, SM_CXSIZEFRAME, SM_CYSIZEFRAME, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE,
    SW_SHOW, WINDOW_EX_STYLE, WM_CAPTURECHANGED, WM_CHAR, WM_COMMAND, WM_DESTROY, WM_DPICHANGED,
    WM_ERASEBKGND, WM_GETOBJECT, WM_KEYDOWN, WM_KILLFOCUS, WM_MOUSEHWHEEL, WM_MOUSEWHEEL,
    WM_NCCALCSIZE, WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMOUSEMOVE, WM_PAINT,
    WM_POINTERCAPTURECHANGED, WM_POINTERDOWN, WM_POINTERENTER, WM_POINTERHWHEEL, WM_POINTERLEAVE,
    WM_POINTERUP, WM_POINTERUPDATE, WM_POINTERWHEEL, WM_SETFOCUS, WM_SETTINGCHANGE, WM_SIZE,
    WM_SYSCOLORCHANGE, WM_SYSKEYDOWN, WM_THEMECHANGED, WM_TIMER, WNDCLASSW, WS_CLIPCHILDREN,
    WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW,
};

use super::a11y::{self, A11yActionQueue, StaticActivation, WM_A11Y_ACTION};
use super::focus::{EscapeOutcome, FocusModel};
use super::frame::{self, Hit};
use super::input::{KeyCode, KeyEvent, PointerButton, PointerKind, PointerState};
use super::popup::WM_POPUP_NOTIFY;
use super::prefs::{self, SystemPrefs};
use super::render::{reset_shared, DrawError, FrameOpts, Hover, Renderer};
use super::screen::{KeyModifiers, Screen, ScreenEnv, ScreenHost};
use super::strings;
use super::theme::Palette;
use super::tree::{Rect, Tree, WidgetId};
use super::widgets::text_field::{
    RichEditField, WM_RICHEDIT_BLUR, WM_RICHEDIT_FOCUS, WM_RICHEDIT_SUBMIT,
};

use accesskit::Action;
use accesskit_windows::Adapter;

/// `WM_NCMOUSELEAVE`
const WM_NCMOUSELEAVE: u32 = 0x02A2;
/// `WM_MOUSELEAVE`
const WM_MOUSELEAVE: u32 = 0x02A3;
const EN_CHANGE: u16 = 0x0300;
const TIMER_TICK_ID: usize = 1;
const TIMER_AUTOSCROLL_ID: usize = 2;

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
    screen_capturing: bool,
    tick_timer_on: bool,
    autoscroll_timer_on: bool,
    syncing_rich: bool,
}

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    static CUR_HWND: Cell<isize> = const { Cell::new(0) };
    static RECT_CACHE: RefCell<Vec<(WidgetId, Rect)>> = const { RefCell::new(Vec::new()) };
}

fn with_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|u| u.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn current() -> Option<HWND> {
    let h = CUR_HWND.get();
    if h != 0 {
        return Some(HWND(h as *mut core::ffi::c_void));
    }
    UI.with(|u| u.try_borrow().ok().and_then(|g| g.as_ref().map(|u| u.hwnd)))
}

fn sync_rect_cache(tree: Option<&Tree>) {
    RECT_CACHE.with(|c| {
        let mut v = c.borrow_mut();
        v.clear();
        if let Some(t) = tree {
            for n in t.all_nodes() {
                v.push((t.id(n), t.rect(n)));
            }
        }
    });
}

#[allow(dead_code)] // used by Select popups, RichEdit screens, and tests
pub fn hwnd() -> Option<HWND> {
    current()
}

/// Returns the laid-out client DIP rectangle of a widget in the active tree, if mounted.
pub fn widget_rect(id: WidgetId) -> Option<Rect> {
    if let Some(res) = with_ui(|u| {
        let tree = active_tree(u)?;
        let node = tree.node_by_id(id)?;
        Some(tree.rect(node))
    }) {
        return res;
    }
    // Re-entrant lookup while `with_ui` holds `UI` borrowed mutably (e.g. `on_click`, `on_pointer_down`).
    RECT_CACHE.with(|c| {
        c.borrow()
            .iter()
            .find(|(wid, _)| *wid == id)
            .map(|(_, r)| *r)
    })
}

pub fn paint_count() -> u64 {
    UI.with(|u| {
        u.try_borrow()
            .ok()
            .and_then(|g| g.as_ref().map(|x| x.paint_count))
    })
    .unwrap_or(0)
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
        sync_rect_cache(Some(host.tree()));
        u.screen = Some(host);
        u.screen_capturing = false;
        sync_rich_fields(u);
        sync_rich_focus(u);
        sync_timers(u, hwnd, false);
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

/// Invokes a click on the first active clickable widget matching `name`.
pub fn click_by_name(name: &str) -> bool {
    let Some(hwnd) = current() else {
        return false;
    };
    let changed = with_ui(|u| {
        let target = active_tree(u).and_then(|t| {
            t.all_nodes().into_iter().find_map(|n| {
                let d = t.data(n)?;
                let matched = d.a11y.name == name
                    || (d.a11y.name.starts_with(name)
                        && d.a11y.name[name.len()..].starts_with(", "));
                (d.a11y.clickable && !d.disabled && matched).then_some(d.id)
            })
        })?;
        let h = u.screen.as_mut()?;
        if h.on_click(target) {
            rebuild_screen_if_dirty(u, hwnd);
            sync_timers(u, hwnd, false);
            Some(true)
        } else {
            Some(false)
        }
    })
    .flatten()
    .unwrap_or(false);
    if changed {
        // SAFETY: `hwnd` is live on this thread.
        unsafe {
            sync_a11y(hwnd);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
    changed
}

#[derive(Clone, Debug)]
pub struct DragBenchMetrics {
    pub frames: u32,
    pub res_w_px: u32,
    pub res_h_px: u32,
    pub dpi: u32,
    pub elapsed_ms: f64,
    pub avg_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    pub fps: f64,
}

/// Lays out the active Calendar screen at 1920x1080 DIPs (`200%` DPI -> `3840x2160` px),
/// executes `frames` continuous week-grid drag + `sync_drag_tree` + Direct2D draws without
/// vsync blocking, and reports timing percentiles.
pub fn bench_drag_4k(frames: u32) -> Result<DragBenchMetrics, String> {
    let setup = with_ui(|u| -> Result<(WidgetId, Rect), String> {
        let palette = u.palette;
        {
            let r = u
                .renderer
                .as_mut()
                .ok_or_else(|| "no renderer".to_string())?;
            r.resize(3840, 2160).map_err(|e| format!("{e:?}"))?;
            r.set_present_sync(false);
        }
        let text = u.renderer.as_ref().unwrap().text();
        let host = u
            .screen
            .as_mut()
            .ok_or_else(|| "no mounted screen".to_string())?;
        let env = ScreenEnv {
            width_dip: 1920.0,
            height_dip: 1080.0,
            hover: None,
            pressed: None,
            focused: None,
            palette: &palette,
            text,
        };
        host.mark_dirty();
        host.rebuild_if_dirty(&env, &mut u.focus);
        sync_rect_cache(Some(host.tree()));
        let t = host.tree();
        let node = t
            .all_nodes()
            .into_iter()
            .find(|&n| {
                t.data(n)
                    .is_some_and(|d| d.a11y.name == strings::CAL_GRID_LABEL)
            })
            .ok_or_else(|| "calendar grid canvas node not found".to_string())?;
        let canvas_id = t.id(node);
        let vp_rect = t
            .parent(node)
            .map(|p| t.rect(p))
            .unwrap_or_else(|| t.rect(node));
        Ok((canvas_id, vp_rect))
    })
    .ok_or_else(|| "ui unavailable".to_string())??;

    let (canvas_id, vp_rect) = setup;
    let start_x = vp_rect.x + 120.0;
    let start_y = vp_rect.y + 40.0;

    let (mut frame_ms, elapsed_ms) = with_ui(|u| -> Result<(Vec<f64>, f64), String> {
        let palette = u.palette;
        let host = u
            .screen
            .as_mut()
            .ok_or_else(|| "no mounted screen".to_string())?;
        let r = u
            .renderer
            .as_mut()
            .ok_or_else(|| "no renderer".to_string())?;
        let down = host.on_pointer_down(canvas_id, start_x, start_y, KeyModifiers::default());
        if !down.dragging {
            return Err("pointer_down did not start grid drag".into());
        }
        let _ = host.on_pointer_move(start_x + 20.0, start_y + 15.0, KeyModifiers::default());
        host.sync_drag_tree();
        r.draw_frame(FrameOpts {
            dpi: 192,
            w: 1920.0,
            h: 1080.0,
            hover: Hover::None,
            palette: &palette,
            mica: false,
            show_title_bar: true,
            tree: Some(host.tree()),
            focus_ring: None,
        })
        .map_err(|e| format!("{e:?}"))?;

        let mut samples = Vec::with_capacity(frames as usize);
        let t_all = std::time::Instant::now();
        for f in 0..frames {
            let tf = std::time::Instant::now();
            let y = vp_rect.y + 40.0 + ((f % 160) as f32) * 1.5;
            let x = vp_rect.x + 120.0 + (((f / 30) % 5) as f32) * 140.0;
            let _ = host.on_pointer_move(x, y, KeyModifiers::default());
            host.sync_drag_tree();
            r.draw_frame(FrameOpts {
                dpi: 192,
                w: 1920.0,
                h: 1080.0,
                hover: Hover::None,
                palette: &palette,
                mica: false,
                show_title_bar: true,
                tree: Some(host.tree()),
                focus_ring: None,
            })
            .map_err(|e| format!("{e:?}"))?;
            samples.push(tf.elapsed().as_secs_f64() * 1000.0);
        }
        let elapsed = t_all.elapsed().as_secs_f64() * 1000.0;
        let _ = host.on_pointer_cancel();
        r.set_present_sync(true);
        Ok((samples, elapsed))
    })
    .ok_or_else(|| "ui unavailable".to_string())??;

    frame_ms.sort_by(|a, b| a.total_cmp(b));
    let n = frame_ms.len().max(1);
    let avg_ms = elapsed_ms / (n as f64);
    let p95_idx = ((n * 95) / 100).min(n - 1);
    let p95_ms = frame_ms[p95_idx];
    let max_ms = frame_ms[n - 1];
    let fps = (n as f64) * 1000.0 / elapsed_ms.max(0.001);
    Ok(DragBenchMetrics {
        frames,
        res_w_px: 3840,
        res_h_px: 2160,
        dpi: 192,
        elapsed_ms,
        avg_ms,
        p95_ms,
        max_ms,
        fps,
    })
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
        sync_rect_cache(Some(host.tree()));
        sync_rich_fields(ui);
        sync_rich_focus(ui);
        sync_timers(ui, hwnd, false);
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
    CUR_HWND.set(0);
    RECT_CACHE.with(|c| c.borrow_mut().clear());
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
            screen_capturing: false,
            tick_timer_on: false,
            autoscroll_timer_on: false,
            syncing_rich: false,
        })
    });
    CUR_HWND.set(hwnd.0 as isize);
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
            let mods = current_modifiers();
            let (changed, sync_a11y_needed) = with_ui(|u| {
                let mut repaint = false;
                let mut a11y = false;
                if u.screen_capturing {
                    u.pointer.last_dip = Some((x, y));
                    let reply = u
                        .screen
                        .as_mut()
                        .map(|h| h.on_pointer_move(x, y, mods))
                        .unwrap_or_default();
                    u.focus.set_dragging(reply.dragging);
                    if reply.rebuild {
                        a11y |= rebuild_screen_if_dirty(u, hwnd);
                        repaint = true;
                    } else if reply.fast_redraw {
                        if let Some(h) = u.screen.as_mut() {
                            h.sync_drag_tree();
                        }
                        repaint = true;
                    }
                    sync_timers(u, hwnd, false);
                } else {
                    let hit = active_tree(u).and_then(|t| t.hit_test(x, y));
                    let dirty = u.pointer.on_move(hit, x, y).visual_changed;
                    if dirty {
                        if let Some(h) = u.screen.as_mut() {
                            h.mark_dirty();
                            rebuild_screen_if_dirty(u, hwnd);
                        }
                        repaint = true;
                    }
                }
                (repaint, a11y)
            })
            .unwrap_or((false, false));
            if sync_a11y_needed {
                sync_a11y(hwnd);
            }
            if changed {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            true
        }
        WM_POINTERDOWN => {
            let Some((pid, kind, btn, x, y)) = query_pointer(hwnd, wparam) else {
                return false;
            };
            let mods = current_modifiers();
            let (request_capture, focus_changed, visual_changed) = with_ui(|u| {
                let hit = active_tree(u).and_then(|t| t.hit_test(x, y));
                let out = u.pointer.on_down(pid, btn, hit, x, y);
                let focus_changed = match hit {
                    Some(id) => u.focus.focus_pointer(id),
                    None => u.focus.clear_focus(),
                };
                if focus_changed {
                    sync_rich_focus(u);
                }
                let mut screen_capture = false;
                let mut screen_dirty = false;
                if btn == PointerButton::Primary {
                    if let (Some(id), Some(h)) = (hit, u.screen.as_mut()) {
                        let reply = h.on_pointer_down(id, x, y, mods);
                        if reply.capture {
                            u.screen_capturing = true;
                            screen_capture = true;
                        }
                        u.focus.set_dragging(reply.dragging);
                        if reply.fast_redraw && !reply.rebuild {
                            h.sync_drag_tree();
                            screen_dirty = true;
                        }
                        screen_dirty |= reply.rebuild;
                    }
                }
                let dirty = out.visual_changed || focus_changed || screen_dirty;
                if out.visual_changed || focus_changed {
                    if let Some(h) = u.screen.as_mut() {
                        h.mark_dirty();
                    }
                }
                rebuild_screen_if_dirty(u, hwnd);
                sync_timers(u, hwnd, false);
                (out.request_capture || screen_capture, focus_changed, dirty)
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
            let Some((pid, _, btn, x, y)) = query_pointer(hwnd, wparam) else {
                return false;
            };
            let mods = current_modifiers();
            // On pointer-up, FIRSTBUTTON is cleared in pointerFlags; treat release as primary completion.
            let (release_capture, tree_changed, visual_changed) = with_ui(|u| {
                let hit = active_tree(u).and_then(|t| t.hit_test(x, y));
                let was_screen_cap = u.screen_capturing && btn != PointerButton::Secondary;
                let mut screen_handled = false;
                let mut screen_dirty = false;
                if was_screen_cap {
                    u.screen_capturing = false;
                    u.focus.set_dragging(false);
                    if let Some(h) = u.screen.as_mut() {
                        let reply = h.on_pointer_up(x, y, mods);
                        screen_handled = reply.handled;
                        screen_dirty = reply.rebuild;
                        if reply.fast_redraw && !reply.rebuild {
                            h.sync_drag_tree();
                            screen_dirty = true;
                        }
                    }
                }
                let out = u.pointer.on_up(pid, PointerButton::Primary, hit, x, y);
                let mut tree_changed = screen_dirty;
                if !screen_handled {
                    if let Some(id) = out.clicked {
                        if let Some(h) = u.screen.as_mut() {
                            tree_changed |= h.on_click(id);
                        }
                    }
                }
                let dirty = out.visual_changed || tree_changed;
                if out.visual_changed {
                    if let Some(h) = u.screen.as_mut() {
                        h.mark_dirty();
                    }
                }
                rebuild_screen_if_dirty(u, hwnd);
                sync_timers(u, hwnd, false);
                (out.release_capture || was_screen_cap, tree_changed, dirty)
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
                if u.screen_capturing {
                    return false;
                }
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
                let mut screen_changed = false;
                if u.screen_capturing {
                    u.screen_capturing = false;
                    u.focus.set_dragging(false);
                    if let Some(h) = u.screen.as_mut() {
                        screen_changed = h.on_pointer_cancel();
                    }
                }
                let dirty = u.pointer.on_capture_lost().visual_changed || screen_changed;
                if dirty {
                    if let Some(h) = u.screen.as_mut() {
                        h.mark_dirty();
                        rebuild_screen_if_dirty(u, hwnd);
                    }
                }
                sync_timers(u, hwnd, false);
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
        let (dx, dy) = if horizontal {
            (u.pointer.wheel.push_horizontal_dip(delta, 48.0), 0.0)
        } else {
            (0.0, u.pointer.wheel.push_vertical_dip(delta, 48.0))
        };
        if dx.abs() > 0.01 || dy.abs() > 0.01 {
            let (px, py) = u.pointer.last_dip.unwrap_or((0.0, 0.0));
            let dirty = u
                .screen
                .as_mut()
                .is_some_and(|h| h.on_wheel_at(px, py, dx, dy));
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

fn current_modifiers() -> KeyModifiers {
    // SAFETY: `GetKeyState` queries the calling thread's modifier key state.
    unsafe {
        KeyModifiers {
            shift: GetKeyState(VK_SHIFT.0 as i32) < 0,
            ctrl: GetKeyState(VK_CONTROL.0 as i32) < 0,
            alt: GetKeyState(VK_MENU.0 as i32) < 0,
        }
    }
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
                    if focus_changed {
                        sync_rich_focus(u);
                        sync_timers(u, hwnd, false);
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
    let (handled, focus_changed, changed, release_cap) = with_ui(|u| match ev.key {
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
                sync_rich_focus(u);
            }
            (true, next != prev, dirty, false)
        }
        KeyCode::Escape => {
            let mut release_cap = false;
            if u.screen_capturing {
                u.screen_capturing = false;
                release_cap = true;
            }
            if let Some(h) = u.screen.as_mut() {
                let prev = u.focus.focused();
                let (handled, dirty) = h.on_key(ev, &mut u.focus);
                if dirty {
                    rebuild_screen_if_dirty(u, hwnd);
                    sync_rich_focus(u);
                    sync_timers(u, hwnd, false);
                    return (handled, u.focus.focused() != prev, true, release_cap);
                }
            }
            let prev = u.focus.focused();
            let out = u.focus.on_escape();
            let changed = out != EscapeOutcome::None;
            if changed {
                sync_rich_focus(u);
            }
            sync_timers(u, hwnd, false);
            (changed, u.focus.focused() != prev, changed, release_cap)
        }
        _ => {
            let prev = u.focus.focused();
            if let Some(h) = u.screen.as_mut() {
                let (mut handled, mut dirty) = h.on_key(ev, &mut u.focus);
                if !handled
                    && !ev.ctrl
                    && !ev.alt
                    && !ev.shift
                    && matches!(ev.key, KeyCode::Enter | KeyCode::Space)
                {
                    if let Some(fid) = u.focus.focused() {
                        let is_rich = u.rich_fields.iter().any(|f| f.slot_id() == fid);
                        let is_clickable = !is_rich
                            && h.tree()
                                .node_by_id(fid)
                                .and_then(|n| h.tree().data(n))
                                .is_some_and(|d| !d.disabled && d.a11y.clickable);
                        if is_clickable && h.on_click(fid) {
                            handled = true;
                            dirty = true;
                        }
                    }
                }
                if dirty {
                    rebuild_screen_if_dirty(u, hwnd);
                    sync_rich_focus(u);
                    sync_timers(u, hwnd, false);
                }
                return (handled, u.focus.focused() != prev, dirty, false);
            }
            (false, false, false, false)
        }
    })
    .unwrap_or((false, false, false, false));
    if release_cap {
        let _ = ReleaseCapture();
    }
    if focus_changed {
        sync_a11y(hwnd);
    }
    if changed {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    handled
}

fn sync_rich_fields(ui: &mut Ui) {
    let Some(slots) = active_tree(ui).map(|t| t.rich_text_slots()) else {
        return;
    };
    ui.syncing_rich = true;
    ui.rich_fields
        .retain(|f| slots.iter().any(|s| s.key == f.key()));

    // SAFETY: `ui.hwnd` is our thread-owned window handle.
    let dpi = unsafe { GetDpiForWindow(ui.hwnd) };
    let text_scale = ui
        .renderer
        .as_ref()
        .map(|r| r.text().text_scale())
        .unwrap_or(1.0);
    let palette = ui.palette;

    for spec in slots {
        if let Some(field) = ui.rich_fields.iter_mut().find(|f| f.key() == spec.key) {
            field.sync_spec(
                spec.id,
                &spec.value,
                &spec.placeholder,
                spec.sync_rev,
                spec.disabled,
                &palette,
            );
            field.sync_bounds(spec.rect, spec.hidden);
        } else if let Some(mut field) = RichEditField::create(
            ui.hwnd,
            spec.id,
            &spec.key,
            spec.sync_rev,
            spec.multiline,
            &spec.placeholder,
            &palette,
            dpi,
            text_scale,
        ) {
            if !spec.value.is_empty() {
                field.set_text(&spec.value);
            }
            field.sync_spec(
                spec.id,
                &spec.value,
                &spec.placeholder,
                spec.sync_rev,
                spec.disabled,
                &palette,
            );
            field.sync_bounds(spec.rect, spec.hidden);
            ui.rich_fields.push(field);
        }
    }
    ui.syncing_rich = false;
}

fn sync_rich_focus(ui: &Ui) {
    // SAFETY: queries and transfers Win32 child focus only within our own active window.
    unsafe {
        let cur = GetFocus();
        let ours = cur == ui.hwnd || ui.rich_fields.iter().any(|f| f.hwnd() == cur);
        if !ours {
            return;
        }
        if let Some(fid) = ui.focus.focused() {
            if let Some(field) = ui.rich_fields.iter().find(|f| f.slot_id() == fid) {
                let disabled = active_tree(ui)
                    .and_then(|t| t.node_by_id(fid).and_then(|n| t.data(n)))
                    .is_none_or(|d| d.disabled || d.hidden);
                if !disabled && cur != field.hwnd() {
                    let _ = SetFocus(Some(field.hwnd()));
                }
                return;
            }
        }
        if ui.rich_fields.iter().any(|f| f.hwnd() == cur) {
            let _ = SetFocus(Some(ui.hwnd));
        }
    }
}

fn sync_timers(ui: &mut Ui, hwnd: HWND, minimized: bool) {
    let want_tick = !minimized && ui.screen.as_ref().is_some_and(|h| h.wants_tick());
    let want_auto = !minimized && ui.screen.as_ref().is_some_and(|h| h.wants_autoscroll());
    // SAFETY: `hwnd` is owned by this thread.
    unsafe {
        if want_tick && !ui.tick_timer_on {
            let _ = SetTimer(Some(hwnd), TIMER_TICK_ID, 1000, None);
            ui.tick_timer_on = true;
        } else if !want_tick && ui.tick_timer_on {
            let _ = KillTimer(Some(hwnd), TIMER_TICK_ID);
            ui.tick_timer_on = false;
        }

        if want_auto && !ui.autoscroll_timer_on {
            let _ = SetTimer(Some(hwnd), TIMER_AUTOSCROLL_ID, 16, None);
            ui.autoscroll_timer_on = true;
        } else if !want_auto && ui.autoscroll_timer_on {
            let _ = KillTimer(Some(hwnd), TIMER_AUTOSCROLL_ID);
            ui.autoscroll_timer_on = false;
        }
    }
}

unsafe fn on_size(hwnd: HWND, size_type: u32) {
    let minimized = size_type == SIZE_MINIMIZED;
    let (w, h) = client_size(hwnd);
    let dpi = GetDpiForWindow(hwnd);
    let scale = frame::scale(dpi);
    with_ui(|u| {
        if !minimized {
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
                sync_rect_cache(active_tree(u));
            }
            sync_rich_fields(u);
        }
        sync_timers(u, hwnd, minimized);
    });
    if !minimized {
        sync_a11y(hwnd);
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
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
        WM_SIZE => on_size(hwnd, wparam.0 as u32),
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
        WM_COMMAND => {
            let code = ((wparam.0 >> 16) & 0xFFFF) as u16;
            if code == EN_CHANGE {
                let child = HWND(lparam.0 as *mut core::ffi::c_void);
                let changed = with_ui(|u| {
                    if u.syncing_rich {
                        return false;
                    }
                    let payload = u
                        .rich_fields
                        .iter()
                        .find(|f| f.hwnd() == child)
                        .map(|f| (f.key().to_string(), f.text()));
                    let Some((key, val)) = payload else {
                        return false;
                    };
                    if let Some(h) = u.screen.as_mut() {
                        if h.on_text_change(&key, &val) {
                            return rebuild_screen_if_dirty(u, hwnd);
                        }
                    }
                    false
                })
                .unwrap_or(false);
                if changed {
                    sync_a11y(hwnd);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                return LRESULT(0);
            }
        }
        m if m == WM_RICHEDIT_SUBMIT => {
            let child = HWND(wparam.0 as *mut core::ffi::c_void);
            let changed = with_ui(|u| {
                let payload = u
                    .rich_fields
                    .iter()
                    .find(|f| f.hwnd() == child)
                    .map(|f| (f.key().to_string(), f.text()));
                let Some((key, val)) = payload else {
                    return false;
                };
                if let Some(h) = u.screen.as_mut() {
                    if h.on_text_submit(&key, &val) {
                        return rebuild_screen_if_dirty(u, hwnd);
                    }
                }
                false
            })
            .unwrap_or(false);
            if changed {
                sync_a11y(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            return LRESULT(0);
        }
        m if m == WM_POPUP_NOTIFY => {
            let changed = with_ui(|u| {
                if let Some(h) = u.screen.as_mut() {
                    if h.on_popup_poll() {
                        rebuild_screen_if_dirty(u, hwnd);
                        sync_timers(u, hwnd, false);
                        return true;
                    }
                }
                false
            })
            .unwrap_or(false);
            if changed {
                sync_a11y(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            return LRESULT(0);
        }
        WM_TIMER => {
            let id = wparam.0;
            let (repaint, a11y) = with_ui(|u| {
                let mut repaint = false;
                let mut a11y = false;
                if id == TIMER_TICK_ID {
                    if let Some(h) = u.screen.as_mut() {
                        if h.on_tick() {
                            a11y = rebuild_screen_if_dirty(u, hwnd);
                            repaint = true;
                        }
                    }
                    sync_timers(u, hwnd, false);
                } else if id == TIMER_AUTOSCROLL_ID {
                    if let Some(h) = u.screen.as_mut() {
                        let reply = h.on_autoscroll();
                        if reply.rebuild {
                            a11y = rebuild_screen_if_dirty(u, hwnd);
                            repaint = true;
                        } else if reply.fast_redraw {
                            h.sync_drag_tree();
                            repaint = true;
                        }
                    }
                    sync_timers(u, hwnd, false);
                }
                (repaint, a11y)
            })
            .unwrap_or((false, false));
            if a11y {
                sync_a11y(hwnd);
            }
            if repaint {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
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
                    let dirty = if m == WM_RICHEDIT_FOCUS {
                        if u.focus.focused() != Some(slot_id) {
                            u.focus.focus_pointer(slot_id)
                        } else {
                            false
                        }
                    } else if u.focus.focused() == Some(slot_id) {
                        u.focus.clear_focus()
                    } else {
                        false
                    };
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
            CUR_HWND.set(0);
            RECT_CACHE.with(|c| c.borrow_mut().clear());
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

    #[test]
    fn wm_dpichanged_and_caption_hit_test_cover_100_150_200() {
        use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

        super::super::init();
        assert!(open());
        assert!(set_screen(Box::new(
            super::super::screen::WidgetGalleryScreen::new()
        )));
        pump_until_painted(1);
        let hwnd = hwnd().expect("live hwnd");

        // SAFETY: `hwnd` is our thread-owned window; `GetWindowRect` & `GetDpiForWindow` are safe.
        let (rc, dpi) = unsafe {
            let mut rc = RECT::default();
            let _ = GetWindowRect(hwnd, &mut rc);
            (rc, GetDpiForWindow(hwnd))
        };
        let max_x = rc.right - frame::px(46.0 * 2.0 - 20.0, dpi);
        let title_y = rc.top + frame::px(16.0, dpi);
        let cap_x = rc.left + frame::px(40.0, dpi);
        let lparam_pt =
            |x: i32, y: i32| LPARAM((((y as u32 & 0xFFFF) << 16) | (x as u32 & 0xFFFF)) as isize);

        // SAFETY: sends synchronous hit-test queries to our live window.
        unsafe {
            assert_eq!(
                SendMessageW(
                    hwnd,
                    WM_NCHITTEST,
                    Some(WPARAM(0)),
                    Some(lparam_pt(max_x, title_y))
                )
                .0 as u32,
                HTMAXBUTTON
            );
            assert_eq!(
                SendMessageW(
                    hwnd,
                    WM_NCHITTEST,
                    Some(WPARAM(0)),
                    Some(lparam_pt(cap_x, title_y))
                )
                .0 as u32,
                HTCAPTION
            );

            // Simulate moving across monitors: 150% (144 DPI) then 200% (192 DPI)
            // using base 500x350 DIPs so even 200% (1000x700 px) stays inside 1080p limits.
            for target_dpi in [144_usize, 192_usize, 96_usize] {
                let s = target_dpi as f32 / 96.0;
                let suggested = RECT {
                    left: rc.left,
                    top: rc.top,
                    right: rc.left + (500.0 * s).round() as i32,
                    bottom: rc.top + (350.0 * s).round() as i32,
                };
                let wp = WPARAM((target_dpi << 16) | target_dpi);
                let _ = SendMessageW(
                    hwnd,
                    WM_DPICHANGED,
                    Some(wp),
                    Some(LPARAM((&suggested as *const RECT) as isize)),
                );
                pump();
                let mut cur = RECT::default();
                let _ = GetWindowRect(hwnd, &mut cur);
                assert_eq!(cur.right - cur.left, suggested.right - suggested.left);
                assert_eq!(cur.bottom - cur.top, suggested.bottom - suggested.top);
            }
        }

        close();
        pump();
        reset_shared();
        assert!(!is_open());
    }

    #[test]
    fn rich_text_slots_auto_sync_and_preserve_text_across_rebuilds() {
        use super::super::screen::BuiltScreen;
        use super::super::widgets::button::{self, ButtonProps};
        use super::super::widgets::text_field::{self, TextFieldProps};
        use taffy::prelude::*;

        struct RichScreen {
            show_extra: bool,
            rev: u64,
            val_main: String,
            val_extra: String,
            submitted: bool,
            toggle_btn: Option<WidgetId>,
        }

        impl Screen for RichScreen {
            fn build(&mut self, env: &ScreenEnv<'_>) -> BuiltScreen {
                let mut tree = Tree::new(Style {
                    size: super::super::tree::style::size(env.width_dip, env.height_dip),
                    gap: super::super::tree::style::gap(8.0),
                    padding: super::super::tree::style::pad(12.0, 12.0, 12.0, 12.0),
                    ..super::super::tree::style::column()
                });
                let root = tree.root();
                let mut tf = TextFieldProps::simple("main.field", "Main note", &self.val_main);
                tf.sync_rev = self.rev;
                text_field::build_tree_node(&mut tree, root, &tf, env.palette);
                if self.show_extra {
                    let tf2 = TextFieldProps::simple("extra.field", "Extra note", &self.val_extra);
                    text_field::build_tree_node(&mut tree, root, &tf2, env.palette);
                }
                let (_btn, bid) = button::build(
                    &mut tree,
                    root,
                    &ButtonProps::new("Toggle extra"),
                    env.palette,
                );
                self.toggle_btn = Some(bid);
                BuiltScreen {
                    tree,
                    modal_root: None,
                }
            }

            fn on_click(&mut self, id: WidgetId) -> bool {
                if Some(id) == self.toggle_btn {
                    self.show_extra = !self.show_extra;
                    return true;
                }
                false
            }

            fn on_text_change(&mut self, key: &str, value: &str) -> bool {
                if key == "main.field" {
                    self.val_main = value.to_string();
                } else if key == "extra.field" {
                    self.val_extra = value.to_string();
                }
                false
            }

            fn on_text_submit(&mut self, key: &str, _value: &str) -> bool {
                if key == "main.field" {
                    self.submitted = true;
                    self.rev += 1;
                    self.val_main = "submitted".into();
                    return true;
                }
                false
            }
        }

        super::super::init();
        assert!(open());
        assert!(set_screen(Box::new(RichScreen {
            show_extra: true,
            rev: 0,
            val_main: "Initial".into(),
            val_extra: "Second".into(),
            submitted: false,
            toggle_btn: None,
        })));
        pump_until_painted(1);

        let hwnd = hwnd().expect("live hwnd");
        let (count, main_hwnd, main_text) = with_ui(|u| {
            let f = u
                .rich_fields
                .iter()
                .find(|f| f.key() == "main.field")
                .unwrap();
            (u.rich_fields.len(), f.hwnd(), f.text())
        })
        .unwrap();
        assert_eq!(count, 2);
        assert_eq!(main_text, "Initial");

        // User edits text in child RichEdit: rebuilding screen without bumping sync_rev must preserve user text.
        with_ui(|u| {
            let f = u
                .rich_fields
                .iter()
                .find(|f| f.hwnd() == main_hwnd)
                .unwrap();
            f.set_text("User typed");
        });
        // Trigger EN_CHANGE + toggle extra field off (rebuilds tree, destroys dead slot).
        let toggle_id = with_ui(|u| {
            let t = active_tree(u).unwrap();
            t.all_nodes()
                .into_iter()
                .filter_map(|n| t.data(n))
                .find(|d| d.a11y.name == "Toggle extra")
                .unwrap()
                .id
        })
        .unwrap();
        // SAFETY: sends WM_COMMAND(EN_CHANGE) for our test child HWND and triggers screen click.
        unsafe {
            let wp = WPARAM(((EN_CHANGE as usize) << 16) | 1);
            let _ = SendMessageW(
                hwnd,
                WM_COMMAND,
                Some(wp),
                Some(LPARAM(main_hwnd.0 as isize)),
            );
        }
        with_ui(|u| {
            assert!(u.screen.as_mut().unwrap().on_click(toggle_id));
            assert!(rebuild_screen_if_dirty(u, hwnd));
        });
        pump();

        let (count2, preserved) = with_ui(|u| {
            let f = u
                .rich_fields
                .iter()
                .find(|f| f.key() == "main.field")
                .unwrap();
            (u.rich_fields.len(), f.text())
        })
        .unwrap();
        assert_eq!(count2, 1, "hidden/removed slot should be destroyed");
        assert_eq!(preserved, "User typed");

        // Submit bumps sync_rev -> child text syncs to updated model value.
        // SAFETY: sends WM_RICHEDIT_SUBMIT for our test child HWND.
        unsafe {
            let _ = SendMessageW(
                hwnd,
                WM_RICHEDIT_SUBMIT,
                Some(WPARAM(main_hwnd.0 as usize)),
                Some(LPARAM(0)),
            );
        }
        pump();
        let after_submit = with_ui(|u| u.rich_fields[0].text()).unwrap();
        assert_eq!(after_submit, "submitted");

        close();
        pump();
        reset_shared();
        assert!(!is_open());
    }

    #[test]
    fn keyboard_enter_and_space_activate_focused_clickable_widget() {
        super::super::init();
        assert!(open());
        assert!(set_screen(Box::new(
            super::super::screen::WidgetGalleryScreen::new()
        )));
        pump_until_painted(1);
        let hwnd = hwnd().expect("live hwnd");

        // Focus first widget ("Open dialog" button) via Tab, then activate with Enter.
        // SAFETY: sends WM_KEYDOWN messages directly to our live window.
        unsafe {
            let _ = SendMessageW(hwnd, WM_KEYDOWN, Some(WPARAM(VK_TAB.0 as usize)), None);
            let _ = SendMessageW(hwnd, WM_KEYDOWN, Some(WPARAM(VK_RETURN.0 as usize)), None);
        }
        pump();
        let scope_after_enter = with_ui(|u| u.focus.active_scope()).unwrap();
        assert!(matches!(scope_after_enter, FocusScope::Modal(_)));

        // Escape dismisses modal cleanly back to Main scope.
        // SAFETY: sends WM_KEYDOWN(VK_ESCAPE) to our live window.
        unsafe {
            let _ = SendMessageW(hwnd, WM_KEYDOWN, Some(WPARAM(VK_ESCAPE.0 as usize)), None);
        }
        pump();
        let scope_after_esc = with_ui(|u| u.focus.active_scope()).unwrap();
        assert_eq!(scope_after_esc, FocusScope::Main);

        close();
        pump();
        reset_shared();
        assert!(!is_open());
    }
}
