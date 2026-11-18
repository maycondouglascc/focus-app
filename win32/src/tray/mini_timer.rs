//! Floating always-on-top mini timer (`WS_POPUP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST`).
//!
//! Uses shared Direct2D/DWrite + AccessKit, drags via `HTCAPTION`, validates saved
//! screen coordinates via `MonitorFromRect(MONITOR_DEFAULTTONULL)`, and runs a
//! `1s` UI timer ONLY while a manual timer is active (zero wakeups when stopped).

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use accesskit::Action;
use accesskit_windows::Adapter;
use taffy::prelude::*;
use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, InvalidateRect, MonitorFromRect, ScreenToClient, MONITOR_DEFAULTTONULL,
    PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
    VIRTUAL_KEY, VK_ESCAPE, VK_RETURN, VK_SHIFT, VK_SPACE, VK_TAB,
};
use windows::Win32::UI::Input::Pointer::{GetPointerInfo, POINTER_FLAG_FIRSTBUTTON, POINTER_INFO};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowRect, IsWindow,
    KillTimer, LoadCursorW, RegisterClassW, SetTimer, SetWindowPos, ShowWindow,
    SystemParametersInfoW, CS_HREDRAW, CS_VREDRAW, HTCAPTION, HTCLIENT, IDC_ARROW, PT_PEN,
    PT_TOUCH, SPI_GETWORKAREA, SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOWNOACTIVATE,
    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WINDOW_EX_STYLE, WM_CAPTURECHANGED, WM_CLOSE, WM_DESTROY,
    WM_DPICHANGED, WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_GETOBJECT, WM_KEYDOWN, WM_KILLFOCUS,
    WM_NCHITTEST, WM_NCLBUTTONDBLCLK, WM_PAINT, WM_POINTERCAPTURECHANGED, WM_POINTERDOWN,
    WM_POINTERENTER, WM_POINTERLEAVE, WM_POINTERUP, WM_POINTERUPDATE, WM_SETFOCUS,
    WM_SETTINGCHANGE, WM_SIZE, WM_SYSCOLORCHANGE, WM_THEMECHANGED, WM_TIMER, WNDCLASSW,
    WS_CLIPCHILDREN, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const WM_MOUSELEAVE: u32 = 0x02A3;

use crate::db::{Db, SettingsPatch, TimerStartInput};
use crate::db_thread::DbHandle;
use crate::format;
use crate::locale::SystemLocale;
use crate::ui::a11y::{self, A11yActionQueue, StaticActivation, WM_A11Y_ACTION};
use crate::ui::focus::{FocusModel, FocusScope};
use crate::ui::frame;
use crate::ui::input::{PointerButton, PointerKind, PointerState};
use crate::ui::prefs::{self, SystemPrefs};
use crate::ui::render::{reset_shared, DrawError, FrameOpts, Hover, Renderer};
use crate::ui::strings;
use crate::ui::text::{Align, Weight};
use crate::ui::theme::{parse_hex_color, Color, Palette, NO_PROJECT};
use crate::ui::tree::{style, A11yInfo, A11yRole, Leaf, PaintStyle, Tree, WidgetId};
use crate::ui::widgets::button::{self, ButtonProps, ButtonSize, ButtonVariant};
use crate::ui::widgets::{add_label, glyph, Interact};

const MINI_W_DIP: f32 = 228.0;
const MINI_H_DIP: f32 = 44.0;
const MINI_TICK_TIMER_ID: usize = 1;

struct MiniSnapshot {
    running_start_ms: Option<i64>,
    label: String,
    dot_color: Color,
    saved_pos: (Option<i32>, Option<i32>),
}

struct MiniWindow {
    hwnd: HWND,
    db_path: PathBuf,
    db_handle: DbHandle,
    renderer: Option<Renderer>,
    prefs: SystemPrefs,
    palette: Palette,
    tree: Tree,
    focus: FocusModel,
    pointer: PointerState,
    a11y_queue: A11yActionQueue,
    a11y: Adapter,
    snap: MiniSnapshot,
    id_toggle_btn: Option<WidgetId>,
    id_close_btn: Option<WidgetId>,
    tick_armed: bool,
    on_open_app: fn(),
    on_data_changed: fn(),
}

thread_local! {
    static MINI: RefCell<Option<MiniWindow>> = const { RefCell::new(None) };
}

fn system_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn with_mini<R>(f: impl FnOnce(&mut MiniWindow) -> R) -> Option<R> {
    MINI.with(|m| m.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn read_snapshot(db_path: &Path) -> MiniSnapshot {
    let Ok(db) = Db::open_readonly(db_path) else {
        return MiniSnapshot {
            running_start_ms: None,
            label: strings::MINI_TIMER_READY.into(),
            dot_color: NO_PROJECT,
            saved_pos: (None, None),
        };
    };
    let locale = SystemLocale::query();
    let saved_pos = db
        .load_settings(&locale)
        .map(|s| (s.mini_timer_x, s.mini_timer_y))
        .unwrap_or((None, None));
    if let Ok(Some(run)) = db.running_entry() {
        let (proj_name, dot_color) = match run
            .project_id
            .as_deref()
            .and_then(|pid| db.get_project(pid).ok())
        {
            Some(p) => (
                Some(p.name),
                parse_hex_color(&p.color).unwrap_or(NO_PROJECT),
            ),
            None => (None, NO_PROJECT),
        };
        let label = if !run.description.trim().is_empty() {
            run.description.trim().to_string()
        } else if let Some(name) = proj_name {
            name
        } else {
            strings::TRAY_MENU_NO_PROJECTS.to_string()
        };
        return MiniSnapshot {
            running_start_ms: Some(run.start),
            label,
            dot_color,
            saved_pos,
        };
    }
    MiniSnapshot {
        running_start_ms: None,
        label: strings::MINI_TIMER_READY.into(),
        dot_color: NO_PROJECT,
        saved_pos,
    }
}

/// Checks whether `(x, y, x+w, y+h)` has overlap with any monitor work area.
#[cfg(test)]
pub fn rect_visible_on_monitors(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    monitors: &[(i32, i32, i32, i32)],
) -> bool {
    let r = x + w.max(1);
    let b = y + h.max(1);
    monitors.iter().any(|&(ml, mt, mr, mb)| {
        let ix = (r.min(mr) - x.max(ml)).max(0);
        let iy = (b.min(mb) - y.max(mt)).max(0);
        ix >= 32.min(w) && iy >= 20.min(h)
    })
}

fn default_work_area_pos(w_px: i32, h_px: i32) -> (i32, i32) {
    let mut work = RECT {
        left: 0,
        top: 0,
        right: 1280,
        bottom: 800,
    };
    // SAFETY: `SPI_GETWORKAREA` fills stack `RECT`.
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut work as *mut RECT as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    (
        (work.right - w_px - 16).max(work.left),
        (work.bottom - h_px - 16).max(work.top),
    )
}

fn resolve_window_pos(saved: (Option<i32>, Option<i32>), w_px: i32, h_px: i32) -> (i32, i32) {
    if let (Some(x), Some(y)) = saved {
        let rc = RECT {
            left: x,
            top: y,
            right: x + w_px,
            bottom: y + h_px,
        };
        // SAFETY: `MonitorFromRect` validates if the rect falls on any active monitor.
        let hmon = unsafe { MonitorFromRect(&rc, MONITOR_DEFAULTTONULL) };
        if !hmon.is_invalid() {
            return (x, y);
        }
    }
    default_work_area_pos(w_px, h_px)
}

fn build_mini_tree(
    snap: &MiniSnapshot,
    now_ms: i64,
    palette: &Palette,
    hover: Option<WidgetId>,
    pressed: Option<WidgetId>,
) -> (Tree, Option<WidgetId>, Option<WidgetId>) {
    let mut tree = Tree::new(Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Row,
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::SpaceBetween),
        gap: style::gap(8.0),
        padding: style::pad(10.0, 4.0, 8.0, 4.0),
        size: Size {
            width: percent(1.0_f32),
            height: percent(1.0_f32),
        },
        ..Default::default()
    });
    let root = tree.root();
    tree.set_paint(
        root,
        PaintStyle {
            bg: Some(palette.surface),
            border_color: Some(palette.card_border),
            border_width: palette.border_width,
            radius: 10.0,
            clip: true,
        },
    );
    tree.set_a11y(
        root,
        A11yInfo {
            role: A11yRole::Group,
            name: strings::MINI_TIMER_NAME.into(),
            requires_name: true,
            ..Default::default()
        },
    );

    let left = tree.add_container(
        root,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            align_items: Some(AlignItems::Center),
            gap: style::gap(8.0),
            flex_grow: 1.0,
            min_size: Size {
                width: length(0.0_f32),
                height: auto(),
            },
            ..Default::default()
        },
    );

    let dot = tree.add_leaf(
        left,
        Style {
            flex_shrink: 0.0,
            size: Size {
                width: length(8.0_f32),
                height: length(8.0_f32),
            },
            ..Default::default()
        },
        Leaf::Box,
    );
    tree.set_paint(
        dot,
        PaintStyle {
            bg: Some(if snap.running_start_ms.is_some() {
                snap.dot_color
            } else {
                palette.faint
            }),
            radius: 4.0,
            ..Default::default()
        },
    );

    let clock_str = match snap.running_start_ms {
        Some(start) => format::clock((now_ms - start).max(0)),
        None => snap.label.clone(),
    };
    add_label(
        &mut tree,
        left,
        &clock_str,
        13.0,
        if snap.running_start_ms.is_some() {
            Weight::SemiBold
        } else {
            Weight::Medium
        },
        Align::Left,
        if snap.running_start_ms.is_some() {
            palette.text
        } else {
            palette.muted
        },
    );
    if snap.running_start_ms.is_some() && !snap.label.is_empty() {
        let preview = if snap.label.chars().count() > 14 {
            let mut s: String = snap.label.chars().take(13).collect();
            s.push('…');
            s
        } else {
            snap.label.clone()
        };
        add_label(
            &mut tree,
            left,
            &preview,
            11.5,
            Weight::Regular,
            Align::Left,
            palette.muted,
        );
    }

    let btns = tree.add_container(
        root,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            align_items: Some(AlignItems::Center),
            gap: style::gap(4.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
    );
    let is_running = snap.running_start_ms.is_some();
    let mut tprops = ButtonProps::new("");
    tprops.aria_label = Some(if is_running {
        strings::MINI_TIMER_STOP_A11Y
    } else {
        strings::MINI_TIMER_START_A11Y
    });
    tprops.icon = Some(if is_running { glyph::STOP } else { glyph::PLAY });
    tprops.variant = if is_running {
        ButtonVariant::Danger
    } else {
        ButtonVariant::Primary
    };
    tprops.size = ButtonSize::Icon;
    tprops.interact = Interact::from_state(None, hover, pressed, false);
    let (_, toggle_id) = button::build(&mut tree, btns, &tprops, palette);

    let mut cprops = ButtonProps::new("");
    cprops.aria_label = Some(strings::MINI_TIMER_HIDE_A11Y);
    cprops.icon = Some(glyph::CLOSE);
    cprops.variant = ButtonVariant::Ghost;
    cprops.size = ButtonSize::Icon;
    let (_, close_id) = button::build(&mut tree, btns, &cprops, palette);

    (tree, Some(toggle_id), Some(close_id))
}

unsafe fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    (rc.right - rc.left, rc.bottom - rc.top)
}

unsafe fn sync_a11y(hwnd: HWND) {
    let dpi = GetDpiForWindow(hwnd);
    let (cw, ch) = client_size(hwnd);
    let scale = frame::scale(dpi);
    let queued = MINI.with(|m| {
        let Ok(mut g) = m.try_borrow_mut() else {
            return None;
        };
        let win = g.as_mut()?;
        win.a11y.update_if_active(|| {
            a11y::build_tree_update(
                Some(&win.tree),
                win.focus.focused(),
                scale,
                (cw, ch),
                strings::MINI_TIMER_NAME,
            )
        })
    });
    if let Some(ev) = queued {
        ev.raise();
    }
}

fn rebuild_mini_in_place(win: &mut MiniWindow) {
    win.snap = read_snapshot(&win.db_path);
    let (tree, toggle_id, close_id) =
        build_mini_tree(&win.snap, system_now_ms(), &win.palette, None, None);
    win.tree = tree;
    win.id_toggle_btn = toggle_id;
    win.id_close_btn = close_id;
    // SAFETY: `win.hwnd` is live on this UI thread.
    let (cw, ch, scale) = unsafe {
        let (cw, ch) = client_size(win.hwnd);
        let scale = frame::scale(GetDpiForWindow(win.hwnd));
        (cw, ch, scale)
    };
    if let Some(r) = win.renderer.as_ref() {
        win.tree
            .layout(cw as f32 / scale, ch as f32 / scale, r.text());
    }
    let root_id = win.tree.id(win.tree.root());
    win.focus
        .sync_from_tree(&win.tree, |_, _| FocusScope::Popup(root_id));
    sync_mini_timer_interval(win);
}

fn sync_mini_timer_interval(win: &mut MiniWindow) {
    let want = win.snap.running_start_ms.is_some();
    if want == win.tick_armed {
        return;
    }
    // SAFETY: `win.hwnd` is owned by this UI thread.
    unsafe {
        if want {
            let _ = SetTimer(Some(win.hwnd), MINI_TICK_TIMER_ID, 1_000, None);
        } else {
            let _ = KillTimer(Some(win.hwnd), MINI_TICK_TIMER_ID);
        }
    }
    win.tick_armed = want;
}

fn trigger_mini_button(id: WidgetId) -> bool {
    let action = with_mini(|win| {
        if Some(id) == win.id_toggle_btn {
            Some((
                win.snap.running_start_ms.is_some(),
                win.db_handle.clone(),
                win.on_data_changed,
                false,
            ))
        } else if Some(id) == win.id_close_btn {
            Some((false, win.db_handle.clone(), win.on_data_changed, true))
        } else {
            None
        }
    })
    .flatten();

    let Some((was_running, db_handle, on_data_changed, hide_mini)) = action else {
        return false;
    };
    if hide_mini {
        let _ = db_handle.save_settings(
            SettingsPatch {
                mini_timer_enabled: Some(false),
                ..Default::default()
            },
            SystemLocale::query(),
        );
        close();
        on_data_changed();
        return true;
    }
    let now_ms = system_now_ms();
    if was_running {
        let _ = db_handle.stop_timers(now_ms);
    } else {
        let _ = db_handle.start_timer(
            TimerStartInput {
                project_id: None,
                description: String::new(),
                billable: true,
            },
            now_ms,
        );
    }
    on_data_changed();
    true
}

unsafe fn on_paint(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    BeginPaint(hwnd, &mut ps);
    let _ = EndPaint(hwnd, &ps);

    let dpi = GetDpiForWindow(hwnd);
    let (w, h) = client_size(hwnd);
    let scale = frame::scale(dpi);
    let retry = with_mini(|win| {
        if win.renderer.is_none() {
            win.renderer = Renderer::new(hwnd, w.max(1) as u32, h.max(1) as u32).ok();
            if let Some(r) = win.renderer.as_ref() {
                r.set_text_scale(win.prefs.text_scale);
            }
        }
        let palette = win.palette;
        let focus_ring = win.focus.focus_ring_rect(&win.tree);
        let Some(r) = win.renderer.as_mut() else {
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
            tree: Some(&win.tree),
            focus_ring,
            render_rich_slots: false,
        });
        match res {
            Ok(()) => false,
            Err(DrawError::DeviceLost) => {
                win.renderer = None;
                reset_shared();
                true
            }
            Err(_) => false,
        }
    })
    .unwrap_or(false);
    if retry {
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

unsafe extern "system" fn mini_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_CLOSE => {
            close();
            return LRESULT(0);
        }
        WM_NCHITTEST => {
            let sx = (lparam.0 & 0xFFFF) as i16 as i32;
            let sy = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut pt = POINT { x: sx, y: sy };
            let _ = ScreenToClient(hwnd, &mut pt);
            let scale = frame::scale(GetDpiForWindow(hwnd));
            let over_btn = with_mini(|win| {
                win.tree
                    .hit_test(pt.x as f32 / scale, pt.y as f32 / scale)
                    .is_some()
            })
            .unwrap_or(false);
            return LRESULT(if over_btn { HTCLIENT } else { HTCAPTION } as isize);
        }
        WM_NCLBUTTONDBLCLK if wparam.0 == HTCAPTION as usize => {
            let cb = with_mini(|win| win.on_open_app);
            if let Some(f) = cb {
                f();
            }
            return LRESULT(0);
        }
        WM_EXITSIZEMOVE => {
            let mut rc = RECT::default();
            if GetWindowRect(hwnd, &mut rc).is_ok() {
                if let Some((handle, cb)) =
                    with_mini(|win| (win.db_handle.clone(), win.on_data_changed))
                {
                    let _ = handle.save_settings(
                        SettingsPatch {
                            mini_timer_x: Some(Some(rc.left)),
                            mini_timer_y: Some(Some(rc.top)),
                            ..Default::default()
                        },
                        SystemLocale::query(),
                    );
                    cb();
                }
            }
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == MINI_TICK_TIMER_ID => {
            with_mini(rebuild_mini_in_place);
            sync_a11y(hwnd);
            let _ = InvalidateRect(Some(hwnd), None, false);
            return LRESULT(0);
        }
        WM_SIZE => {
            let (w, h) = client_size(hwnd);
            let dpi = GetDpiForWindow(hwnd);
            let scale = frame::scale(dpi);
            with_mini(|win| {
                if let Some(r) = win.renderer.as_mut() {
                    if r.resize(w.max(1) as u32, h.max(1) as u32).is_err() {
                        win.renderer = None;
                        reset_shared();
                    }
                }
                if let Some(r) = win.renderer.as_ref() {
                    win.tree
                        .layout(w as f32 / scale, h as f32 / scale, r.text());
                }
            });
            sync_a11y(hwnd);
            let _ = InvalidateRect(Some(hwnd), None, false);
            return LRESULT(0);
        }
        WM_DPICHANGED => {
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
        WM_SETTINGCHANGE | WM_THEMECHANGED | WM_SYSCOLORCHANGE => {
            let sys_prefs = SystemPrefs::query();
            let changed = with_mini(|win| {
                let palette = Palette::for_system(sys_prefs.high_contrast);
                let text_changed = win
                    .renderer
                    .as_ref()
                    .is_some_and(|r| r.set_text_scale(sys_prefs.text_scale));
                let changed = win.prefs != sys_prefs || win.palette != palette || text_changed;
                win.prefs = sys_prefs;
                win.palette = palette;
                if changed {
                    rebuild_mini_in_place(win);
                }
                changed
            })
            .unwrap_or(false);
            if changed {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
        WM_POINTERENTER | WM_POINTERUPDATE => {
            if let Some((_, kind, _, x, y)) = query_pointer(hwnd, wparam) {
                if kind == PointerKind::Mouse {
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut t);
                }
                if with_mini(|win| {
                    let hit = win.tree.hit_test(x, y);
                    win.pointer.on_move(hit, x, y).visual_changed
                })
                .unwrap_or(false)
                {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                return LRESULT(0);
            }
        }
        WM_POINTERDOWN => {
            if let Some((pid, kind, btn, x, y)) = query_pointer(hwnd, wparam) {
                let (cap, fchg, vchg) = with_mini(|win| {
                    let hit = win.tree.hit_test(x, y);
                    let out = win.pointer.on_down(pid, btn, hit, x, y);
                    let fchg = match hit {
                        Some(id) => win.focus.focus_pointer(id),
                        None => win.focus.clear_focus(),
                    };
                    (out.request_capture, fchg, out.visual_changed || fchg)
                })
                .unwrap_or((false, false, false));
                if cap && kind == PointerKind::Mouse {
                    SetCapture(hwnd);
                }
                if fchg {
                    sync_a11y(hwnd);
                }
                if vchg {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                return LRESULT(0);
            }
        }
        WM_POINTERUP => {
            if let Some((pid, _, _, x, y)) = query_pointer(hwnd, wparam) {
                let (rel, clicked, vchg) = with_mini(|win| {
                    let hit = win.tree.hit_test(x, y);
                    let out = win.pointer.on_up(pid, PointerButton::Primary, hit, x, y);
                    (out.release_capture, out.clicked, out.visual_changed)
                })
                .unwrap_or((false, None, false));
                if rel {
                    let _ = ReleaseCapture();
                }
                if let Some(id) = clicked {
                    let _ = trigger_mini_button(id);
                } else if vchg {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                return LRESULT(0);
            }
        }
        WM_POINTERLEAVE | WM_MOUSELEAVE | WM_POINTERCAPTURECHANGED | WM_CAPTURECHANGED => {
            let vchg = with_mini(|win| {
                if msg == WM_POINTERLEAVE || msg == WM_MOUSELEAVE {
                    win.pointer.on_leave().visual_changed
                } else {
                    win.pointer.on_capture_lost().visual_changed
                }
            })
            .unwrap_or(false);
            if vchg {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wparam.0 as u16);
            if vk == VK_ESCAPE {
                close();
                return LRESULT(0);
            }
            if vk == VK_TAB {
                let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                if with_mini(|win| {
                    let prev = win.focus.focused();
                    let next = win.focus.tab(shift);
                    next != prev
                })
                .unwrap_or(false)
                {
                    sync_a11y(hwnd);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                return LRESULT(0);
            }
            if vk == VK_RETURN || vk == VK_SPACE {
                let fid = with_mini(|win| win.focus.focused()).flatten();
                if let Some(id) = fid {
                    let _ = trigger_mini_button(id);
                }
                return LRESULT(0);
            }
        }
        WM_GETOBJECT => {
            let dpi = GetDpiForWindow(hwnd);
            let (cw, ch) = client_size(hwnd);
            let scale = frame::scale(dpi);
            let res = MINI.with(|m| {
                let Ok(mut g) = m.try_borrow_mut() else {
                    return None;
                };
                let win = g.as_mut()?;
                let update = a11y::build_tree_update(
                    Some(&win.tree),
                    win.focus.focused(),
                    scale,
                    (cw, ch),
                    strings::MINI_TIMER_NAME,
                );
                let mut activation = StaticActivation(Some(update));
                win.a11y
                    .handle_wm_getobject(wparam, lparam, &mut activation)
            });
            if let Some(r) = res {
                return r.into();
            }
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            let queued = MINI.with(|m| {
                let Ok(mut g) = m.try_borrow_mut() else {
                    return None;
                };
                g.as_mut()?
                    .a11y
                    .update_window_focus_state(msg == WM_SETFOCUS)
            });
            if let Some(ev) = queued {
                ev.raise();
            }
        }
        m if m == WM_A11Y_ACTION => {
            let reqs = MINI
                .with(|m| {
                    m.try_borrow()
                        .ok()
                        .and_then(|g| g.as_ref().map(|w| w.a11y_queue.drain()))
                })
                .unwrap_or_default();
            for req in reqs {
                let id = a11y::widget_id(req.target_node);
                match req.action {
                    Action::Focus => {
                        with_mini(|w| {
                            let _ = w.focus.focus_keyboard(id);
                        });
                    }
                    Action::Click => {
                        let _ = trigger_mini_button(id);
                    }
                    _ => {}
                }
            }
            return LRESULT(0);
        }
        WM_PAINT => {
            on_paint(hwnd);
            return LRESULT(0);
        }
        WM_DESTROY => {
            let popped = MINI.with(|m| {
                let mut g = m.borrow_mut();
                if g.as_ref().is_some_and(|w| w.hwnd == hwnd) {
                    g.take()
                } else {
                    None
                }
            });
            if let Some(win) = popped {
                if win.tick_armed {
                    let _ = KillTimer(Some(hwnd), MINI_TICK_TIMER_ID);
                }
                drop(win);
                crate::ui::reset_shared_if_idle();
            }
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

pub fn is_open() -> bool {
    let hwnd = MINI.with(|m| m.try_borrow().ok().and_then(|g| g.as_ref().map(|w| w.hwnd)));
    // SAFETY: `IsWindow` validates any HWND safely.
    hwnd.is_some_and(|h| unsafe { IsWindow(Some(h)).as_bool() })
}

pub fn open(
    db_path: &Path,
    db_handle: DbHandle,
    on_open_app: fn(),
    on_data_changed: fn(),
) -> Option<HWND> {
    if is_open() {
        refresh();
        return MINI.with(|m| m.borrow().as_ref().map(|w| w.hwnd));
    }

    let snap = read_snapshot(db_path);
    let sys_prefs = SystemPrefs::query();
    let palette = Palette::for_system(sys_prefs.high_contrast);
    let (mut tree, toggle_id, close_id) =
        build_mini_tree(&snap, system_now_ms(), &palette, None, None);

    // SAFETY: called on UI thread; registers class, creates tool topmost popup, and mounts renderer/a11y.
    unsafe {
        let hinstance = HINSTANCE(GetModuleHandleW(None).ok()?.0);
        let class = w!("FocusMiniTimerWindow");
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(mini_wnd_proc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_ARROW).ok()?,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);

        let initial_scale = 1.0f32;
        let est_w = (MINI_W_DIP * initial_scale).ceil() as i32;
        let est_h = (MINI_H_DIP * initial_scale).ceil() as i32;
        let (x, y) = resolve_window_pos(snap.saved_pos, est_w, est_h);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0 | WS_EX_NOREDIRECTIONBITMAP.0),
            class,
            w!("Focus Mini Timer"),
            WS_POPUP | WS_CLIPCHILDREN,
            x,
            y,
            est_w,
            est_h,
            None,
            None,
            Some(hinstance),
            None,
        )
        .ok()?;

        let dpi = GetDpiForWindow(hwnd);
        let scale = frame::scale(dpi);
        let w_px = (MINI_W_DIP * scale).ceil() as i32;
        let h_px = (MINI_H_DIP * scale).ceil() as i32;
        let (rx, ry) = resolve_window_pos(snap.saved_pos, w_px, h_px);
        let _ = SetWindowPos(
            hwnd,
            None,
            rx,
            ry,
            w_px,
            h_px,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
        prefs::apply_rounded_corners(hwnd);

        let (cw, ch) = client_size(hwnd);
        let renderer = match Renderer::new(hwnd, cw.max(1) as u32, ch.max(1) as u32) {
            Ok(r) => {
                r.set_text_scale(sys_prefs.text_scale);
                tree.layout(cw as f32 / scale, ch as f32 / scale, r.text());
                Some(r)
            }
            Err(_) => None,
        };
        let root_id = tree.id(tree.root());
        let mut focus = FocusModel::new();
        focus.sync_from_tree(&tree, |_, _| FocusScope::Popup(root_id));
        focus.push_popup(root_id);

        let a11y_queue = A11yActionQueue::new(hwnd);
        let a11y = Adapter::new(hwnd, false, a11y_queue.handler());

        MINI.with(|m| {
            let mut win = MiniWindow {
                hwnd,
                db_path: db_path.to_path_buf(),
                db_handle,
                renderer,
                prefs: sys_prefs,
                palette,
                tree,
                focus,
                pointer: PointerState::default(),
                a11y_queue,
                a11y,
                snap,
                id_toggle_btn: toggle_id,
                id_close_btn: close_id,
                tick_armed: false,
                on_open_app,
                on_data_changed,
            };
            sync_mini_timer_interval(&mut win);
            *m.borrow_mut() = Some(win);
        });

        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = InvalidateRect(Some(hwnd), None, false);
        Some(hwnd)
    }
}

pub fn refresh() {
    let hwnd = with_mini(|win| {
        rebuild_mini_in_place(win);
        win.hwnd
    });
    if let Some(h) = hwnd {
        // SAFETY: `h` belongs to this UI thread.
        unsafe {
            sync_a11y(h);
            let _ = InvalidateRect(Some(h), None, false);
        }
    }
}

pub fn close() {
    let popped = MINI.with(|m| m.borrow_mut().take());
    let Some(win) = popped else {
        return;
    };
    let hwnd = win.hwnd;
    if win.tick_armed {
        // SAFETY: `hwnd` belongs to this UI thread.
        unsafe {
            let _ = KillTimer(Some(hwnd), MINI_TICK_TIMER_ID);
        }
    }
    drop(win);
    // SAFETY: `hwnd` is owned by this UI thread.
    unsafe {
        if IsWindow(Some(hwnd)).as_bool() {
            let _ = DestroyWindow(hwnd);
        }
    }
    crate::ui::reset_shared_if_idle();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::text::TextSystem;

    #[test]
    fn mini_timer_trees_pass_a11y_and_unplugged_monitor_resets() {
        let text = TextSystem::new().expect("dwrite");
        let stopped = MiniSnapshot {
            running_start_ms: None,
            label: strings::MINI_TIMER_READY.into(),
            dot_color: NO_PROJECT,
            saved_pos: (None, None),
        };
        let (mut t1, _, _) = build_mini_tree(&stopped, 10_000, &Palette::LIGHT, None, None);
        t1.layout(MINI_W_DIP, MINI_H_DIP, &text);
        assert_eq!(a11y::validate_tree_names(&t1), Ok(()));

        let running = MiniSnapshot {
            running_start_ms: Some(1_000),
            label: "Deep focus work".into(),
            dot_color: Palette::LIGHT.accent,
            saved_pos: (None, None),
        };
        let (mut t2, _, _) = build_mini_tree(&running, 75_000, &Palette::LIGHT, None, None);
        t2.layout(MINI_W_DIP, MINI_H_DIP, &text);
        assert_eq!(a11y::validate_tree_names(&t2), Ok(()));

        let monitors = [(0, 0, 1920, 1040)];
        assert!(rect_visible_on_monitors(1600, 950, 228, 44, &monitors));
        assert!(!rect_visible_on_monitors(2500, 100, 228, 44, &monitors));
    }
}
