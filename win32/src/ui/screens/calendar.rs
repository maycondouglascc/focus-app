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
use crate::time::{add_days, add_days_start, iso_week, start_of_week_with_first};
use crate::ui::input::KeyEvent;
use crate::ui::screen::{BuiltScreen, KeyModifiers, PointerReply, Screen, ScreenEnv, TickReply};
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
    last_full_refresh_ms: i64,
    pub new_project_requested: bool,
    #[cfg(test)]
    pub last_opened_activity: Option<super::calendar_layout::ActivitySession>,
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
            last_full_refresh_ms: now_ms,
            new_project_requested: false,
            #[cfg(test)]
            last_opened_activity: None,
        }
    }

    pub fn has_modal(&self) -> bool {
        self.editor.is_some()
    }

    pub fn reset_transient(&mut self) {
        self.editor = None;
        self.act_log_slot = None;
        self.new_project_requested = false;
        let _ = self.grid.cancel_drag();
    }

    pub fn open_new_entry_at_now(&mut self) {
        let now_ms = self.store.borrow().now_ms();
        let rounded_min = ((crate::time::minutes_of_day(now_ms) / 15.0).floor() as i32) * 15;
        let day = crate::time::start_of_day(now_ms);
        let start = crate::time::at_minutes(day, rounded_min);
        self.anchor_ms = now_ms;
        self.editor = Some(EntryEditorState::new(EntryDraft {
            id: None,
            project_id: None,
            description: String::new(),
            start,
            end: Some(start + 30 * crate::time::MIN),
            billable: false,
            source: "manual".into(),
        }));
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
                #[cfg(test)]
                {
                    self.last_opened_activity = Some(session.clone());
                }
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
            let week_end = add_days_start(week_start, 7);
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
        self.last_full_refresh_ms = now_ms;
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
                min_size: Size {
                    width: length(0.0_f32),
                    height: length(0.0_f32),
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
        let (pnode, _) = button::build(tree, nav_left, &prev_p, p);
        self.id_prev_week = Some(tree.set_key(pnode, "cal.prev"));

        let cur_week_start = start_of_week_with_first(now_ms, first_day_mon0);
        let this_week = week_start == cur_week_start;
        let mut today_p = ButtonProps::new(strings::CAL_DAY_TODAY);
        today_p.size = ButtonSize::Small;
        today_p.disabled = this_week;
        today_p.interact = Interact::from_state(self.id_today, env.hover, env.pressed, this_week);
        let (tnode, _) = button::build(tree, nav_left, &today_p, p);
        self.id_today = Some(tree.set_key(tnode, "cal.today"));

        let mut next_p = ButtonProps::new("");
        next_p.aria_label = Some(strings::CAL_NEXT_WEEK);
        next_p.icon = Some(glyph::CHEVRON_RIGHT);
        next_p.variant = ButtonVariant::Ghost;
        next_p.size = ButtonSize::Small;
        next_p.interact = Interact::from_state(self.id_next_week, env.hover, env.pressed, false);
        let (nnode, _) = button::build(tree, nav_left, &next_p, p);
        self.id_next_week = Some(tree.set_key(nnode, "cal.next"));

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
        let (_, mids) = segmented::build_keyed(
            tree,
            nav_right,
            &SegmentedProps {
                group_name: "Calendar shows",
                options: &mode_opts,
                selected: self.grid.mode.index(),
                hover: env.hover,
                disabled: false,
            },
            Some("cal.mode"),
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
                np.interact =
                    Interact::from_state(self.id_empty_new_proj, env.hover, env.pressed, false);
                let (bnode, _) = button::build(tree, hint_bar, &np, p);
                self.id_empty_new_proj = Some(tree.set_key(bnode, "cal.empty_new_project"));
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
            self.new_project_requested = true;
            return true;
        }
        if self.grid.on_click(id) {
            if let Some(action) = self.grid.pending_action.take() {
                self.apply_grid_action(action, palette);
            }
            return true;
        }
        false
    }

    pub fn on_a11y_focus(&mut self, id: WidgetId) -> bool {
        if self.editor.is_some() {
            return false;
        }
        self.grid.on_a11y_focus(id)
    }

    pub fn handle_key(&mut self, ev: KeyEvent) -> bool {
        if self.editor.is_some() {
            return false;
        }
        if self.grid.on_key(ev) {
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

    pub fn wants_tick(&self) -> Option<u32> {
        if self.store.borrow().is_demo() {
            None
        } else if self.timer.is_running_cached() {
            Some(1000)
        } else {
            Some(30_000)
        }
    }

    pub fn handle_tick(&mut self, tree: &mut Tree) -> TickReply {
        if self.store.borrow().is_demo() {
            return TickReply::default();
        }
        let now_ms = self.store.borrow().now_ms();
        let elapsed = now_ms - self.last_full_refresh_ms;
        let running = self.timer.is_running_cached();
        let refresh_due = if running {
            !(0..30_000).contains(&elapsed)
        } else {
            !(0..25_000).contains(&elapsed)
        };
        if refresh_due {
            return TickReply {
                repaint: true,
                rebuild: true,
            };
        }
        if running && self.timer.update_clock_in_tree(tree, now_ms) {
            return TickReply {
                repaint: true,
                rebuild: false,
            };
        }
        TickReply::default()
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

    fn on_a11y_focus(&mut self, id: WidgetId) -> bool {
        CalendarScreen::on_a11y_focus(self, id)
    }

    fn on_key(&mut self, ev: KeyEvent, focus: &mut crate::ui::focus::FocusModel) -> bool {
        self.grid.set_canvas_focused(
            focus.focused().is_some() && focus.focused() == self.grid.canvas_id,
        );
        self.handle_key(ev)
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

    fn wants_tick(&self) -> Option<u32> {
        self.wants_tick()
    }

    fn on_tick(&mut self, tree: &mut Tree) -> TickReply {
        self.handle_tick(tree)
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
    use crate::ui::input::KeyCode;
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
        assert_eq!(host.wants_tick(), None);

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

    #[test]
    fn focus_stays_on_same_widget_when_conditional_nodes_appear_or_disappear() {
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
        // Start with an empty store (no entries -> hint_bar visible, no project dot).
        let store = shared_store(Box::new(DemoStore::fresh(SystemLocale::en_us())));
        let _proj = store
            .borrow_mut()
            .create_project(crate::db::ProjectInput {
                name: "Alpha".into(),
                client: String::new(),
                color: "#2266dd".into(),
                rate: 100.0,
                currency: "USD".into(),
                billable_default: true,
                archived: false,
            })
            .unwrap();

        let screen = CalendarScreen::new(Rc::clone(&store));
        let mut focus = FocusModel::new();
        let mut host = ScreenHost::mount(Box::new(screen), &env, &mut focus);

        // 1. Focus "Time" mode button while tracked/earned labels are present, then switch to Activity
        //    mode (which removes tracked/earned labels before the segmented control).
        let mode_time_id = host
            .tree()
            .all_nodes()
            .into_iter()
            .filter_map(|n| host.tree().data(n))
            .find(|d| d.a11y.name == strings::CAL_MODE_TIME)
            .unwrap()
            .id;
        assert!(focus.focus_keyboard(mode_time_id));
        let mode_act_id = host
            .tree()
            .all_nodes()
            .into_iter()
            .filter_map(|n| host.tree().data(n))
            .find(|d| d.a11y.name == strings::CAL_MODE_ACTIVITY)
            .unwrap()
            .id;
        assert!(host.on_click(mode_act_id));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        let focused_after_mode = focus.focused().expect("focus should stay on Time button");
        let mode_name = host
            .tree()
            .node_by_id(focused_after_mode)
            .and_then(|n| host.tree().data(n))
            .map(|d| d.a11y.name.as_str());
        assert_eq!(mode_name, Some(strings::CAL_MODE_TIME));

        // 2. Focus WeekGrid while empty hint_bar is absent (Activity mode), then switch back to Mix
        //    (hint_bar appears before WeekGrid!). Focus must stay on WeekGrid.
        let grid_id = host
            .tree()
            .all_nodes()
            .into_iter()
            .filter_map(|n| host.tree().data(n))
            .find(|d| d.a11y.name == strings::CAL_GRID_LABEL)
            .unwrap()
            .id;
        assert!(focus.focus_keyboard(grid_id));
        let mode_mix_id = host
            .tree()
            .all_nodes()
            .into_iter()
            .filter_map(|n| host.tree().data(n))
            .find(|d| d.a11y.name == strings::CAL_MODE_MIX)
            .unwrap()
            .id;
        assert!(host.on_click(mode_mix_id));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        let focused_after_hint = focus.focused().expect("focus should stay on WeekGrid");
        let grid_name = host
            .tree()
            .node_by_id(focused_after_hint)
            .and_then(|n| host.tree().data(n))
            .map(|d| d.a11y.name.as_str());
        assert_eq!(grid_name, Some(strings::CAL_GRID_LABEL));
    }

    #[test]
    fn week_grid_exposes_virtual_a11y_children_for_demo_entries_sessions_and_cursor_and_opens_popover_by_keyboard(
    ) {
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
        let now_ms = store.borrow().now_ms();
        let week_start = start_of_week_with_first(now_ms, 0);
        let week_end = add_days_start(week_start, 7);
        let week_entries = store.borrow().list_entries(week_start, week_end).unwrap();
        assert!(!week_entries.is_empty());

        let screen = CalendarScreen::new(Rc::clone(&store));
        let mut focus = FocusModel::new();
        let mut host = ScreenHost::mount(Box::new(screen), &env, &mut focus);
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));

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

        let update = a11y::build_tree_update(
            Some(host.tree()),
            focus.focused(),
            1.0,
            (1200, 800),
            strings::APP_NAME,
        );
        // Must expose one virtual child button per visible entry segment in the demo week.
        let button_names: Vec<String> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == accesskit::Role::Button)
            .filter_map(|(_, n)| n.label().map(str::to_string))
            .collect();
        for e in &week_entries {
            let desc_part = if !e.description.is_empty() {
                e.description.as_str()
            } else {
                ""
            };
            assert!(
                button_names
                    .iter()
                    .any(|lbl| desc_part.is_empty() || lbl.contains(desc_part)),
                "missing virtual a11y node for demo entry {:?} in {:?}",
                e.description,
                button_names
            );
        }
        // Focus in UIA must target the virtual item under the cursor, not just the grid group.
        assert_ne!(update.focus, a11y::ak_id(grid_id));

        // Must also expose virtual nodes for demo activity sessions in Mix/Activity mode.
        let activity_nodes: Vec<(accesskit::NodeId, String)> = update
            .nodes
            .iter()
            .filter_map(|(nid, n)| {
                let lbl = n.label()?;
                if lbl.starts_with("Activity: ") {
                    Some((*nid, lbl.to_string()))
                } else {
                    None
                }
            })
            .collect();
        assert!(
            !activity_nodes.is_empty(),
            "expected virtual a11y buttons for demo activity sessions"
        );

        // Moving UIA focus onto a virtual activity node syncs WeekGrid cursor and UIA focus.
        let target_sess_ak = activity_nodes[0].0;
        let target_sess_wid = a11y::widget_id(target_sess_ak);
        assert!(host.on_a11y_focus(target_sess_wid));
        assert!(host.rebuild_if_dirty(&env_focused, &mut focus));
        let update2 = a11y::build_tree_update(
            Some(host.tree()),
            focus.focused(),
            1.0,
            (1200, 800),
            strings::APP_NAME,
        );
        assert_eq!(update2.focus, target_sess_ak);

        // Activating 'A' or switching to Activity mode + Enter opens the activity session by keyboard.
        let mut grid = WeekGridState::new();
        let entries_all = store.borrow().list_entries(week_start, week_end).unwrap();
        let activity_all = store.borrow().list_activity(week_start, week_end).unwrap();
        let projs_map: HashMap<String, Project> = store
            .borrow()
            .list_projects()
            .unwrap()
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();
        let mut t = Tree::new(style::column());
        let r = t.root();
        grid.mode = CalendarMode::Activity;
        grid.build(
            &mut t,
            r,
            week_start,
            &entries_all,
            &activity_all,
            &projs_map,
            now_ms,
            8,
            true,
            &env_focused,
        );
        grid.set_canvas_focused(true);
        // Find any virtual session and move focus to its slot, then press Enter -> OpenActivity!
        let sess_vid = t
            .data(t.node_by_id(grid.canvas_id.unwrap()).unwrap())
            .unwrap()
            .virtual_children
            .iter()
            .find(|c| c.a11y.name.starts_with("Activity: "))
            .unwrap()
            .id;
        assert!(grid.on_a11y_focus(sess_vid));
        let enter = KeyEvent {
            key: KeyCode::Enter,
            shift: false,
            ctrl: false,
            alt: false,
        };
        assert!(grid.on_key(enter));
        match grid.pending_action.take() {
            Some(GridAction::OpenActivity(sess, _)) => assert!(sess.active_ms > 0),
            other => panic!("expected GridAction::OpenActivity in Activity mode, got {other:?}"),
        }

        // Also test pressing 'A' in Mix mode to open ActivityPopover on the focused session slot.
        let mut grid_mix = WeekGridState::new();
        grid_mix.mode = CalendarMode::Mix;
        let mut t2 = Tree::new(style::column());
        let r2 = t2.root();
        grid_mix.build(
            &mut t2,
            r2,
            week_start,
            &entries_all,
            &activity_all,
            &projs_map,
            now_ms,
            8,
            true,
            &env_focused,
        );
        let sess_slot_vid = t2
            .data(t2.node_by_id(grid_mix.canvas_id.unwrap()).unwrap())
            .unwrap()
            .virtual_children
            .iter()
            .find(|c| c.a11y.name.starts_with("Activity: "))
            .unwrap()
            .id;
        assert!(grid_mix.on_a11y_focus(sess_slot_vid));
        grid_mix.set_canvas_focused(true);
        let key_a = KeyEvent {
            key: KeyCode::Other(0x41),
            shift: false,
            ctrl: false,
            alt: false,
        };
        assert!(grid_mix.on_key(key_a));
        assert!(matches!(
            grid_mix.pending_action.take(),
            Some(GridAction::OpenActivity(_, _))
        ));

        // Also test direct CalendarScreen::handle_click on virtual session -> records OpenActivity.
        let mut cal = CalendarScreen::new(Rc::clone(&store));
        let mut cal_tree = screen_root_tree(&env_focused);
        let cal_root = cal_tree.root();
        cal.build_content(&mut cal_tree, cal_root, cal_root, &env_focused);
        cal_tree.layout(1200.0, 800.0, &text);
        assert!(cal.handle_click(target_sess_wid, &palette));
        assert!(cal.last_opened_activity.is_some());

        // And host handling virtual click stays valid.
        assert!(host.on_click(target_sess_wid));
    }

    #[test]
    fn calendar_tick_updates_clock_lightweight_at_1s_refreshes_at_30s_and_stays_silent_in_demo() {
        use std::cell::Cell;
        use std::collections::HashMap as Map;

        struct TimedDbStore {
            inner: crate::db::DbStore,
            now: Rc<Cell<i64>>,
            entry_queries: Rc<Cell<u32>>,
        }
        impl crate::db::Store for TimedDbStore {
            fn now_ms(&self) -> i64 {
                self.now.get()
            }
            fn is_demo(&self) -> bool {
                false
            }
            fn system_locale(&self) -> &SystemLocale {
                self.inner.system_locale()
            }
            fn list_projects(&self) -> Result<Vec<Project>, String> {
                self.inner.list_projects()
            }
            fn get_project(&self, id: &str) -> Result<Project, String> {
                self.inner.get_project(id)
            }
            fn create_project(
                &mut self,
                input: crate::db::ProjectInput,
            ) -> Result<Project, String> {
                self.inner.create_project(input)
            }
            fn update_project(
                &mut self,
                id: &str,
                patch: crate::db::ProjectPatch,
            ) -> Result<Project, String> {
                self.inner.update_project(id, patch)
            }
            fn delete_project(&mut self, id: &str) -> Result<(), String> {
                self.inner.delete_project(id)
            }
            fn project_summaries(
                &self,
                mf: i64,
                mt: i64,
            ) -> Result<Map<String, crate::db::ProjectSummary>, String> {
                self.inner.project_summaries(mf, mt)
            }
            fn project_all_time_stats(
                &self,
                id: &str,
            ) -> Result<crate::db::ProjectAllTimeStats, String> {
                self.inner.project_all_time_stats(id)
            }
            fn list_entries(&self, from: i64, to: i64) -> Result<Vec<TimeEntry>, String> {
                self.entry_queries.set(self.entry_queries.get() + 1);
                self.inner.list_entries(from, to)
            }
            fn get_entry(&self, id: &str) -> Result<TimeEntry, String> {
                self.inner.get_entry(id)
            }
            fn has_any_entries(&self) -> Result<bool, String> {
                self.inner.has_any_entries()
            }
            fn earliest_entry_start(
                &self,
                pid: Option<&str>,
                np: bool,
            ) -> Result<Option<i64>, String> {
                self.inner.earliest_entry_start(pid, np)
            }
            fn create_entry(&mut self, input: crate::db::EntryInput) -> Result<TimeEntry, String> {
                self.inner.create_entry(input)
            }
            fn update_entry(
                &mut self,
                id: &str,
                patch: crate::db::EntryPatch,
            ) -> Result<TimeEntry, String> {
                self.inner.update_entry(id, patch)
            }
            fn delete_entry(&mut self, id: &str) -> Result<(), String> {
                self.inner.delete_entry(id)
            }
            fn running_entry(&self) -> Result<Option<TimeEntry>, String> {
                self.inner.running_entry()
            }
            fn start_timer(
                &mut self,
                input: crate::db::TimerStartInput,
            ) -> Result<TimeEntry, String> {
                self.inner.create_entry(crate::db::EntryInput {
                    project_id: input.project_id,
                    description: input.description,
                    start: self.now.get(),
                    end: None,
                    billable: input.billable,
                    source: "timer".into(),
                })
            }
            fn stop_timer(&mut self) -> Result<Option<TimeEntry>, String> {
                if let Some(cur) = self.inner.running_entry()? {
                    return self
                        .inner
                        .update_entry(
                            &cur.id,
                            crate::db::EntryPatch {
                                end: Some(Some(self.now.get().max(cur.start + 1))),
                                ..Default::default()
                            },
                        )
                        .map(Some);
                }
                Ok(None)
            }
            fn list_activity(&self, f: i64, t: i64) -> Result<Vec<crate::db::Activity>, String> {
                self.inner.list_activity(f, t)
            }
            fn visit_activity(
                &self,
                f: i64,
                t: i64,
                func: &mut dyn FnMut(crate::db::Activity),
            ) -> Result<(), String> {
                self.inner.visit_activity(f, t, func)
            }
            fn active_union_ms(&self, f: i64, t: i64) -> Result<i64, String> {
                self.inner.active_union_ms(f, t)
            }
            fn earliest_activity_start(&self) -> Result<Option<i64>, String> {
                self.inner.earliest_activity_start()
            }
            fn clear_activity(&mut self) -> Result<(), String> {
                self.inner.clear_activity()
            }
            fn get_settings(&self) -> Result<crate::db::Settings, String> {
                self.inner.get_settings()
            }
            fn save_settings(
                &mut self,
                p: crate::db::SettingsPatch,
            ) -> Result<crate::db::Settings, String> {
                self.inner.save_settings(p)
            }
            fn monitor_status(&self) -> crate::db::MonitorStatus {
                self.inner.monitor_status()
            }
        }

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

        // 1. Demo mode must request no timer ticks at all.
        let mut demo_store = DemoStore::new(SystemLocale::en_us());
        let _ = crate::db::Store::start_timer(
            &mut demo_store,
            crate::db::TimerStartInput {
                project_id: None,
                description: "Demo task".into(),
                billable: false,
            },
        );
        let demo_screen = CalendarScreen::new(shared_store(Box::new(demo_store)));
        assert_eq!(demo_screen.wants_tick(), None);

        // 2. Non-demo DB with NO timer running asks for a 30s tick (refreshes now/today).
        let base = crate::time::from_local(2026, 11, 18, 10, 0, 0, 0);
        let now = Rc::new(Cell::new(base));
        let entry_queries = Rc::new(Cell::new(0));
        let db = crate::db::Db::open_in_memory().unwrap();
        let store = shared_store(Box::new(TimedDbStore {
            inner: crate::db::DbStore::new(db, SystemLocale::en_us()),
            now: Rc::clone(&now),
            entry_queries: Rc::clone(&entry_queries),
        }));

        let screen = CalendarScreen::new(Rc::clone(&store));
        let mut focus = FocusModel::new();
        let mut host = ScreenHost::mount(Box::new(screen), &env, &mut focus);
        assert_eq!(host.wants_tick(), Some(30_000));
        assert_eq!(entry_queries.get(), 1);

        now.set(base + 30_000);
        assert_eq!(
            host.on_tick(),
            TickReply {
                repaint: true,
                rebuild: true
            }
        );
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(entry_queries.get(), 2);

        // 3. Starting a timer switches tick to 1000ms; 1s ticks update only clock text (0 SQL queries, no rebuild).
        store
            .borrow_mut()
            .start_timer(crate::db::TimerStartInput {
                project_id: None,
                description: "Live".into(),
                billable: false,
            })
            .unwrap();
        host.mark_dirty();
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(host.wants_tick(), Some(1000));
        let queries_after_start = entry_queries.get();

        now.set(base + 35_000); // +5s since timer started at base+30s
        let r1 = host.on_tick();
        assert_eq!(
            r1,
            TickReply {
                repaint: true,
                rebuild: false
            }
        );
        assert_eq!(entry_queries.get(), queries_after_start);
        assert!(!host.rebuild_if_dirty(&env, &mut focus));

        let clock_id = crate::ui::tree::keyed_widget_id("timer.clock");
        let clock_text = host
            .tree()
            .node_by_id(clock_id)
            .and_then(|n| host.tree().data(n))
            .and_then(|d| match &d.leaf {
                Some(crate::ui::tree::Leaf::StyledText { text, .. }) => Some(text.as_str()),
                _ => None,
            });
        assert_eq!(clock_text, Some("0:00:05"));

        // 4. At +30s with running timer, tick requests full rebuild to move the "now" line and live block.
        now.set(base + 60_000);
        assert_eq!(
            host.on_tick(),
            TickReply {
                repaint: true,
                rebuild: true
            }
        );
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(entry_queries.get(), queries_after_start + 1);
    }
}
