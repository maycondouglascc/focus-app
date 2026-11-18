//! Native `WS_POPUP` activity-session inspector with expandable per-app window details
//! and "Log as time entry" action.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use taffy::prelude::*;
use windows::Win32::Foundation::HWND;

use crate::format::{duration, short_duration, time};
use crate::ui::a11y;
use crate::ui::focus::FocusModel;
use crate::ui::popup::{PopupDelegate, PopupEvent, PopupOutcome};
use crate::ui::strings;
use crate::ui::text::{Align, Weight};
use crate::ui::theme::{app_color, Palette};
use crate::ui::tree::{
    style, A11yInfo, A11yRole, HitMode, Leaf, Node, PaintStyle, Rect, Tree, WidgetId,
};
use crate::ui::widgets::button::{self, ButtonProps, ButtonSize, ButtonVariant};
use crate::ui::widgets::popover;
use crate::ui::widgets::{add_label, glyph, interactive_a11y};

use super::calendar_layout::{session_label, ActivitySession};

#[derive(Clone, Debug)]
pub struct LogSessionRequest {
    pub session: ActivitySession,
    pub description: String,
    #[allow(dead_code)] // anchor retained for callers inspecting origin rect
    pub anchor_dip: Rect,
}

struct BuiltPopoverTree {
    tree: Tree,
    apps_vp: Option<Node>,
    toggles: Vec<(WidgetId, String)>,
    log_id: Option<WidgetId>,
}

struct ActivityPopoverDelegate {
    session: ActivitySession,
    is_24h: bool,
    palette: Palette,
    open_apps: HashSet<String>,
    anchor_dip: Rect,
    scroll_y: f32,
    apps_vp: Option<Node>,
    app_toggle_ids: Vec<(WidgetId, String)>,
    log_btn_id: Option<WidgetId>,
    on_log: Rc<RefCell<Option<LogSessionRequest>>>,
}

impl ActivityPopoverDelegate {
    fn estimate_height_dip(session: &ActivitySession, open_apps: &HashSet<String>) -> f32 {
        let mut h = 24.0 + 46.0 + 38.0; // padding + header + footer button
        for a in &session.apps {
            h += 38.0;
            if open_apps.contains(&a.name) {
                let rows = a.details.len().min(8) as f32;
                h += 8.0 + rows * 24.0;
            }
        }
        h.clamp(160.0, 520.0)
    }

    fn build_tree(
        session: &ActivitySession,
        is_24h: bool,
        open_apps: &HashSet<String>,
        scroll_y: f32,
        palette: &Palette,
    ) -> BuiltPopoverTree {
        let mut tree = popover::build_tree("Activity session", 360.0, palette);
        tree.set_root_height_fill();
        let root = tree.root();

        // Header
        let head = tree.add_container(
            root,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                flex_shrink: 0.0,
                gap: style::gap(2.0),
                ..Default::default()
            },
        );
        let title = format!(
            "{} – {}",
            time(session.start, is_24h),
            time(session.end, is_24h)
        );
        add_label(
            &mut tree,
            head,
            &title,
            14.0,
            Weight::SemiBold,
            Align::Left,
            palette.text,
        );
        let span = session.end - session.start;
        let breaks_ms = (span - session.active_ms).max(0);
        let sub = if breaks_ms >= 60_000 {
            format!(
                "{} active · {} of short breaks",
                duration(session.active_ms),
                duration(breaks_ms)
            )
        } else {
            format!("{} active", duration(session.active_ms))
        };
        add_label(
            &mut tree,
            head,
            &sub,
            12.0,
            Weight::Regular,
            Align::Left,
            palette.muted,
        );

        let apps_vp = tree.add_container(
            root,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_shrink: 1.0,
                flex_basis: length(0.0_f32),
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                min_size: Size {
                    width: length(0.0_f32),
                    height: length(0.0_f32),
                },
                ..Default::default()
            },
        );
        tree.set_paint(
            apps_vp,
            PaintStyle {
                bg: None,
                border_color: None,
                border_width: 0.0,
                radius: 0.0,
                clip: true,
            },
        );
        tree.set_scrollable_y(apps_vp, true);
        tree.set_scroll(apps_vp, 0.0, scroll_y);

        let apps_col = tree.add_container(
            apps_vp,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                flex_shrink: 0.0,
                gap: style::gap(4.0),
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );

        let max_ms = session.apps.iter().map(|a| a.ms).max().unwrap_or(1).max(1) as f32;
        let mut toggle_ids = Vec::new();

        for (app_idx, a) in session.apps.iter().enumerate() {
            let has_details = !a.details.is_empty();
            let expanded = open_apps.contains(&a.name);

            let block = tree.add_container(
                apps_col,
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Column,
                    flex_shrink: 0.0,
                    gap: style::gap(4.0),
                    size: Size {
                        width: percent(1.0_f32),
                        height: auto(),
                    },
                    ..Default::default()
                },
            );

            let row_btn = tree.add_container(
                block,
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Column,
                    gap: style::gap(4.0),
                    padding: style::pad(6.0, 4.0, 6.0, 4.0),
                    size: Size {
                        width: percent(1.0_f32),
                        height: auto(),
                    },
                    ..Default::default()
                },
            );
            let top = tree.add_container(
                row_btn,
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    align_items: Some(AlignItems::Center),
                    justify_content: Some(JustifyContent::SpaceBetween),
                    gap: style::gap(8.0),
                    size: Size {
                        width: percent(1.0_f32),
                        height: auto(),
                    },
                    ..Default::default()
                },
            );
            let left = tree.add_container(
                top,
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    align_items: Some(AlignItems::Center),
                    gap: style::gap(6.0),
                    flex_grow: 1.0,
                    flex_basis: length(0.0_f32),
                    ..Default::default()
                },
            );
            let c = app_color(&a.name);
            let dot = tree.add_container(
                left,
                Style {
                    size: style::size(8.0, 8.0),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
            tree.set_paint(
                dot,
                PaintStyle {
                    bg: Some(c),
                    border_color: None,
                    border_width: 0.0,
                    radius: 4.0,
                    clip: false,
                },
            );
            add_label(
                &mut tree,
                left,
                &a.name,
                12.5,
                Weight::Medium,
                Align::Left,
                palette.text,
            );
            if has_details {
                add_label(
                    &mut tree,
                    left,
                    a.details.len().to_string(),
                    11.0,
                    Weight::Regular,
                    Align::Left,
                    palette.faint,
                );
            }

            let right = tree.add_container(
                top,
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    align_items: Some(AlignItems::Center),
                    gap: style::gap(6.0),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
            let dur = short_duration(a.ms);
            add_label(
                &mut tree,
                right,
                &dur,
                12.0,
                Weight::Regular,
                Align::Right,
                palette.muted,
            );
            if has_details {
                tree.add_leaf(
                    right,
                    Style::default(),
                    Leaf::Icon {
                        glyph: if expanded {
                            glyph::CHEVRON_DOWN
                        } else {
                            glyph::CHEVRON_RIGHT
                        },
                        size: 10.0,
                        color: palette.muted,
                    },
                );
                let bid = tree.set_key(row_btn, &format!("act_pop.app.{app_idx}.{}", a.name));
                tree.set_hit(row_btn, HitMode::Interactive);
                tree.set_focusable(row_btn, true);
                let mut info =
                    interactive_a11y(A11yRole::Button, format!("{}, {dur}", a.name), false);
                info.expanded = Some(expanded);
                tree.set_a11y(row_btn, info);
                toggle_ids.push((bid, a.name.clone()));
            } else {
                tree.set_a11y(
                    row_btn,
                    A11yInfo {
                        role: A11yRole::Group,
                        name: format!("{}, {dur}", a.name),
                        requires_name: true,
                        ..Default::default()
                    },
                );
            }

            // Proportional bar
            let track = tree.add_container(
                row_btn,
                Style {
                    size: Size {
                        width: percent(1.0_f32),
                        height: length(4.0_f32),
                    },
                    ..Default::default()
                },
            );
            tree.set_paint(
                track,
                PaintStyle {
                    bg: Some(palette.chart_empty_cell),
                    border_color: None,
                    border_width: 0.0,
                    radius: 2.0,
                    clip: true,
                },
            );
            let fill = tree.add_container(
                track,
                Style {
                    size: Size {
                        width: percent((a.ms as f32 / max_ms).clamp(0.02, 1.0)),
                        height: percent(1.0_f32),
                    },
                    ..Default::default()
                },
            );
            tree.set_paint(
                fill,
                PaintStyle {
                    bg: Some(c),
                    border_color: None,
                    border_width: 0.0,
                    radius: 2.0,
                    clip: false,
                },
            );

            if has_details && expanded {
                let details_box = tree.add_container(
                    block,
                    Style {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Column,
                        padding: style::pad(14.0, 2.0, 4.0, 6.0),
                        gap: style::gap(4.0),
                        size: Size {
                            width: percent(1.0_f32),
                            height: auto(),
                        },
                        ..Default::default()
                    },
                );
                for d in a.details.iter().take(8) {
                    let dr = tree.add_container(
                        details_box,
                        Style {
                            display: Display::Flex,
                            flex_direction: FlexDirection::Row,
                            align_items: Some(AlignItems::Center),
                            justify_content: Some(JustifyContent::SpaceBetween),
                            gap: style::gap(8.0),
                            size: Size {
                                width: percent(1.0_f32),
                                height: auto(),
                            },
                            ..Default::default()
                        },
                    );
                    let dl = if d.sub.is_empty() {
                        d.label.clone()
                    } else {
                        format!("{} ({})", d.label, d.sub)
                    };
                    add_label(
                        &mut tree,
                        dr,
                        &dl,
                        11.5,
                        Weight::Regular,
                        Align::Left,
                        palette.text,
                    );
                    let ddur = short_duration(d.ms);
                    add_label(
                        &mut tree,
                        dr,
                        &ddur,
                        11.5,
                        Weight::Regular,
                        Align::Right,
                        palette.muted,
                    );
                }
            }
        }

        // Footer
        let foot = tree.add_container(
            root,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                flex_shrink: 0.0,
                justify_content: Some(JustifyContent::FlexStart),
                padding: style::pad(0.0, 4.0, 0.0, 0.0),
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );
        let mut log_p = ButtonProps::new(strings::CAL_POPOVER_LOG_BTN);
        log_p.icon = Some(glyph::ADD);
        log_p.variant = ButtonVariant::Primary;
        log_p.size = ButtonSize::Small;
        let (log_node, _) = button::build(&mut tree, foot, &log_p, palette);
        let log_id = tree.set_key(log_node, "act_pop.log");
        debug_assert_eq!(a11y::validate_tree_names(&tree), Ok(()));

        BuiltPopoverTree {
            tree,
            apps_vp: Some(apps_vp),
            toggles: toggle_ids,
            log_id: Some(log_id),
        }
    }

    fn apply_wheel(&mut self, tree: &mut Tree, dy: f32) -> PopupOutcome {
        let Some(vp) = self.apps_vp else {
            return PopupOutcome::default();
        };
        let max_s = tree.scroll_max_y(vp);
        let next = (self.scroll_y - dy).clamp(0.0, max_s);
        if (next - self.scroll_y).abs() > 0.2 {
            self.scroll_y = next;
            tree.set_scroll(vp, 0.0, next);
            return PopupOutcome {
                handled: true,
                dirty: true,
                ..Default::default()
            };
        }
        PopupOutcome::default()
    }
}

impl PopupDelegate for ActivityPopoverDelegate {
    fn on_event(
        &mut self,
        event: PopupEvent,
        tree: &mut Tree,
        _focus: &mut FocusModel,
    ) -> PopupOutcome {
        match event {
            PopupEvent::Click(id) => {
                if Some(id) == self.log_btn_id {
                    let desc = session_label(&self.session);
                    *self.on_log.borrow_mut() = Some(LogSessionRequest {
                        session: self.session.clone(),
                        description: desc,
                        anchor_dip: self.anchor_dip,
                    });
                    return PopupOutcome {
                        handled: true,
                        dirty: false,
                        close: true,
                        notify_owner: true,
                        resize_h_dip: None,
                    };
                }
                if let Some((_, app_name)) = self.app_toggle_ids.iter().find(|(tid, _)| *tid == id)
                {
                    if !self.open_apps.remove(app_name) {
                        self.open_apps.insert(app_name.clone());
                    }
                    let built = Self::build_tree(
                        &self.session,
                        self.is_24h,
                        &self.open_apps,
                        self.scroll_y,
                        &self.palette,
                    );
                    *tree = built.tree;
                    self.apps_vp = built.apps_vp;
                    self.app_toggle_ids = built.toggles;
                    self.log_btn_id = built.log_id;
                    let new_h = Self::estimate_height_dip(&self.session, &self.open_apps);
                    return PopupOutcome {
                        handled: true,
                        dirty: true,
                        close: false,
                        notify_owner: false,
                        resize_h_dip: Some(new_h),
                    };
                }
                PopupOutcome::default()
            }
            PopupEvent::Wheel(dy) => self.apply_wheel(tree, dy),
            PopupEvent::FocusChanged(fid) => {
                if let (Some(vp), Some(id)) = (self.apps_vp, fid) {
                    if let Some(target) = tree.node_by_id(id) {
                        if let Some(new_y) = tree.scroll_node_into_view(vp, target) {
                            self.scroll_y = new_y;
                            return PopupOutcome {
                                handled: true,
                                dirty: true,
                                ..Default::default()
                            };
                        }
                    }
                    let clamped = self.scroll_y.clamp(0.0, tree.scroll_max_y(vp));
                    if (clamped - self.scroll_y).abs() > 0.2 {
                        self.scroll_y = clamped;
                        tree.set_scroll(vp, 0.0, clamped);
                        return PopupOutcome {
                            handled: false,
                            dirty: true,
                            ..Default::default()
                        };
                    }
                }
                PopupOutcome::default()
            }
            PopupEvent::Key(ev) => match ev.key {
                crate::ui::input::KeyCode::PageUp => self.apply_wheel(tree, 180.0),
                crate::ui::input::KeyCode::PageDown => self.apply_wheel(tree, -180.0),
                _ => PopupOutcome::default(),
            },
            PopupEvent::Closed
            | PopupEvent::TextChange(_, _)
            | PopupEvent::TextSubmit(_, _)
            | PopupEvent::Tick => PopupOutcome::default(),
        }
    }
}

/// Opens a transient `WS_POPUP` activity inspector anchored at `anchor_dip` inside `owner`.
/// Returns a shared slot populated with `LogSessionRequest` if the user clicks "Log as time entry".
pub fn open_activity_popover(
    owner: HWND,
    anchor_dip: Rect,
    session: ActivitySession,
    is_24h: bool,
    palette: &Palette,
) -> Option<Rc<RefCell<Option<LogSessionRequest>>>> {
    let open_apps = HashSet::new();
    let est_h = ActivityPopoverDelegate::estimate_height_dip(&session, &open_apps);
    let built = ActivityPopoverDelegate::build_tree(&session, is_24h, &open_apps, 0.0, palette);
    let log_slot = Rc::new(RefCell::new(None));
    let delegate = ActivityPopoverDelegate {
        session,
        is_24h,
        palette: *palette,
        open_apps,
        anchor_dip,
        scroll_y: 0.0,
        apps_vp: built.apps_vp,
        app_toggle_ids: built.toggles,
        log_btn_id: built.log_id,
        on_log: Rc::clone(&log_slot),
    };
    popover::open(
        owner,
        anchor_dip,
        (360.0, est_h),
        crate::ui::popup::PopupAlign::BelowStart,
        built.tree,
        Box::new(delegate),
    )?;
    Some(log_slot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{from_local, MIN};
    use crate::ui::screens::calendar_layout::{SessionApp, SessionDetail};
    use crate::ui::text::TextSystem;

    #[test]
    fn activity_popover_builds_valid_tree_expands_details_and_logs_entry() {
        let text = TextSystem::new().expect("dwrite");
        let day = from_local(2026, 11, 18, 9, 0, 0, 0);
        let session = ActivitySession {
            key: "s1".into(),
            start: day,
            end: day + 45 * MIN,
            active_ms: 42 * MIN,
            apps: vec![SessionApp {
                name: "Visual Studio Code".into(),
                ms: 42 * MIN,
                details: vec![SessionDetail {
                    label: "week_grid.rs".into(),
                    sub: "trackerApp".into(),
                    ms: 30 * MIN,
                }],
            }],
        };

        let mut open = HashSet::new();
        let built =
            ActivityPopoverDelegate::build_tree(&session, true, &open, 0.0, &Palette::LIGHT);
        let mut tree = built.tree;
        let toggles = built.toggles;
        let log_id = built.log_id;
        tree.layout(360.0, 240.0, &text);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));
        assert_eq!(toggles.len(), 1);
        assert!(log_id.is_some());

        let log_slot = Rc::new(RefCell::new(None));
        let mut del = ActivityPopoverDelegate {
            session: session.clone(),
            is_24h: true,
            palette: Palette::LIGHT,
            open_apps: open.clone(),
            anchor_dip: Rect {
                x: 100.0,
                y: 100.0,
                w: 80.0,
                h: 40.0,
            },
            scroll_y: 0.0,
            apps_vp: built.apps_vp,
            app_toggle_ids: toggles.clone(),
            log_btn_id: log_id,
            on_log: Rc::clone(&log_slot),
        };
        let mut focus = FocusModel::new();
        // Expand row -> increases height & returns resize_h_dip.
        let out = del.on_event(PopupEvent::Click(toggles[0].0), &mut tree, &mut focus);
        assert!(out.dirty && out.resize_h_dip.is_some());
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));
        open.insert("Visual Studio Code".into());

        // Click Log as time entry -> closes & populates log request.
        let out2 = del.on_event(
            PopupEvent::Click(del.log_btn_id.unwrap()),
            &mut tree,
            &mut focus,
        );
        assert!(out2.close && out2.notify_owner);
        assert_eq!(
            log_slot.borrow().as_ref().unwrap().description,
            "week_grid.rs"
        );
    }

    #[test]
    fn keyboard_tab_space_and_enter_operate_activity_popover_window() {
        use crate::ui::popup;
        use crate::ui::render::reset_shared;
        use crate::ui::window;
        use windows::Win32::Foundation::{LPARAM, RECT, WPARAM};
        use windows::Win32::UI::Input::KeyboardAndMouse::{VK_RETURN, VK_SPACE, VK_TAB};
        use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, SendMessageW, WM_KEYDOWN};

        crate::ui::init();
        assert!(window::open());
        window::pump();
        let owner = window::hwnd().expect("owner hwnd");

        let day = from_local(2026, 11, 18, 9, 0, 0, 0);
        let session = ActivitySession {
            key: "s_kb".into(),
            start: day,
            end: day + 45 * MIN,
            active_ms: 42 * MIN,
            apps: vec![SessionApp {
                name: "Visual Studio Code".into(),
                ms: 42 * MIN,
                details: vec![SessionDetail {
                    label: "week_grid.rs".into(),
                    sub: "trackerApp".into(),
                    ms: 30 * MIN,
                }],
            }],
        };
        let log_slot = open_activity_popover(
            owner,
            Rect {
                x: 80.0,
                y: 80.0,
                w: 80.0,
                h: 40.0,
            },
            session,
            true,
            &Palette::LIGHT,
        )
        .expect("open activity popover");
        assert!(popup::is_open());

        // SAFETY: `pop_hwnd` is our thread-owned active popup window.
        unsafe {
            let pop_hwnd = popup::active_hwnd().expect("active popup hwnd");
            assert_ne!(pop_hwnd, owner);
            let mut rc_before = RECT::default();
            assert!(GetWindowRect(pop_hwnd, &mut rc_before).is_ok());

            // Focus starts on the expandable app row; Space expands it, Tab moves to
            // "Log as time entry", and Enter activates it.
            let _ = SendMessageW(
                pop_hwnd,
                WM_KEYDOWN,
                Some(WPARAM(VK_SPACE.0 as usize)),
                Some(LPARAM(0)),
            );
            let mut rc_expanded = RECT::default();
            assert!(GetWindowRect(pop_hwnd, &mut rc_expanded).is_ok());
            assert!(
                rc_expanded.bottom - rc_expanded.top > rc_before.bottom - rc_before.top,
                "Space should expand app details and grow popup height"
            );
            let _ = SendMessageW(
                pop_hwnd,
                WM_KEYDOWN,
                Some(WPARAM(VK_TAB.0 as usize)),
                Some(LPARAM(0)),
            );
            let _ = SendMessageW(
                pop_hwnd,
                WM_KEYDOWN,
                Some(WPARAM(VK_RETURN.0 as usize)),
                Some(LPARAM(0)),
            );
        }
        window::pump();
        assert!(
            !popup::is_open(),
            "Enter on Log button should close popover"
        );
        assert_eq!(
            log_slot.borrow().as_ref().map(|r| r.description.as_str()),
            Some("week_grid.rs")
        );

        window::close();
        window::pump();
        reset_shared();
    }

    #[test]
    fn activity_popover_scrolls_many_apps_and_keeps_footer_and_tab_focus_visible() {
        let text = TextSystem::new().expect("dwrite");
        let day = from_local(2026, 11, 18, 9, 0, 0, 0);
        let apps: Vec<SessionApp> = (0..20)
            .map(|i| SessionApp {
                name: format!("App {i}"),
                ms: 3 * MIN,
                details: vec![SessionDetail {
                    label: format!("Window {i}"),
                    sub: String::new(),
                    ms: 3 * MIN,
                }],
            })
            .collect();
        let session = ActivitySession {
            key: "s_many".into(),
            start: day,
            end: day + 60 * MIN,
            active_ms: 60 * MIN,
            apps,
        };
        let open = HashSet::new();
        let est_h = ActivityPopoverDelegate::estimate_height_dip(&session, &open);
        assert!((est_h - 520.0).abs() < 0.1);

        let built =
            ActivityPopoverDelegate::build_tree(&session, true, &open, 0.0, &Palette::LIGHT);
        let mut tree = built.tree;
        let toggles = built.toggles;
        let log_id = built.log_id;
        tree.layout(360.0, est_h, &text);
        let vp = built.apps_vp.unwrap();
        let max_s = tree.scroll_max_y(vp);
        assert!(max_s > 200.0, "expected scrollable overflow, got {max_s}");

        // Footer button must stay visible inside the 520 DIP popup (never clipped off bottom).
        let log_rect = tree.rect(tree.node_by_id(log_id.unwrap()).unwrap());
        assert!(
            log_rect.y + log_rect.h <= est_h,
            "footer clipped: {log_rect:?}"
        );

        let mut del = ActivityPopoverDelegate {
            session,
            is_24h: true,
            palette: Palette::LIGHT,
            open_apps: open,
            anchor_dip: Rect {
                x: 0.0,
                y: 0.0,
                w: 50.0,
                h: 30.0,
            },
            scroll_y: 0.0,
            apps_vp: Some(vp),
            app_toggle_ids: toggles.clone(),
            log_btn_id: log_id,
            on_log: Rc::new(RefCell::new(None)),
        };
        let mut focus = FocusModel::new();
        focus.sync_from_tree(&tree, |_, _| crate::ui::focus::FocusScope::Main);

        // Wheel scrolls the app list down cleanly.
        let w_out = del.on_event(PopupEvent::Wheel(-180.0), &mut tree, &mut focus);
        assert!(w_out.dirty && del.scroll_y > 100.0);

        // Tabbing to the last app row scrolls it into view within `vp`.
        let last_toggle_id = toggles.last().unwrap().0;
        let f_out = del.on_event(
            PopupEvent::FocusChanged(Some(last_toggle_id)),
            &mut tree,
            &mut focus,
        );
        assert!(f_out.dirty);
        let vp_rect = tree.rect(vp);
        let item_rect = tree.rect(tree.node_by_id(last_toggle_id).unwrap());
        assert!(
            item_rect.y >= vp_rect.y && item_rect.y + item_rect.h <= vp_rect.y + vp_rect.h,
            "focused item not visible: item={item_rect:?} vp={vp_rect:?}"
        );
    }
}
