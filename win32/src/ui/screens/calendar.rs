//! Weekly Calendar screen hosting `TimerBar`, week navigation toolbar,
//! `Mix`/`Time`/`Activity` mode segmented switcher, interactive `WeekGrid`,
//! `EntryEditor` modal, and `ActivityPopover` popup.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use taffy::prelude::*;

use crate::db::{Project, SharedStore, TimeEntry};
use crate::format::{hm, money, range_label};
use crate::stats::{earnings_by_currency, entry_ms};
use crate::time::{add_days, iso_week, start_of_week_with_first};
use crate::ui::input::KeyCode;
use crate::ui::screen::{BuiltScreen, KeyModifiers, PointerReply, Screen, ScreenEnv};
use crate::ui::strings;
use crate::ui::text::{Align, Weight};
use crate::ui::theme::Palette;
use crate::ui::tree::{style, Node, PaintStyle, Tree, WidgetId};
use crate::ui::widgets::button::{self, ButtonProps, ButtonSize, ButtonVariant};
use crate::ui::widgets::segmented::{self, SegmentedProps};
use crate::ui::widgets::{add_label, glyph, Interact};
use crate::ui::window;

use super::activity_popover::{open_activity_popover, LogSessionRequest};
use super::entry_editor::{EntryDraft, EntryEditorOutcome, EntryEditorState};
use super::screen_root_tree;
use super::timer_bar::TimerBarState;
use super::week_grid::{CalendarMode, GridAction, WeekGridState};

pub struct CalendarScreen {
    store: SharedStore,
    anchor_ms: i64,
    pub timer: TimerBarState,
    pub grid: WeekGridState,
    editor: Option<EntryEditorState>,
    act_log_slot: Option<Rc<RefCell<Option<LogSessionRequest>>>>,

    id_prev_week: Option<WidgetId>,
    id_today: Option<WidgetId>,
    id_next_week: Option<WidgetId>,
    id_mode_ids: Vec<WidgetId>,
    id_empty_new_proj: Option<WidgetId>,

    cached_projects: Vec<Project>,
    on_new_project: Option<Box<dyn FnMut()>>,
}

impl CalendarScreen {
    pub fn new(store: SharedStore) -> Self {
        let now_ms = store.borrow().now_ms();
        Self {
            store,
            anchor_ms: now_ms,
            timer: TimerBarState::new(),
            grid: WeekGridState::new(),
            editor: None,
            act_log_slot: None,
            id_prev_week: None,
            id_today: None,
            id_next_week: None,
            id_mode_ids: Vec::new(),
            id_empty_new_proj: None,
            cached_projects: Vec::new(),
            on_new_project: None,
        }
    }

    pub fn with_on_new_project(mut self, f: impl FnMut() + 'static) -> Self {
        self.on_new_project = Some(Box::new(f));
        self
    }

    fn apply_grid_action(&mut self, action: GridAction, palette: &Palette) {
        match action {
            GridAction::OpenEntry(draft, _) => {
                crate::ui::popup::close();
                self.act_log_slot = None;
                self.editor = Some(EntryEditorState::new(draft));
            }
            GridAction::UpdateEntry { id, patch } => {
                let _ = self.store.borrow_mut().update_entry(&id, patch);
            }
            GridAction::OpenActivity(session, anchor_dip) => {
                self.editor = None;
                if let Some(hwnd) = window::hwnd() {
                    let is_24h = {
                        let st = self.store.borrow();
                        let loc = st.system_locale().clone();
                        st.get_settings()
                            .map(|s| s.time_format.is_24h(&loc))
                            .unwrap_or(loc.is_24h)
                    };
                    self.act_log_slot =
                        open_activity_popover(hwnd, anchor_dip, session, is_24h, palette);
                }
            }
        }
    }

    pub fn build_content(
        &mut self,
        tree: &mut Tree,
        overlay_root: Node,
        parent: Node,
        env: &ScreenEnv<'_>,
    ) -> Option<Node> {
        let p = env.palette;

        let (
            now_ms,
            first_day_mon0,
            is_24h,
            scroll_to_hour,
            week_start,
            week_end,
            projects,
            entries,
            activity,
            active_ms,
        ) = {
            let st = self.store.borrow();
            let now = st.now_ms();
            let loc = st.system_locale().clone();
            let settings = st
                .get_settings()
                .unwrap_or_else(|_| crate::db::Settings::defaults_for(&loc));
            let first_day_mon0 = settings.first_day_of_week.resolve_mon0(&loc);
            let is_24h = settings.time_format.is_24h(&loc);
            let week_start = start_of_week_with_first(self.anchor_ms, first_day_mon0);
            let week_end = add_days(week_start, 7);
            let projects = st.list_projects().unwrap_or_default();
            let entries = st.list_entries(week_start, week_end).unwrap_or_default();
            let activity = if self.grid.mode != CalendarMode::Time {
                st.list_activity(week_start, week_end).unwrap_or_default()
            } else {
                Vec::new()
            };
            let active_ms = if self.grid.mode != CalendarMode::Time {
                st.active_union_ms(week_start, week_end).unwrap_or(0)
            } else {
                0
            };
            (
                now,
                first_day_mon0,
                is_24h,
                settings.day_start_hour,
                week_start,
                week_end,
                projects,
                entries,
                activity,
                active_ms,
            )
        };
        self.cached_projects = projects.clone();
        let projects_by_id: HashMap<String, Project> = projects
            .iter()
            .map(|pr| (pr.id.clone(), pr.clone()))
            .collect();

        // While describing a brand-new draft in the editor modal, show a preview block on grid.
        let mut shown_entries = entries.clone();
        if let Some(ref ed) = self.editor {
            if ed.draft.id.is_none() {
                if let Ok((s, e)) = ed.parse_times() {
                    shown_entries.push(TimeEntry {
                        id: "__draft".into(),
                        project_id: ed.project_id.clone(),
                        description: ed.description.clone(),
                        start: s,
                        end: e,
                        billable: ed.billable,
                        source: ed.draft.source.clone(),
                    });
                }
            }
        }

        let wrap = tree.add_container(
            parent,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                padding: style::pad(20.0, 14.0, 20.0, 16.0),
                gap: style::gap(12.0),
                size: Size {
                    width: percent(1.0_f32),
                    height: percent(1.0_f32),
                },
                ..Default::default()
            },
        );

        // 1. Timer bar
        self.timer.build(tree, wrap, &self.store, &projects, env);

        // 2. Week toolbar
        let toolbar = tree.add_container(
            wrap,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::SpaceBetween),
                gap: style::gap(12.0),
                flex_shrink: 0.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );

        let nav_left = tree.add_container(
            toolbar,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                gap: style::gap(6.0),
                ..Default::default()
            },
        );
        let mut prev_p = ButtonProps::new("");
        prev_p.aria_label = Some(strings::CAL_PREV_WEEK);
        prev_p.icon = Some(glyph::CHEVRON_LEFT);
        prev_p.variant = ButtonVariant::Ghost;
        prev_p.size = ButtonSize::Small;
        prev_p.interact = Interact::from_state(self.id_prev_week, env.hover, env.pressed, false);
        let (_, pid) = button::build(tree, nav_left, &prev_p, p);
        self.id_prev_week = Some(pid);

        let cur_week_start = start_of_week_with_first(now_ms, first_day_mon0);
        let this_week = week_start == cur_week_start;
        let mut today_p = ButtonProps::new(strings::CAL_DAY_TODAY);
        today_p.size = ButtonSize::Small;
        today_p.disabled = this_week;
        today_p.interact = Interact::from_state(self.id_today, env.hover, env.pressed, this_week);
        let (_, tid) = button::build(tree, nav_left, &today_p, p);
        self.id_today = Some(tid);

        let mut next_p = ButtonProps::new("");
        next_p.aria_label = Some(strings::CAL_NEXT_WEEK);
        next_p.icon = Some(glyph::CHEVRON_RIGHT);
        next_p.variant = ButtonVariant::Ghost;
        next_p.size = ButtonSize::Small;
        next_p.interact = Interact::from_state(self.id_next_week, env.hover, env.pressed, false);
        let (_, nid) = button::build(tree, nav_left, &next_p, p);
        self.id_next_week = Some(nid);

        let week_title = if this_week {
            format!("{} · W{}", strings::CAL_THIS_WEEK, iso_week(week_start))
        } else {
            format!(
                "{} · W{}",
                range_label(week_start, week_end),
                iso_week(week_start)
            )
        };
        add_label(
            tree,
            nav_left,
            &week_title,
            15.0,
            Weight::SemiBold,
            Align::Left,
            p.text,
        );

        // Middle/Right summary stats + mode switcher
        let nav_right = tree.add_container(
            toolbar,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                gap: style::gap(14.0),
                ..Default::default()
            },
        );
        if self.grid.mode != CalendarMode::Activity {
            let total_ms: i64 = entries
                .iter()
                .map(|e| entry_ms(e, week_start, week_end, now_ms))
                .sum();
            add_label(
                tree,
                nav_right,
                format!("{} tracked", hm(total_ms)),
                12.5,
                Weight::Medium,
                Align::Right,
                p.text,
            );
            let proj_refs: HashMap<&str, &Project> =
                projects.iter().map(|pr| (pr.id.as_str(), pr)).collect();
            for (cur, val) in
                earnings_by_currency(&entries, &proj_refs, week_start, week_end, now_ms)
            {
                add_label(
                    tree,
                    nav_right,
                    format!("{} earned", money(val, &cur)),
                    12.5,
                    Weight::Medium,
                    Align::Right,
                    p.done,
                );
            }
        }
        if self.grid.mode != CalendarMode::Time {
            add_label(
                tree,
                nav_right,
                format!("{} active", hm(active_ms)),
                12.5,
                Weight::Regular,
                Align::Right,
                p.muted,
            );
        }

        let mode_opts = [
            strings::CAL_MODE_MIX,
            strings::CAL_MODE_TIME,
            strings::CAL_MODE_ACTIVITY,
        ];
        let (_, mids) = segmented::build(
            tree,
            nav_right,
            &SegmentedProps {
                group_name: "Calendar shows",
                options: &mode_opts,
                selected: self.grid.mode.index(),
                hover: env.hover,
                disabled: false,
            },
            p,
        );
        self.id_mode_ids = mids;

        self.id_empty_new_proj = None;
        if entries.is_empty() && self.grid.mode != CalendarMode::Activity && self.editor.is_none() {
            let hint_bar = tree.add_container(
                wrap,
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    align_items: Some(AlignItems::Center),
                    justify_content: Some(JustifyContent::SpaceBetween),
                    padding: style::pad(12.0, 8.0, 12.0, 8.0),
                    gap: style::gap(12.0),
                    flex_shrink: 0.0,
                    size: Size {
                        width: percent(1.0_f32),
                        height: auto(),
                    },
                    ..Default::default()
                },
            );
            tree.set_paint(
                hint_bar,
                PaintStyle {
                    bg: Some(p.surface),
                    border_color: Some(p.card_border),
                    border_width: p.border_width,
                    radius: 8.0,
                    clip: false,
                },
            );
            let msg = if projects.is_empty() {
                "Log your first hours — type what you're working on above, or drag on the calendar."
            } else {
                "Nothing logged this week — press Start above or drag on the calendar to add time."
            };
            add_label(
                tree,
                hint_bar,
                msg,
                12.5,
                Weight::Regular,
                Align::Left,
                p.muted,
            );
            if projects.is_empty() {
                let mut np = ButtonProps::new("Create a project");
                np.icon = Some(glyph::ADD);
                np.size = ButtonSize::Small;
                let (_, bid) = button::build(tree, hint_bar, &np, p);
                self.id_empty_new_proj = Some(bid);
            }
        }

        // 3. Weekly grid canvas
        self.grid.build(
            tree,
            wrap,
            week_start,
            &shown_entries,
            &activity,
            &projects_by_id,
            now_ms,
            scroll_to_hour,
            is_24h,
            env,
        );

        // 4. Optional EntryEditor modal overlay
        self.editor
            .as_mut()
            .map(|ed| ed.build(tree, overlay_root, &self.store, &projects, env))
    }

    pub fn handle_click(&mut self, id: WidgetId, palette: &Palette) -> bool {
        if let Some(ref mut ed) = self.editor {
            let (handled, out) = ed.on_click(id, &self.store, &self.cached_projects, palette);
            match out {
                EntryEditorOutcome::Closed
                | EntryEditorOutcome::Saved
                | EntryEditorOutcome::Deleted => {
                    self.editor = None;
                    self.timer.sync_from_store(&self.store);
                }
                EntryEditorOutcome::None => {}
            }
            if handled {
                return true;
            }
        }
        if self
            .timer
            .on_click(id, &self.store, &self.cached_projects, palette)
        {
            return true;
        }
        if Some(id) == self.id_prev_week {
            self.anchor_ms = add_days(self.anchor_ms, -7);
            return true;
        }
        if Some(id) == self.id_today {
            self.anchor_ms = self.store.borrow().now_ms();
            return true;
        }
        if Some(id) == self.id_next_week {
            self.anchor_ms = add_days(self.anchor_ms, 7);
            return true;
        }
        if let Some(pos) = self.id_mode_ids.iter().position(|x| *x == id) {
            self.grid.mode = CalendarMode::from_index(pos);
            if self.grid.mode == CalendarMode::Activity {
                self.editor = None;
            }
            return true;
        }
        if Some(id) == self.id_empty_new_proj {
            if let Some(cb) = self.on_new_project.as_mut() {
                cb();
            }
            return true;
        }
        false
    }

    pub fn handle_key(&mut self, key: KeyCode) -> bool {
        if self.editor.is_some() {
            return false;
        }
        if self.grid.on_key(key) {
            if let Some(action) = self.grid.pending_action.take() {
                self.apply_grid_action(action, &Palette::LIGHT);
            }
            return true;
        }
        false
    }

    pub fn handle_text_change(&mut self, key: &str, value: &str) -> bool {
        if let Some(ref mut ed) = self.editor {
            return ed.on_text_change(key, value);
        }
        self.timer.on_text_change(key, value, &self.store)
    }

    pub fn handle_text_submit(&mut self, key: &str, value: &str) -> bool {
        if let Some(ref mut ed) = self.editor {
            let out = ed.on_text_submit(key, value, &self.store);
            if matches!(out, EntryEditorOutcome::Saved | EntryEditorOutcome::Closed) {
                self.editor = None;
                self.timer.sync_from_store(&self.store);
            }
            return true;
        }
        self.timer.on_text_submit(key, value, &self.store)
    }

    pub fn handle_escape(&mut self) -> bool {
        if self.grid.cancel_drag() {
            return true;
        }
        if self.editor.is_some() {
            self.editor = None;
            return true;
        }
        false
    }

    pub fn handle_pointer_down(
        &mut self,
        id: WidgetId,
        x_dip: f32,
        y_dip: f32,
        mods: KeyModifiers,
    ) -> PointerReply {
        if self.editor.is_some() {
            return PointerReply::default();
        }
        self.grid.on_pointer_down(id, x_dip, y_dip, mods)
    }

    pub fn handle_pointer_move(
        &mut self,
        x_dip: f32,
        y_dip: f32,
        mods: KeyModifiers,
    ) -> PointerReply {
        self.grid.on_pointer_move(x_dip, y_dip, mods)
    }

    pub fn handle_pointer_up(
        &mut self,
        x_dip: f32,
        y_dip: f32,
        mods: KeyModifiers,
        palette: &Palette,
    ) -> PointerReply {
        let reply = self.grid.on_pointer_up(x_dip, y_dip, mods);
        if let Some(action) = self.grid.pending_action.take() {
            self.apply_grid_action(action, palette);
        }
        reply
    }

    pub fn handle_popup_poll(&mut self) -> bool {
        let mut changed = self.timer.poll_popup(&self.store, &self.cached_projects);
        if let Some(ref mut ed) = self.editor {
            changed |= ed.poll_popup(&self.cached_projects);
        }
        let act_req = self
            .act_log_slot
            .as_ref()
            .and_then(|s| s.borrow_mut().take());
        if let Some(req) = act_req {
            self.act_log_slot = None;
            self.editor = Some(EntryEditorState::new(EntryDraft {
                id: None,
                project_id: None,
                description: req.description,
                start: req.session.start,
                end: Some(req.session.end),
                billable: false,
                source: "activity".into(),
            }));
            changed = true;
        } else if self.act_log_slot.is_some() && !crate::ui::popup::is_open() {
            self.act_log_slot = None;
        }
        changed
    }

    pub fn wants_tick(&self) -> bool {
        self.timer.is_running(&self.store)
    }
}

impl Screen for CalendarScreen {
    fn build(&mut self, env: &ScreenEnv<'_>) -> BuiltScreen {
        let mut tree = screen_root_tree(env);
        let root = tree.root();
        let modal_root = self.build_content(&mut tree, root, root, env);
        BuiltScreen { tree, modal_root }
    }

    fn on_click(&mut self, id: WidgetId) -> bool {
        self.handle_click(id, &Palette::LIGHT)
    }

    fn on_key(
        &mut self,
        ev: crate::ui::input::KeyEvent,
        focus: &mut crate::ui::focus::FocusModel,
    ) -> bool {
        self.grid.set_canvas_focused(
            focus.focused().is_some() && focus.focused() == self.grid.canvas_id,
        );
        self.handle_key(ev.key)
    }

    fn on_text_change(&mut self, key: &str, value: &str) -> bool {
        self.handle_text_change(key, value)
    }

    fn on_text_submit(&mut self, key: &str, value: &str) -> bool {
        self.handle_text_submit(key, value)
    }

    fn on_escape(&mut self) -> bool {
        self.handle_escape()
    }

    fn on_pointer_down(
        &mut self,
        id: WidgetId,
        x_dip: f32,
        y_dip: f32,
        mods: KeyModifiers,
    ) -> PointerReply {
        self.handle_pointer_down(id, x_dip, y_dip, mods)
    }

    fn on_pointer_move(&mut self, x_dip: f32, y_dip: f32, mods: KeyModifiers) -> PointerReply {
        self.handle_pointer_move(x_dip, y_dip, mods)
    }

    fn on_pointer_up(&mut self, x_dip: f32, y_dip: f32, mods: KeyModifiers) -> PointerReply {
        self.handle_pointer_up(x_dip, y_dip, mods, &Palette::LIGHT)
    }

    fn on_pointer_cancel(&mut self) -> bool {
        self.grid.cancel_drag()
    }

    fn sync_drag_tree(&mut self, tree: &mut Tree) {
        self.grid.sync_drag_tree(tree);
    }

    fn on_wheel(&mut self, delta_dip: f32) -> bool {
        if self.editor.is_some() {
            return false;
        }
        self.grid.on_wheel_at(0.0, 0.0, delta_dip)
    }

    fn on_wheel_at(&mut self, x_dip: f32, y_dip: f32, _dx_dip: f32, dy_dip: f32) -> bool {
        if self.editor.is_some() {
            return false;
        }
        self.grid.on_wheel_at(x_dip, y_dip, dy_dip)
    }

    fn wants_tick(&self) -> bool {
        self.wants_tick()
    }

    fn on_tick(&mut self) -> bool {
        self.wants_tick()
    }

    fn wants_autoscroll(&self) -> bool {
        self.grid.wants_autoscroll()
    }

    fn on_autoscroll(&mut self) -> PointerReply {
        self.grid.on_autoscroll()
    }

    fn on_popup_poll(&mut self) -> bool {
        self.handle_popup_poll()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::shared_store;
    use crate::demo::DemoStore;
    use crate::locale::SystemLocale;
    use crate::ui::a11y;
    use crate::ui::focus::{FocusModel, FocusScope};
    use crate::ui::screen::ScreenHost;
    use crate::ui::text::TextSystem;

    #[test]
    fn calendar_screen_validates_a11y_switches_modes_and_opens_entry_editor() {
        let text = TextSystem::new().expect("dwrite");
        let palette = Palette::LIGHT;
        let env = ScreenEnv {
            width_dip: 1200.0,
            height_dip: 800.0,
            hover: None,
            pressed: None,
            focused: None,
            palette: &palette,
            text: &text,
        };
        let store = shared_store(Box::new(DemoStore::new(SystemLocale::en_us())));
        let screen = CalendarScreen::new(Rc::clone(&store));
        let mut focus = FocusModel::new();
        let mut host = ScreenHost::mount(Box::new(screen), &env, &mut focus);
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));
        assert!(host.wants_tick());

        // Switch Mix -> Time -> Activity -> Mix.
        for label in [
            strings::CAL_MODE_TIME,
            strings::CAL_MODE_ACTIVITY,
            strings::CAL_MODE_MIX,
        ] {
            let btn = host
                .tree()
                .all_nodes()
                .into_iter()
                .filter_map(|n| host.tree().data(n))
                .find(|d| d.a11y.name == label)
                .unwrap()
                .id;
            assert!(host.on_click(btn));
            assert!(host.rebuild_if_dirty(&env, &mut focus));
            assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));
        }

        // Navigate grid by keyboard & press Enter -> opens EntryEditor modal!
        let grid_id = host
            .tree()
            .all_nodes()
            .into_iter()
            .filter_map(|n| host.tree().data(n))
            .find(|d| d.a11y.name == strings::CAL_GRID_LABEL)
            .unwrap()
            .id;
        assert!(focus.focus_keyboard(grid_id));
        let env_focused = ScreenEnv {
            focused: focus.focused(),
            ..env
        };
        host.mark_dirty();
        assert!(host.rebuild_if_dirty(&env_focused, &mut focus));
        let (handled, dirty2) = host.on_key(
            crate::ui::input::KeyEvent {
                key: KeyCode::Enter,
                shift: false,
                ctrl: false,
                alt: false,
            },
            &mut focus,
        );
        assert!(handled && dirty2);
        assert!(host.rebuild_if_dirty(&env_focused, &mut focus));
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));
        assert!(matches!(focus.active_scope(), FocusScope::Modal(_)));
    }
}
