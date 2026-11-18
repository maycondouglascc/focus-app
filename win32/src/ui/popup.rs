//! Lightweight top-level `WS_POPUP` window for transient overlays (`Select` menus
//! and `Popover` panels). Clamps/flips against the monitor's `rcWork` via
//! `MonitorFromPoint` + `GetMonitorInfoW`, owns its own swap chain / AccessKit
//! adapter, and dismisses on `Esc` or `WM_ACTIVATE(WA_INACTIVE)`.

use std::cell::RefCell;

use accesskit::Action;
use accesskit_windows::Adapter;
use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, GetMonitorInfoW, InvalidateRect, MonitorFromPoint, ScreenToClient,
    MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
    VIRTUAL_KEY, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_MENU, VK_NEXT,
    VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::Input::Pointer::{
    GetPointerInfo, POINTER_FLAG_FIRSTBUTTON, POINTER_FLAG_SECONDBUTTON, POINTER_INFO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, IsWindow, KillTimer,
    LoadCursorW, PostMessageW, RegisterClassW, SetForegroundWindow, SetTimer, SetWindowPos,
    ShowWindow, CS_HREDRAW, CS_VREDRAW, IDC_ARROW, PT_PEN, PT_TOUCH, SWP_NOACTIVATE, SWP_NOZORDER,
    SW_SHOW, WA_INACTIVE, WINDOW_EX_STYLE, WM_ACTIVATE, WM_APP, WM_CAPTURECHANGED, WM_CLOSE,
    WM_COMMAND, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND, WM_GETOBJECT, WM_KEYDOWN, WM_KILLFOCUS,
    WM_MOUSEHWHEEL, WM_MOUSEWHEEL, WM_PAINT, WM_POINTERCAPTURECHANGED, WM_POINTERDOWN,
    WM_POINTERENTER, WM_POINTERHWHEEL, WM_POINTERLEAVE, WM_POINTERUP, WM_POINTERUPDATE,
    WM_POINTERWHEEL, WM_SETFOCUS, WM_SETTINGCHANGE, WM_SIZE, WM_SYSCOLORCHANGE, WM_SYSKEYDOWN,
    WM_THEMECHANGED, WM_TIMER, WNDCLASSW, WS_CLIPCHILDREN, WS_EX_NOREDIRECTIONBITMAP,
    WS_EX_TOOLWINDOW, WS_POPUP,
};

use super::a11y::{self, A11yActionQueue, StaticActivation, WM_A11Y_ACTION};
use super::focus::{FocusModel, FocusScope};
use super::frame;
use super::input::{KeyCode, KeyEvent, PointerButton, PointerKind, PointerState};
use super::prefs::{self, SystemPrefs};
use super::render::{reset_shared, DrawError, FrameOpts, Hover, Renderer};
use super::strings;
use super::theme::Palette;
use super::tree::{Tree, WidgetId};
use super::widgets::text_field::{
    sync_rich_fields, sync_rich_focus, RichEditField, WM_RICHEDIT_BLUR, WM_RICHEDIT_FOCUS,
    WM_RICHEDIT_SUBMIT,
};

/// `WM_MOUSELEAVE`
const WM_MOUSELEAVE: u32 = 0x02A3;
const EN_CHANGE: u16 = 0x0300;
const POPUP_TICK_TIMER_ID: usize = 1;
/// Posted to the popup owner window when a popup updates state or closes.
pub const WM_POPUP_NOTIFY: u32 = WM_APP + 5;

/// Physical screen-space rectangle (`left/top/right/bottom` in virtual desktop pixels).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl ScreenRect {
    pub fn width(self) -> i32 {
        (self.right - self.left).max(0)
    }

    pub fn height(self) -> i32 {
        (self.bottom - self.top).max(0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // variants used by Select and Popover
pub enum PopupAlign {
    BelowStart,
    BelowEnd,
    Center,
}

/// Computes the top-left physical screen position `(x, y)` for a `size_px` popup,
/// flipping across the anchor when overflowing the monitor work area and clamping inside `work`.
pub fn place_popup(
    anchor: Option<ScreenRect>,
    size_px: (i32, i32),
    work: ScreenRect,
    align: PopupAlign,
) -> (i32, i32) {
    let w = size_px.0.max(1).min(work.width().max(1));
    let h = size_px.1.max(1).min(work.height().max(1));

    let Some(a) = anchor else {
        let x = work.left + (work.width() - w) / 2;
        let y = work.top + (work.height() - h) / 2;
        return (
            x.clamp(work.left, (work.right - w).max(work.left)),
            y.clamp(work.top, (work.bottom - h).max(work.top)),
        );
    };

    if align == PopupAlign::Center {
        let x = a.left + (a.width() - w) / 2;
        let y = a.top + (a.height() - h) / 2;
        return (
            x.clamp(work.left, (work.right - w).max(work.left)),
            y.clamp(work.top, (work.bottom - h).max(work.top)),
        );
    }

    let mut x = match align {
        PopupAlign::BelowEnd => a.right - w,
        PopupAlign::BelowStart | PopupAlign::Center => a.left,
    };
    if x + w > work.right && a.right - w >= work.left {
        x = a.right - w;
    }
    x = x.clamp(work.left, (work.right - w).max(work.left));

    let mut y = a.bottom + 4;
    if y + h > work.bottom {
        let above = a.top - 4 - h;
        if above >= work.top {
            y = above;
        }
    }
    y = y.clamp(work.top, (work.bottom - h).max(work.top));

    (x, y)
}

/// Queries the nearest monitor's work area (`rcWork`, excluding the taskbar) in physical pixels.
pub fn monitor_work_area(anchor: Option<ScreenRect>) -> ScreenRect {
    let pt = match anchor {
        Some(a) => POINT {
            x: a.left + a.width() / 2,
            y: a.top + a.height() / 2,
        },
        None => POINT { x: 0, y: 0 },
    };
    // SAFETY: `MonitorFromPoint` and `GetMonitorInfoW` accept any POINT and fill `MONITORINFO`.
    unsafe {
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(hmon, &mut mi).as_bool() {
            ScreenRect {
                left: mi.rcWork.left,
                top: mi.rcWork.top,
                right: mi.rcWork.right,
                bottom: mi.rcWork.bottom,
            }
        } else {
            ScreenRect {
                left: 0,
                top: 0,
                right: 1280,
                bottom: 800,
            }
        }
    }
}

/// Events emitted from an open popup window to its widget controller.
#[derive(Clone, Debug, PartialEq)]
#[allow(dead_code)] // consumed by Select, Popover, and Tray Flyout
pub enum PopupEvent {
    Click(WidgetId),
    Key(KeyEvent),
    Wheel(f32),
    FocusChanged(Option<WidgetId>),
    TextChange(String, String),
    TextSubmit(String, String),
    Tick,
    Closed,
}

/// Outcome returned by [`PopupDelegate::on_event`] so popups never close reentrantly
/// while `POPUP` is mutably borrowed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PopupOutcome {
    pub handled: bool,
    pub dirty: bool,
    pub close: bool,
    pub notify_owner: bool,
    pub resize_h_dip: Option<f32>,
}

pub trait PopupDelegate {
    fn on_event(
        &mut self,
        event: PopupEvent,
        tree: &mut Tree,
        focus: &mut FocusModel,
    ) -> PopupOutcome;

    fn tick_ms(&self) -> Option<u32> {
        None
    }
}

struct ActivePopup {
    hwnd: HWND,
    owner: Option<HWND>,
    renderer: Option<Renderer>,
    prefs: SystemPrefs,
    palette: Palette,
    tree: Tree,
    focus: FocusModel,
    pointer: PointerState,
    a11y_queue: A11yActionQueue,
    a11y: Adapter,
    delegate: Box<dyn PopupDelegate>,
    anchor_screen: Option<ScreenRect>,
    align: PopupAlign,
    size_dip: (f32, f32),
    rich_fields: Vec<RichEditField>,
    syncing_rich: bool,
    tick_timer_ms: Option<u32>,
    dismissing: bool,
}

fn sync_popup_rich(p: &mut ActivePopup) {
    let scale = p
        .renderer
        .as_ref()
        .map_or(p.prefs.text_scale, |r| r.text().text_scale());
    sync_rich_fields(
        p.hwnd,
        &p.tree,
        &mut p.rich_fields,
        scale,
        &p.palette,
        &mut p.syncing_rich,
    );
}

fn sync_popup_timer(p: &mut ActivePopup) {
    let want = p.delegate.tick_ms();
    if want == p.tick_timer_ms {
        return;
    }
    // SAFETY: `p.hwnd` is owned by this UI thread.
    unsafe {
        match want {
            Some(ms) => {
                let _ = SetTimer(Some(p.hwnd), POPUP_TICK_TIMER_ID, ms.max(100), None);
            }
            None => {
                let _ = KillTimer(Some(p.hwnd), POPUP_TICK_TIMER_ID);
            }
        }
    }
    p.tick_timer_ms = want;
}

thread_local! {
    static POPUP: RefCell<Option<ActivePopup>> = const { RefCell::new(None) };
}

fn with_popup<R>(f: impl FnOnce(&mut ActivePopup) -> R) -> Option<R> {
    POPUP.with(|p| p.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

#[allow(dead_code)] // used by Select and Popover in commits 6-7
pub fn is_open() -> bool {
    let hwnd = POPUP.with(|p| p.try_borrow().ok().and_then(|g| g.as_ref().map(|a| a.hwnd)));
    // SAFETY: `IsWindow` safely validates any HWND value.
    hwnd.is_some_and(|h| unsafe { IsWindow(Some(h)).as_bool() })
}

#[cfg(test)]
pub fn active_hwnd() -> Option<HWND> {
    POPUP.with(|p| p.try_borrow().ok().and_then(|g| g.as_ref().map(|a| a.hwnd)))
}

/// Opens a new `WS_POPUP` window, closing any previous popup first.
#[allow(dead_code)] // used by Select and Popover in commits 6-7
pub fn open(
    owner: Option<HWND>,
    anchor_screen: Option<ScreenRect>,
    size_dip: (f32, f32),
    align: PopupAlign,
    mut tree: Tree,
    delegate: Box<dyn PopupDelegate>,
) -> Option<HWND> {
    close();

    // SAFETY: called on the UI thread; handles are owned by the caller/popup.
    unsafe {
        let hinstance = HINSTANCE(GetModuleHandleW(None).ok()?.0);
        let class = w!("FocusPopupWindow");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(popup_wnd_proc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_ARROW).ok()?,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);

        let dpi = owner
            .filter(|h| IsWindow(Some(*h)).as_bool())
            .map(|h| GetDpiForWindow(h))
            .unwrap_or(96);
        let scale = frame::scale(dpi);
        let size_px = (
            (size_dip.0 * scale).ceil().max(48.0) as i32,
            (size_dip.1 * scale).ceil().max(32.0) as i32,
        );
        let work = monitor_work_area(anchor_screen);
        let (x, y) = place_popup(anchor_screen, size_px, work, align);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0 | WS_EX_NOREDIRECTIONBITMAP.0),
            class,
            w!("FocusPopup"),
            WS_POPUP | WS_CLIPCHILDREN,
            x,
            y,
            size_px.0,
            size_px.1,
            owner,
            None,
            Some(hinstance),
            None,
        )
        .ok()?;

        // Adjust position once using the popup's actual monitor DPI in case owner straddled monitors.
        let actual_dpi = GetDpiForWindow(hwnd);
        let actual_scale = frame::scale(actual_dpi);
        if (actual_scale - scale).abs() > 0.001 {
            let new_px = (
                (size_dip.0 * actual_scale).ceil().max(48.0) as i32,
                (size_dip.1 * actual_scale).ceil().max(32.0) as i32,
            );
            let (nx, ny) = place_popup(anchor_screen, new_px, work, align);
            let _ = SetWindowPos(
                hwnd,
                None,
                nx,
                ny,
                new_px.0,
                new_px.1,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }

        prefs::apply_rounded_corners(hwnd);
        let sys_prefs = SystemPrefs::query();
        let palette = Palette::for_system(sys_prefs.high_contrast);
        let (cw, ch) = client_size(hwnd);
        let renderer = match Renderer::new(hwnd, cw.max(1) as u32, ch.max(1) as u32) {
            Ok(r) => {
                r.set_text_scale(sys_prefs.text_scale);
                tree.layout(cw as f32 / actual_scale, ch as f32 / actual_scale, r.text());
                Some(r)
            }
            Err(e) => {
                eprintln!("focus: popup renderer failed: {e}");
                None
            }
        };

        let root_id = tree.id(tree.root());
        let mut focus = FocusModel::new();
        focus.sync_from_tree(&tree, |_, _| FocusScope::Popup(root_id));
        focus.push_popup(root_id);

        let a11y_queue = A11yActionQueue::new(hwnd);
        let a11y = Adapter::new(hwnd, false, a11y_queue.handler());

        POPUP.with(|p| {
            let mut active = ActivePopup {
                hwnd,
                owner,
                renderer,
                prefs: sys_prefs,
                palette,
                tree,
                focus,
                pointer: PointerState::default(),
                a11y_queue,
                a11y,
                delegate,
                anchor_screen,
                align,
                size_dip,
                rich_fields: Vec::new(),
                syncing_rich: false,
                tick_timer_ms: None,
                dismissing: false,
            };
            sync_popup_rich(&mut active);
            sync_popup_timer(&mut active);
            *p.borrow_mut() = Some(active);
        });

        if owner.is_none() {
            let _ = SetForegroundWindow(hwnd);
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        with_popup(|p| {
            sync_rich_focus(p.hwnd, &p.focus, &p.tree, &p.rich_fields);
        });
        let _ = InvalidateRect(Some(hwnd), None, false);
        Some(hwnd)
    }
}

/// Closes any open popup window cleanly and returns focus to its owner if still open.
pub fn close() {
    let popped = POPUP.with(|p| p.try_borrow_mut().ok().and_then(|mut g| g.take()));
    let Some(mut popup) = popped else {
        return;
    };
    popup.dismissing = true;
    let _ = popup
        .delegate
        .on_event(PopupEvent::Closed, &mut popup.tree, &mut popup.focus);
    let hwnd = popup.hwnd;
    let owner = popup.owner;
    if popup.tick_timer_ms.is_some() {
        // SAFETY: `hwnd` belongs to the UI thread.
        unsafe {
            let _ = KillTimer(Some(hwnd), POPUP_TICK_TIMER_ID);
        }
    }
    drop(popup);
    // SAFETY: `hwnd` and `owner` belong to the current UI thread; invalid handles are harmless.
    unsafe {
        if IsWindow(Some(hwnd)).as_bool() {
            let _ = DestroyWindow(hwnd);
        }
        if let Some(ow) = owner {
            if IsWindow(Some(ow)).as_bool() {
                let _ = SetFocus(Some(ow));
                let _ = PostMessageW(Some(ow), WM_POPUP_NOTIFY, WPARAM(0), LPARAM(0));
            }
        }
    }
    super::reset_shared_if_idle();
}

/// Updates the active popup's tree and optional height, keeping it clamped inside `rcWork`.
#[allow(dead_code)] // used by Select and Popover
pub fn refresh_layout() {
    refresh_layout_with_height(None);
}

#[allow(dead_code)]
pub fn refresh_layout_with_height(desired_h_dip: Option<f32>) {
    let info = POPUP.with(|p| {
        p.try_borrow().ok().and_then(|g| {
            g.as_ref()
                .map(|a| (a.hwnd, a.size_dip, a.anchor_screen, a.align))
        })
    });
    let Some((hwnd, mut size_dip, anchor, align)) = info else {
        return;
    };
    // SAFETY: `hwnd` is a live popup owned by this thread.
    unsafe {
        if let Some(new_h) = desired_h_dip {
            let dpi = GetDpiForWindow(hwnd);
            let scale = frame::scale(dpi);
            let work = monitor_work_area(anchor);
            let max_h_dip = (work.height() as f32 / scale - 16.0).max(80.0);
            let clamped_h = new_h.clamp(48.0, max_h_dip);
            if (clamped_h - size_dip.1).abs() > 0.5 {
                size_dip.1 = clamped_h;
                let size_px = (
                    (size_dip.0 * scale).ceil().max(48.0) as i32,
                    (size_dip.1 * scale).ceil().max(32.0) as i32,
                );
                let (x, y) = place_popup(anchor, size_px, work, align);
                with_popup(|p| {
                    p.size_dip = size_dip;
                });
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    x,
                    y,
                    size_px.0,
                    size_px.1,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }
        let dpi = GetDpiForWindow(hwnd);
        let scale = frame::scale(dpi);
        let (cw, ch) = client_size(hwnd);
        with_popup(|p| {
            if let Some(r) = p.renderer.as_ref() {
                p.tree
                    .layout(cw as f32 / scale, ch as f32 / scale, r.text());
            }
            let root_id = p.tree.id(p.tree.root());
            p.focus
                .sync_from_tree(&p.tree, |_, _| FocusScope::Popup(root_id));
            let cur = p.focus.focused();
            let _ = p
                .delegate
                .on_event(PopupEvent::FocusChanged(cur), &mut p.tree, &mut p.focus);
            sync_popup_rich(p);
            sync_popup_timer(p);
            sync_rich_focus(p.hwnd, &p.focus, &p.tree, &p.rich_fields);
        });
        sync_a11y(hwnd);
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

unsafe fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    (rc.right - rc.left, rc.bottom - rc.top)
}

unsafe fn sync_a11y(hwnd: HWND) {
    let dpi = GetDpiForWindow(hwnd);
    let client_px = client_size(hwnd);
    let scale = frame::scale(dpi);
    let queued = POPUP.with(|p| {
        let Ok(mut g) = p.try_borrow_mut() else {
            return None;
        };
        let popup = g.as_mut()?;
        let tree = &popup.tree;
        let focused = popup.focus.focused();
        popup.a11y.update_if_active(|| {
            a11y::build_tree_update(Some(tree), focused, scale, client_px, strings::APP_NAME)
        })
    });
    if let Some(ev) = queued {
        ev.raise();
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

unsafe fn on_pointer_msg(hwnd: HWND, msg: u32, wparam: WPARAM) -> bool {
    match msg {
        WM_POINTERENTER | WM_POINTERUPDATE => {
            let Some((_, kind, _, x, y)) = query_pointer(hwnd, wparam) else {
                return false;
            };
            if kind == PointerKind::Mouse {
                let mut t = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = TrackMouseEvent(&mut t);
            }
            let changed = with_popup(|p| {
                let hit = p.tree.hit_test(x, y);
                p.pointer.on_move(hit, x, y).visual_changed
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
            let (request_capture, focus_changed, visual_changed) = with_popup(|p| {
                let hit = p.tree.hit_test(x, y);
                let out = p.pointer.on_down(pid, btn, hit, x, y);
                let focus_changed = match hit {
                    Some(id) => p.focus.focus_pointer(id),
                    None => p.focus.clear_focus(),
                };
                (
                    out.request_capture,
                    focus_changed,
                    out.visual_changed || focus_changed,
                )
            })
            .unwrap_or((false, false, false));
            if request_capture && kind == PointerKind::Mouse {
                SetCapture(hwnd);
            }
            if focus_changed {
                with_popup(|p| {
                    sync_rich_focus(p.hwnd, &p.focus, &p.tree, &p.rich_fields);
                });
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
            let (release_capture, clicked, dirty, should_close, notify_owner, resize_h, owner) =
                with_popup(|p| {
                    let hit = p.tree.hit_test(x, y);
                    let out = p.pointer.on_up(pid, PointerButton::Primary, hit, x, y);
                    let mut dirty = out.visual_changed;
                    let mut should_close = false;
                    let mut notify_owner = false;
                    let mut resize_h = None;
                    if let Some(id) = out.clicked {
                        let ev_out =
                            p.delegate
                                .on_event(PopupEvent::Click(id), &mut p.tree, &mut p.focus);
                        dirty |= ev_out.dirty;
                        should_close |= ev_out.close;
                        notify_owner |= ev_out.notify_owner;
                        resize_h = ev_out.resize_h_dip;
                    }
                    (
                        out.release_capture,
                        out.clicked,
                        dirty,
                        should_close,
                        notify_owner,
                        resize_h,
                        p.owner,
                    )
                })
                .unwrap_or((false, None, false, false, false, None, None));
            if release_capture {
                let _ = ReleaseCapture();
            }
            if should_close {
                close();
                return true;
            }
            if notify_owner {
                if let Some(ow) = owner {
                    let _ = PostMessageW(Some(ow), WM_POPUP_NOTIFY, WPARAM(0), LPARAM(0));
                }
            }
            if resize_h.is_some() || dirty {
                if resize_h.is_some() || clicked.is_some() {
                    refresh_layout_with_height(resize_h);
                } else {
                    if clicked.is_some() {
                        sync_a11y(hwnd);
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                return true;
            }
            if clicked.is_some() {
                sync_a11y(hwnd);
            }
            true
        }
        WM_POINTERLEAVE | WM_MOUSELEAVE => {
            let changed = with_popup(|p| p.pointer.on_leave().visual_changed).unwrap_or(false);
            if changed {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            true
        }
        WM_POINTERCAPTURECHANGED | WM_CAPTURECHANGED => {
            let changed =
                with_popup(|p| p.pointer.on_capture_lost().visual_changed).unwrap_or(false);
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
    let (handled, dirty) = with_popup(|p| {
        let dy = if horizontal {
            let _ = p.pointer.wheel.push_horizontal_dip(delta, 48.0);
            0.0
        } else {
            p.pointer.wheel.push_vertical_dip(delta, 48.0)
        };
        if dy.abs() > 0.01 {
            let out = p
                .delegate
                .on_event(PopupEvent::Wheel(dy), &mut p.tree, &mut p.focus);
            (out.handled, out.dirty)
        } else {
            (false, false)
        }
    })
    .unwrap_or((false, false));
    if dirty {
        sync_a11y(hwnd);
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    handled || msg == WM_POINTERWHEEL || msg == WM_POINTERHWHEEL
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
    // SAFETY: `GetKeyState` queries the calling thread's modifier key state.
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
    if ev.key == KeyCode::Escape {
        close();
        return true;
    }
    let (
        handled,
        focus_changed,
        dirty,
        activated_click,
        should_close,
        notify_owner,
        resize_h,
        owner,
    ) = with_popup(|p| match ev.key {
        KeyCode::Tab if !ev.ctrl && !ev.alt => {
            let prev = p.focus.focused();
            let prev_vis = p.focus.is_focus_visible();
            let next = p.focus.tab(ev.shift);
            let mut scroll_dirty = false;
            if next != prev {
                let fout =
                    p.delegate
                        .on_event(PopupEvent::FocusChanged(next), &mut p.tree, &mut p.focus);
                scroll_dirty = fout.dirty;
                sync_rich_focus(p.hwnd, &p.focus, &p.tree, &p.rich_fields);
            }
            (
                true,
                next != prev,
                next != prev || p.focus.is_focus_visible() != prev_vis || scroll_dirty,
                false,
                false,
                false,
                None,
                p.owner,
            )
        }
        _ => {
            let prev = p.focus.focused();
            let mut out = p
                .delegate
                .on_event(PopupEvent::Key(ev), &mut p.tree, &mut p.focus);
            let mut activated_click = false;
            if !out.handled
                && !ev.ctrl
                && !ev.alt
                && !ev.shift
                && matches!(ev.key, KeyCode::Enter | KeyCode::Space)
            {
                if let Some(fid) = p.focus.focused() {
                    let is_clickable = p
                        .tree
                        .node_by_id(fid)
                        .and_then(|n| p.tree.data(n))
                        .is_some_and(|d| !d.disabled && d.a11y.clickable);
                    if is_clickable {
                        out =
                            p.delegate
                                .on_event(PopupEvent::Click(fid), &mut p.tree, &mut p.focus);
                        out.handled = true;
                        activated_click = true;
                    }
                }
            }
            (
                out.handled,
                p.focus.focused() != prev,
                out.dirty,
                activated_click,
                out.close,
                out.notify_owner,
                out.resize_h_dip,
                p.owner,
            )
        }
    })
    .unwrap_or((false, false, false, false, false, false, None, None));

    if should_close {
        close();
        return true;
    }
    if notify_owner {
        if let Some(ow) = owner {
            let _ = PostMessageW(Some(ow), WM_POPUP_NOTIFY, WPARAM(0), LPARAM(0));
        }
    }
    if resize_h.is_some() || (activated_click && dirty) {
        refresh_layout_with_height(resize_h);
        return handled;
    }
    if focus_changed {
        sync_a11y(hwnd);
    }
    if dirty {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    handled
}

unsafe fn on_paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    BeginPaint(hwnd, &mut ps);
    let _ = EndPaint(hwnd, &ps);

    let dpi = GetDpiForWindow(hwnd);
    let (w, h) = client_size(hwnd);
    let scale = frame::scale(dpi);
    let retry = with_popup(|p| {
        if p.renderer.is_none() {
            p.renderer = Renderer::new(hwnd, w.max(1) as u32, h.max(1) as u32).ok();
            if let Some(r) = p.renderer.as_ref() {
                r.set_text_scale(p.prefs.text_scale);
            }
        }
        let palette = p.palette;
        let focus_ring = p.focus.focus_ring_rect(&p.tree);
        let Some(r) = p.renderer.as_mut() else {
            return false;
        };
        let res = r.draw_frame(FrameOpts {
            dpi,
            w: w as f32 / scale,
            h: h as f32 / scale,
            hover: Hover::None,
            palette: &palette,
            mica: false,
            show_title_bar: false,
            tree: Some(&p.tree),
            focus_ring,
            render_rich_slots: false,
        });
        match res {
            Ok(()) => false,
            Err(DrawError::DeviceLost) => {
                p.renderer = None;
                reset_shared();
                true
            }
            Err(DrawError::Other(e)) => {
                eprintln!("focus: popup draw failed: {e}");
                false
            }
        }
    })
    .unwrap_or(false);
    if retry {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

unsafe extern "system" fn popup_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_ACTIVATE => {
            if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE {
                let already_closing =
                    POPUP.with(|p| p.try_borrow().ok().is_none_or(|g| g.is_none()));
                if !already_closing {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
            return LRESULT(0);
        }
        WM_CLOSE => {
            close();
            return LRESULT(0);
        }
        WM_SIZE => {
            let (w, h) = client_size(hwnd);
            let dpi = GetDpiForWindow(hwnd);
            let scale = frame::scale(dpi);
            with_popup(|p| {
                if let Some(r) = p.renderer.as_mut() {
                    if r.resize(w.max(1) as u32, h.max(1) as u32).is_err() {
                        p.renderer = None;
                        reset_shared();
                    }
                }
                if let Some(r) = p.renderer.as_ref() {
                    p.tree.layout(w as f32 / scale, h as f32 / scale, r.text());
                }
            });
            sync_a11y(hwnd);
            let _ = InvalidateRect(Some(hwnd), None, false);
            return LRESULT(0);
        }
        WM_DPICHANGED => {
            let raw_dpi = (wparam.0 & 0xFFFF) as u32;
            let dpi = if raw_dpi > 0 {
                raw_dpi
            } else {
                GetDpiForWindow(hwnd)
            };
            let scale = frame::scale(dpi);
            let r = &*(lparam.0 as *const RECT);
            let target_w =
                with_popup(|p| (p.size_dip.0 * scale).ceil() as i32).unwrap_or(r.right - r.left);
            let target_h =
                with_popup(|p| (p.size_dip.1 * scale).ceil() as i32).unwrap_or(r.bottom - r.top);
            let _ = SetWindowPos(
                hwnd,
                None,
                r.left,
                r.top,
                target_w,
                target_h,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            with_popup(|p| {
                let text_scale = p.prefs.text_scale;
                let palette = p.palette;
                for f in &mut p.rich_fields {
                    f.sync_theme(&palette, dpi, text_scale);
                }
                sync_popup_rich(p);
            });
            return LRESULT(0);
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED | WM_SYSCOLORCHANGE => {
            let sys_prefs = SystemPrefs::query();
            let dpi = GetDpiForWindow(hwnd);
            let changed = with_popup(|p| {
                let palette = Palette::for_system(sys_prefs.high_contrast);
                let text_changed = p
                    .renderer
                    .as_ref()
                    .is_some_and(|r| r.set_text_scale(sys_prefs.text_scale));
                let changed = p.prefs != sys_prefs || p.palette != palette || text_changed;
                p.prefs = sys_prefs;
                p.palette = palette;
                if changed {
                    for f in &mut p.rich_fields {
                        f.sync_theme(&palette, dpi, p.prefs.text_scale);
                    }
                    sync_popup_rich(p);
                }
                changed
            })
            .unwrap_or(false);
            if changed {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
        WM_COMMAND => {
            let code = ((wparam.0 >> 16) & 0xFFFF) as u16;
            if code == EN_CHANGE {
                let child = HWND(lparam.0 as *mut core::ffi::c_void);
                let (dirty, should_close, resize_h) = with_popup(|p| {
                    if p.syncing_rich {
                        return (false, false, None);
                    }
                    let payload = p
                        .rich_fields
                        .iter()
                        .find(|f| f.hwnd() == child)
                        .map(|f| (f.key().to_string(), f.text()));
                    let Some((key, val)) = payload else {
                        return (false, false, None);
                    };
                    let out = p.delegate.on_event(
                        PopupEvent::TextChange(key, val),
                        &mut p.tree,
                        &mut p.focus,
                    );
                    (out.dirty, out.close, out.resize_h_dip)
                })
                .unwrap_or((false, false, None));
                if should_close {
                    close();
                } else if resize_h.is_some() || dirty {
                    refresh_layout_with_height(resize_h);
                }
                return LRESULT(0);
            }
        }
        m if m == WM_RICHEDIT_SUBMIT => {
            let child = HWND(wparam.0 as *mut core::ffi::c_void);
            let (dirty, should_close, notify_owner, resize_h, owner) = with_popup(|p| {
                let payload = p
                    .rich_fields
                    .iter()
                    .find(|f| f.hwnd() == child)
                    .map(|f| (f.key().to_string(), f.text()));
                let Some((key, val)) = payload else {
                    return (false, false, false, None, p.owner);
                };
                let out = p.delegate.on_event(
                    PopupEvent::TextSubmit(key, val),
                    &mut p.tree,
                    &mut p.focus,
                );
                (
                    out.dirty,
                    out.close,
                    out.notify_owner,
                    out.resize_h_dip,
                    p.owner,
                )
            })
            .unwrap_or((false, false, false, None, None));
            if should_close {
                close();
                return LRESULT(0);
            }
            if notify_owner {
                if let Some(ow) = owner {
                    let _ = PostMessageW(Some(ow), WM_POPUP_NOTIFY, WPARAM(0), LPARAM(0));
                }
            }
            if resize_h.is_some() || dirty {
                refresh_layout_with_height(resize_h);
            }
            return LRESULT(0);
        }
        m if m == WM_RICHEDIT_FOCUS || m == WM_RICHEDIT_BLUR => {
            let child = HWND(wparam.0 as *mut core::ffi::c_void);
            let dirty = with_popup(|p| {
                let Some(slot_id) = p
                    .rich_fields
                    .iter()
                    .find(|f| f.hwnd() == child)
                    .map(|f| f.slot_id())
                else {
                    return false;
                };
                let changed = if m == WM_RICHEDIT_FOCUS {
                    p.focus.focus_pointer(slot_id)
                } else if p.focus.focused() == Some(slot_id) {
                    p.focus.clear_focus()
                } else {
                    false
                };
                if changed {
                    let cur = p.focus.focused();
                    let _ = p.delegate.on_event(
                        PopupEvent::FocusChanged(cur),
                        &mut p.tree,
                        &mut p.focus,
                    );
                }
                changed
            })
            .unwrap_or(false);
            if dirty {
                sync_a11y(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == POPUP_TICK_TIMER_ID => {
            let (dirty, should_close, resize_h) = with_popup(|p| {
                let out = p
                    .delegate
                    .on_event(PopupEvent::Tick, &mut p.tree, &mut p.focus);
                sync_popup_timer(p);
                (out.dirty, out.close, out.resize_h_dip)
            })
            .unwrap_or((false, false, None));
            if should_close {
                close();
            } else if resize_h.is_some() || dirty {
                refresh_layout_with_height(resize_h);
            }
            return LRESULT(0);
        }
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
        WM_GETOBJECT => {
            let dpi = GetDpiForWindow(hwnd);
            let client_px = client_size(hwnd);
            let scale = frame::scale(dpi);
            let result = POPUP.with(|p| {
                let Ok(mut g) = p.try_borrow_mut() else {
                    return None;
                };
                let popup = g.as_mut()?;
                let update = a11y::build_tree_update(
                    Some(&popup.tree),
                    popup.focus.focused(),
                    scale,
                    client_px,
                    strings::APP_NAME,
                );
                let mut activation = StaticActivation(Some(update));
                popup
                    .a11y
                    .handle_wm_getobject(wparam, lparam, &mut activation)
            });
            if let Some(res) = result {
                return res.into();
            }
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            let is_focused = msg == WM_SETFOCUS;
            let queued = POPUP.with(|p| {
                let Ok(mut g) = p.try_borrow_mut() else {
                    return None;
                };
                let popup = g.as_mut()?;
                popup.a11y.update_window_focus_state(is_focused)
            });
            if let Some(ev) = queued {
                ev.raise();
            }
        }
        m if m == WM_A11Y_ACTION => {
            let requests = POPUP
                .with(|p| {
                    p.try_borrow()
                        .ok()
                        .and_then(|g| g.as_ref().map(|a| a.a11y_queue.drain()))
                })
                .unwrap_or_default();
            let (dirty, should_close, notify_owner, owner) = with_popup(|p| {
                let mut dirty = false;
                let mut should_close = false;
                let mut notify_owner = false;
                for req in requests {
                    let id = a11y::widget_id(req.target_node);
                    match req.action {
                        Action::Focus => {
                            if p.focus.focus_keyboard(id) {
                                dirty = true;
                                let fout = p.delegate.on_event(
                                    PopupEvent::FocusChanged(Some(id)),
                                    &mut p.tree,
                                    &mut p.focus,
                                );
                                dirty |= fout.dirty;
                            }
                        }
                        Action::Click => {
                            let out = p.delegate.on_event(
                                PopupEvent::Click(id),
                                &mut p.tree,
                                &mut p.focus,
                            );
                            dirty |= out.dirty;
                            should_close |= out.close;
                            notify_owner |= out.notify_owner;
                        }
                        _ => {}
                    }
                }
                (dirty, should_close, notify_owner, p.owner)
            })
            .unwrap_or((false, false, false, None));
            if should_close {
                close();
                return LRESULT(0);
            }
            if notify_owner {
                if let Some(ow) = owner {
                    let _ = PostMessageW(Some(ow), WM_POPUP_NOTIFY, WPARAM(0), LPARAM(0));
                }
            }
            if dirty {
                sync_a11y(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            return LRESULT(0);
        }
        WM_PAINT => {
            on_paint(hwnd);
            return LRESULT(0);
        }
        WM_DESTROY => {
            let popped = POPUP.with(|p| {
                let Ok(mut g) = p.try_borrow_mut() else {
                    return None;
                };
                if g.as_ref().is_some_and(|a| a.hwnd == hwnd) {
                    g.take()
                } else {
                    None
                }
            });
            if let Some(mut popup) = popped {
                if popup.tick_timer_ms.is_some() {
                    let _ = KillTimer(Some(hwnd), POPUP_TICK_TIMER_ID);
                }
                if !popup.dismissing {
                    let _ = popup.delegate.on_event(
                        PopupEvent::Closed,
                        &mut popup.tree,
                        &mut popup.focus,
                    );
                }
                drop(popup);
                super::reset_shared_if_idle();
            }
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tree::{style, A11yInfo, A11yRole, HitMode, Leaf};
    use crate::ui::window;
    use windows::Win32::UI::Accessibility::UiaRootObjectId;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, SendMessageW};

    struct NoopDelegate;
    impl PopupDelegate for NoopDelegate {
        fn on_event(
            &mut self,
            _event: PopupEvent,
            _tree: &mut Tree,
            _focus: &mut FocusModel,
        ) -> PopupOutcome {
            PopupOutcome::default()
        }
    }

    fn popup_test_tree() -> Tree {
        let mut tree = Tree::new(style::column());
        let item = tree.add_leaf(
            tree.root(),
            taffy::Style {
                size: style::size(180.0, 32.0),
                ..Default::default()
            },
            Leaf::Box,
        );
        tree.set_hit(item, HitMode::Interactive);
        tree.set_focusable(item, true);
        tree.set_a11y(
            item,
            A11yInfo {
                role: A11yRole::ListBoxOption,
                name: "Option 1".into(),
                clickable: true,
                requires_name: true,
                ..Default::default()
            },
        );
        tree
    }

    #[test]
    fn place_popup_flips_and_clamps_to_monitor_work_area() {
        let work = ScreenRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1032,
        };
        // Normal placement below-start.
        let anchor_mid = ScreenRect {
            left: 200,
            top: 200,
            right: 320,
            bottom: 232,
        };
        assert_eq!(
            place_popup(Some(anchor_mid), (200, 180), work, PopupAlign::BelowStart),
            (200, 236)
        );

        // Near bottom-right corner: flips above anchor and clamps left inside work area.
        let anchor_corner = ScreenRect {
            left: 1850,
            top: 980,
            right: 1910,
            bottom: 1012,
        };
        let (x, y) = place_popup(
            Some(anchor_corner),
            (240, 200),
            work,
            PopupAlign::BelowStart,
        );
        assert_eq!(x, 1910 - 240);
        assert_eq!(y, 980 - 4 - 200);
        assert!(x >= work.left && x + 240 <= work.right);
        assert!(y >= work.top && y + 200 <= work.bottom);

        // Negative secondary monitor coordinates work identically.
        let neg_work = ScreenRect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let (nx, ny) = place_popup(None, (300, 200), neg_work, PopupAlign::Center);
        assert!(nx >= neg_work.left && nx + 300 <= neg_work.right);
        assert!(ny >= neg_work.top && ny + 200 <= neg_work.bottom);
    }

    #[test]
    fn popup_repositions_in_work_area_and_dismisses_on_esc_and_deactivation() {
        crate::ui::init();
        assert!(window::open());
        window::pump();

        let work = monitor_work_area(None);
        // Anchor right against the bottom-right corner so popup must clamp & flip.
        let corner = ScreenRect {
            left: work.right - 40,
            top: work.bottom - 40,
            right: work.right - 4,
            bottom: work.bottom - 8,
        };
        let tree = popup_test_tree();
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));
        let hwnd = open(
            None,
            Some(corner),
            (220.0, 140.0),
            PopupAlign::BelowStart,
            tree,
            Box::new(NoopDelegate),
        )
        .expect("open popup");
        assert!(is_open());

        let mut rc = RECT::default();
        // SAFETY: `hwnd` is our newly created popup HWND.
        unsafe {
            assert!(GetWindowRect(hwnd, &mut rc).is_ok());
            assert!(rc.right <= work.right, "right overflow: {rc:?} vs {work:?}");
            assert!(
                rc.bottom <= work.bottom,
                "bottom overflow: {rc:?} vs {work:?}"
            );
            let lres = SendMessageW(
                hwnd,
                WM_GETOBJECT,
                Some(WPARAM(0)),
                Some(LPARAM(UiaRootObjectId as isize)),
            );
            assert_ne!(lres.0, 0);
            // Esc closes popup immediately.
            let _ = SendMessageW(
                hwnd,
                WM_KEYDOWN,
                Some(WPARAM(VK_ESCAPE.0 as usize)),
                Some(LPARAM(0)),
            );
        }
        window::pump();
        assert!(!is_open(), "Esc must close popup");

        // Second open: WA_INACTIVE must close popup too.
        let hwnd2 = open(
            None,
            Some(corner),
            (200.0, 120.0),
            PopupAlign::BelowStart,
            popup_test_tree(),
            Box::new(NoopDelegate),
        )
        .expect("open second popup");
        assert!(is_open());

        // WM_DPICHANGED must actually resize the popup to the scaled DIP size (no SWP_NOSIZE).
        let dpi_192 = ((192usize) << 16) | 192usize;
        let suggested = RECT {
            left: work.left + 40,
            top: work.top + 40,
            right: work.left + 440,
            bottom: work.top + 280,
        };
        // SAFETY: `hwnd2` is our live popup HWND and `suggested` is a valid stack RECT.
        unsafe {
            let _ = SendMessageW(
                hwnd2,
                WM_DPICHANGED,
                Some(WPARAM(dpi_192)),
                Some(LPARAM(&suggested as *const RECT as isize)),
            );
            let mut scaled_rc = RECT::default();
            assert!(GetWindowRect(hwnd2, &mut scaled_rc).is_ok());
            assert_eq!(scaled_rc.right - scaled_rc.left, 400);
            assert_eq!(scaled_rc.bottom - scaled_rc.top, 240);
            let _ = SendMessageW(
                hwnd2,
                WM_ACTIVATE,
                Some(WPARAM(WA_INACTIVE as usize)),
                Some(LPARAM(0)),
            );
        }
        window::pump();
        assert!(!is_open(), "WA_INACTIVE must close popup");

        window::close();
        window::pump();
        reset_shared();
    }
}
