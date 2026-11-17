//! Retained screen ownership (`Screen` + `ScreenHost`): each screen owns its
//! state and `Tree`, rebuilds and lays out only on demand when state, viewport,
//! hover, or preferences change (never in a continuous animation loop).

use taffy::prelude::*;

use super::a11y;
use super::focus::{EscapeOutcome, FocusModel, FocusScope};
use super::frame::TITLE_BAR_DIP;
use super::input::{KeyCode, KeyEvent};
use super::text::TextSystem;
use super::theme::Palette;
use super::tree::{style, Node, Tree, WidgetId};

/// Layout and theme inputs passed to a screen when building or rebuilding its tree.
pub struct ScreenEnv<'a> {
    pub width_dip: f32,
    pub height_dip: f32,
    pub hover: Option<WidgetId>,
    pub pressed: Option<WidgetId>,
    pub focused: Option<WidgetId>,
    pub palette: &'a Palette,
    pub text: &'a TextSystem,
}

/// Tree produced by `Screen::build`, with an optional topmost modal dialog node.
pub struct BuiltScreen {
    pub tree: Tree,
    pub modal_root: Option<Node>,
}

/// Modifier keys active during a pointer event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Response returned by screen-level pointer handlers (e.g., WeekGrid drag).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PointerReply {
    pub handled: bool,
    pub capture: bool,
    pub dragging: bool,
    /// Rebuild the retained UI tree from `Screen::build` and repaint.
    pub rebuild: bool,
    /// Update drag canvas/scroll directly on the existing `Tree` via `Screen::sync_drag_tree` without relayout.
    pub fast_redraw: bool,
}

/// Trait implemented by each Win32 Focus view/screen.
#[allow(dead_code)] // F3 screens implement all methods
pub trait Screen {
    fn build(&mut self, env: &ScreenEnv<'_>) -> BuiltScreen;
    fn on_click(&mut self, _id: WidgetId) -> bool {
        false
    }
    fn on_a11y_focus(&mut self, _id: WidgetId) -> bool {
        false
    }
    fn on_key(&mut self, _key: KeyEvent, _focus: &mut FocusModel) -> bool {
        false
    }
    fn on_wheel(&mut self, _delta_dip: f32) -> bool {
        false
    }
    fn on_wheel_at(&mut self, _x_dip: f32, _y_dip: f32, _dx_dip: f32, dy_dip: f32) -> bool {
        self.on_wheel(dy_dip)
    }
    fn on_escape(&mut self) -> bool {
        false
    }
    fn on_text_change(&mut self, _key: &str, _value: &str) -> bool {
        false
    }
    fn on_text_submit(&mut self, _key: &str, _value: &str) -> bool {
        false
    }
    fn on_pointer_down(
        &mut self,
        _id: WidgetId,
        _x_dip: f32,
        _y_dip: f32,
        _mods: KeyModifiers,
    ) -> PointerReply {
        PointerReply::default()
    }
    fn on_pointer_move(&mut self, _x_dip: f32, _y_dip: f32, _mods: KeyModifiers) -> PointerReply {
        PointerReply::default()
    }
    fn on_pointer_up(&mut self, _x_dip: f32, _y_dip: f32, _mods: KeyModifiers) -> PointerReply {
        PointerReply::default()
    }
    fn on_pointer_cancel(&mut self) -> bool {
        false
    }
    fn sync_drag_tree(&mut self, _tree: &mut Tree) {}
    fn wants_tick(&self) -> bool {
        false
    }
    fn on_tick(&mut self) -> bool {
        false
    }
    fn wants_autoscroll(&self) -> bool {
        false
    }
    fn on_autoscroll(&mut self) -> PointerReply {
        PointerReply::default()
    }
    fn on_popup_poll(&mut self) -> bool {
        false
    }
}

/// Owns the active `Screen` instance and its retained `Tree` with on-demand invalidation.
pub struct ScreenHost {
    screen: Box<dyn Screen>,
    tree: Tree,
    modal_root: Option<Node>,
    dirty: bool,
    last_w: f32,
    last_h: f32,
}

impl ScreenHost {
    pub fn mount(mut screen: Box<dyn Screen>, env: &ScreenEnv<'_>, focus: &mut FocusModel) -> Self {
        let mut built = screen.build(env);
        built.tree.layout(env.width_dip, env.height_dip, env.text);
        debug_assert_eq!(a11y::validate_tree_names(&built.tree), Ok(()));
        sync_focus(focus, &built.tree, built.modal_root);
        Self {
            screen,
            tree: built.tree,
            modal_root: built.modal_root,
            dirty: false,
            last_w: env.width_dip,
            last_h: env.height_dip,
        }
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn rebuild_if_dirty(&mut self, env: &ScreenEnv<'_>, focus: &mut FocusModel) -> bool {
        if !self.dirty {
            return false;
        }
        let mut built = self.screen.build(env);
        built.tree.layout(env.width_dip, env.height_dip, env.text);
        debug_assert_eq!(a11y::validate_tree_names(&built.tree), Ok(()));
        self.tree = built.tree;
        self.modal_root = built.modal_root;
        self.last_w = env.width_dip;
        self.last_h = env.height_dip;
        self.dirty = false;
        sync_focus(focus, &self.tree, self.modal_root);
        true
    }

    pub fn relayout(&mut self, width_dip: f32, height_dip: f32, text: &TextSystem) {
        self.tree.layout(width_dip, height_dip, text);
        self.last_w = width_dip;
        self.last_h = height_dip;
    }

    pub fn on_click(&mut self, id: WidgetId) -> bool {
        if self.screen.on_click(id) {
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn on_a11y_focus(&mut self, id: WidgetId) -> bool {
        if self.screen.on_a11y_focus(id) {
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn on_key(&mut self, ev: KeyEvent, focus: &mut FocusModel) -> (bool, bool) {
        if ev.key == KeyCode::Escape {
            if self.screen.on_escape() {
                let _ = focus.on_escape();
                self.dirty = true;
                return (true, true);
            }
            let out = focus.on_escape();
            return (out != EscapeOutcome::None, out != EscapeOutcome::None);
        }
        let handled = self.screen.on_key(ev, focus);
        if handled {
            self.dirty = true;
        }
        (handled, handled)
    }

    #[allow(dead_code)]
    pub fn on_wheel(&mut self, delta_dip: f32) -> bool {
        if self.screen.on_wheel(delta_dip) {
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn on_wheel_at(&mut self, x_dip: f32, y_dip: f32, dx_dip: f32, dy_dip: f32) -> bool {
        if self.screen.on_wheel_at(x_dip, y_dip, dx_dip, dy_dip) {
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn on_text_change(&mut self, key: &str, value: &str) -> bool {
        if self.screen.on_text_change(key, value) {
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn on_text_submit(&mut self, key: &str, value: &str) -> bool {
        if self.screen.on_text_submit(key, value) {
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn on_pointer_down(
        &mut self,
        id: WidgetId,
        x_dip: f32,
        y_dip: f32,
        mods: KeyModifiers,
    ) -> PointerReply {
        let reply = self.screen.on_pointer_down(id, x_dip, y_dip, mods);
        if reply.rebuild {
            self.dirty = true;
        }
        reply
    }

    pub fn on_pointer_move(&mut self, x_dip: f32, y_dip: f32, mods: KeyModifiers) -> PointerReply {
        let reply = self.screen.on_pointer_move(x_dip, y_dip, mods);
        if reply.rebuild {
            self.dirty = true;
        }
        reply
    }

    pub fn on_pointer_up(&mut self, x_dip: f32, y_dip: f32, mods: KeyModifiers) -> PointerReply {
        let reply = self.screen.on_pointer_up(x_dip, y_dip, mods);
        if reply.rebuild {
            self.dirty = true;
        }
        reply
    }

    pub fn on_pointer_cancel(&mut self) -> bool {
        if self.screen.on_pointer_cancel() {
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn sync_drag_tree(&mut self) {
        self.screen.sync_drag_tree(&mut self.tree);
    }

    pub fn wants_tick(&self) -> bool {
        self.screen.wants_tick()
    }

    pub fn on_tick(&mut self) -> bool {
        if self.screen.on_tick() {
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn wants_autoscroll(&self) -> bool {
        self.screen.wants_autoscroll()
    }

    pub fn on_autoscroll(&mut self) -> PointerReply {
        let reply = self.screen.on_autoscroll();
        if reply.rebuild {
            self.dirty = true;
        }
        reply
    }

    pub fn on_popup_poll(&mut self) -> bool {
        if self.screen.on_popup_poll() {
            self.dirty = true;
            true
        } else {
            false
        }
    }
}

fn sync_focus(focus: &mut FocusModel, tree: &Tree, modal_root: Option<Node>) {
    if let Some(card) = modal_root {
        let card_id = tree.id(card);
        focus.sync_from_tree(tree, |t, n| {
            super::widgets::dialog::scope_classifier(t, n, card)
        });
        if focus.active_scope() != FocusScope::Modal(card_id) {
            focus.push_modal(card_id);
        }
    } else {
        focus.sync_from_tree(tree, |_, _| FocusScope::Main);
        if let s @ FocusScope::Modal(_) = focus.active_scope() {
            let _ = focus.pop_scope(s);
        }
    }
}

/// Complete F2 widget showcase screen exercising every closed widget primitive in one tree.
#[allow(dead_code)] // used by automated gallery & DPI verification tests
pub struct WidgetGalleryScreen {
    pub toggle_on: bool,
    pub check_on: bool,
    pub seg_index: usize,
    pub select_index: Option<usize>,
    pub dialog_open: bool,
    pub list: super::widgets::virtual_list::VirtualListState,
    pub btn_open_dialog: Option<WidgetId>,
    pub toggle_id: Option<WidgetId>,
    pub check_id: Option<WidgetId>,
    pub seg_ids: Vec<WidgetId>,
    pub text_field_id: Option<WidgetId>,
    pub dialog_close_id: Option<WidgetId>,
    pub dialog_ok_id: Option<WidgetId>,
}

impl Default for WidgetGalleryScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl WidgetGalleryScreen {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            toggle_on: true,
            check_on: false,
            seg_index: 1,
            select_index: Some(0),
            dialog_open: false,
            list: super::widgets::virtual_list::VirtualListState::new(1_000, 32.0, 128.0),
            btn_open_dialog: None,
            toggle_id: None,
            check_id: None,
            seg_ids: Vec::new(),
            text_field_id: None,
            dialog_close_id: None,
            dialog_ok_id: None,
        }
    }
}

impl Screen for WidgetGalleryScreen {
    fn build(&mut self, env: &ScreenEnv<'_>) -> BuiltScreen {
        use super::text::Align;
        use super::theme::PROJECT_COLORS;
        use super::widgets::{
            button::{self, ButtonProps, ButtonVariant},
            charts, dialog,
            segmented::{self, SegmentedProps},
            select::{self, SelectProps},
            table::{self, SortDir, SortState, TableColumn, TableProps, TableRow},
            text_field::{self, TextFieldProps},
            toggle::{self, ToggleKind, ToggleProps},
            virtual_list::{self, VirtualListProps},
            Interact,
        };

        let p = env.palette;
        let mut tree = Tree::new(Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            padding: style::pad(20.0, TITLE_BAR_DIP + 16.0, 20.0, 20.0),
            gap: style::gap(12.0),
            size: style::size(env.width_dip, env.height_dip),
            ..Default::default()
        });
        let root = tree.root();

        let row = tree.add_container(
            root,
            Style {
                gap: style::gap(10.0),
                ..style::row()
            },
        );
        let mut btn_props = ButtonProps::new("Open dialog");
        btn_props.variant = ButtonVariant::Primary;
        btn_props.interact =
            Interact::from_state(self.btn_open_dialog, env.hover, env.pressed, false);
        let (_b, bid) = button::build(&mut tree, row, &btn_props, p);
        self.btn_open_dialog = Some(bid);

        let (_sw, sw_id) = toggle::build(
            &mut tree,
            row,
            &ToggleProps {
                kind: ToggleKind::Switch,
                label: "Auto-pause",
                aria_label: None,
                checked: self.toggle_on,
                disabled: false,
                interact: Interact::from_state(self.toggle_id, env.hover, env.pressed, false),
            },
            p,
        );
        self.toggle_id = Some(sw_id);

        let (_cb, cb_id) = toggle::build(
            &mut tree,
            row,
            &ToggleProps {
                kind: ToggleKind::Checkbox,
                label: "Detailed",
                aria_label: None,
                checked: self.check_on,
                disabled: false,
                interact: Interact::from_state(self.check_id, env.hover, env.pressed, false),
            },
            p,
        );
        self.check_id = Some(cb_id);

        let (_seg, seg_ids) = segmented::build(
            &mut tree,
            row,
            &SegmentedProps {
                group_name: "View range",
                options: &["Day", "Week", "Month"],
                selected: self.seg_index,
                hover: env.hover,
                disabled: false,
            },
            p,
        );
        self.seg_ids = seg_ids;

        let select_opts = ["Design", "Engineering", "Research"];
        let _ = select::build(
            &mut tree,
            row,
            &SelectProps {
                aria_label: "Category",
                options: &select_opts,
                selected: self.select_index,
                placeholder: super::strings::SELECT_PLACEHOLDER,
                min_width: 140.0,
                open: false,
                disabled: false,
                interact: Interact::default(),
            },
            p,
        );

        let (_tf, tf_id) = text_field::build_tree_node(
            &mut tree,
            root,
            &TextFieldProps {
                key: "gallery.notes",
                aria_label: "Notes",
                placeholder: "Notes",
                value: "",
                multiline: false,
                min_height: 32.0,
                focused: env.focused.is_some() && env.focused == self.text_field_id,
                error: false,
                disabled: false,
                sync_rev: 0,
            },
            p,
        );
        self.text_field_id = Some(tf_id);

        let cols = [
            TableColumn {
                label: "Segment",
                width: None,
                align: Align::Left,
                sortable: true,
            },
            TableColumn {
                label: "Duration",
                width: Some(90.0),
                align: Align::Right,
                sortable: true,
            },
        ];
        let rows = [
            TableRow {
                aria_name: "Primary block, 45m",
                cells: &["Primary block", "45m"],
                selected: true,
            },
            TableRow {
                aria_name: "Secondary block, 20m",
                cells: &["Secondary block", "20m"],
                selected: false,
            },
        ];
        let _ = table::build(
            &mut tree,
            root,
            &TableProps {
                aria_name: "Blocks",
                columns: &cols,
                rows: &rows,
                sort: Some(SortState {
                    col: 1,
                    dir: SortDir::Desc,
                }),
                hover: env.hover,
            },
            p,
        );

        let _ = virtual_list::build(
            &mut tree,
            root,
            &VirtualListProps {
                aria_name: "Virtual rows",
                state: self.list,
                hover: env.hover,
            },
            p,
            |t, parent, idx, selected| {
                let text = format!("Row {}", idx + 1);
                super::widgets::add_label(
                    t,
                    parent,
                    &text,
                    12.0,
                    if selected {
                        super::text::Weight::Medium
                    } else {
                        super::text::Weight::Regular
                    },
                    Align::Left,
                    p.text,
                );
                text
            },
        );

        let chart_row = tree.add_container(
            root,
            Style {
                gap: style::gap(12.0),
                ..style::row()
            },
        );
        let c0 = [(PROJECT_COLORS[0], 30.0), (PROJECT_COLORS[1], 12.0)];
        let c1 = [(PROJECT_COLORS[0], 18.0), (PROJECT_COLORS[2], 24.0)];
        let _ = charts::stacked_bars(&mut tree, chart_row, "Bars", &[&c0, &c1], 140.0, 56.0, p);
        let hm0 = [0.1, 0.5, 0.9];
        let _ = charts::heatmap(
            &mut tree,
            chart_row,
            "Heatmap",
            &[&hm0],
            PROJECT_COLORS[0],
            (14.0, 3.0),
            p,
        );
        let _ = charts::donut(&mut tree, chart_row, "Donut", &c0, 56.0, 8.0, p);
        let _ = charts::proportion_bar(&mut tree, chart_row, "Proportion", &c1, 120.0, 8.0, p);
        let _ = charts::spark(
            &mut tree,
            chart_row,
            "Spark",
            &[1.0, 3.0, 2.0, 5.0],
            (80.0, 24.0),
            p.accent,
            p,
        );
        let _ = charts::share_row(
            &mut tree,
            root,
            "Category A",
            "65%",
            0.65,
            PROJECT_COLORS[0],
            p,
        );

        let modal_root = if self.dialog_open {
            let dlg = dialog::build(&mut tree, root, "Confirm action", 360.0, env.hover, p);
            let mut ok_props = ButtonProps::new(super::strings::CONFIRM_LABEL);
            ok_props.variant = ButtonVariant::Primary;
            let (_ok, ok_id) = button::build(&mut tree, dlg.body, &ok_props, p);
            self.dialog_close_id = Some(dlg.close_id);
            self.dialog_ok_id = Some(ok_id);
            Some(dlg.card)
        } else {
            self.dialog_close_id = None;
            self.dialog_ok_id = None;
            None
        };

        BuiltScreen { tree, modal_root }
    }

    fn on_click(&mut self, id: WidgetId) -> bool {
        if Some(id) == self.btn_open_dialog {
            self.dialog_open = true;
            return true;
        }
        if Some(id) == self.toggle_id {
            self.toggle_on = !self.toggle_on;
            return true;
        }
        if Some(id) == self.check_id {
            self.check_on = !self.check_on;
            return true;
        }
        if let Some(pos) = self.seg_ids.iter().position(|x| *x == id) {
            self.seg_index = pos;
            return true;
        }
        if Some(id) == self.dialog_close_id || Some(id) == self.dialog_ok_id {
            self.dialog_open = false;
            return true;
        }
        false
    }

    fn on_escape(&mut self) -> bool {
        if self.dialog_open {
            self.dialog_open = false;
            true
        } else {
            false
        }
    }

    fn on_wheel(&mut self, delta_dip: f32) -> bool {
        self.list.scroll_by(-delta_dip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gallery_validates_every_widget_a11y_name_and_rebuilds_only_when_dirty() {
        let text = TextSystem::new().expect("dwrite");
        let palette = Palette::LIGHT;
        let env = ScreenEnv {
            width_dip: 1024.0,
            height_dip: 720.0,
            hover: None,
            pressed: None,
            focused: None,
            palette: &palette,
            text: &text,
        };
        let mut focus = FocusModel::new();
        let gallery = WidgetGalleryScreen::new();
        let mut host = ScreenHost::mount(Box::new(gallery), &env, &mut focus);

        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));
        // Not dirty -> idle calls must not rebuild.
        assert!(!host.rebuild_if_dirty(&env, &mut focus));

        // Simulate clicking first focusable button (Open dialog) and validate modal a11y + Esc.
        let first_focus = focus.tab(false).unwrap();
        assert!(host.on_click(first_focus));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));
        assert!(matches!(focus.active_scope(), FocusScope::Modal(_)));

        let (handled, dirty) = host.on_key(
            KeyEvent {
                key: KeyCode::Escape,
                shift: false,
                ctrl: false,
                alt: false,
            },
            &mut focus,
        );
        assert!(handled && dirty);
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(focus.active_scope(), FocusScope::Main);
    }
}
