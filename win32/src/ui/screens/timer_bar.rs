//! Top stopwatch TimerBar: describe, pick project, toggle billable, start/stop.
//! Mirrors any external timer changes cleanly (e.g. Ctrl+Space or stop-all-timers).

use taffy::prelude::*;

use crate::db::{EntryPatch, Project, SharedStore, TimerStartInput};
use crate::format;
use crate::ui::screen::ScreenEnv;
use crate::ui::strings;
use crate::ui::text::{Align, Weight, Wrap};
use crate::ui::theme::parse_hex_color;
use crate::ui::tree::{style, Leaf, Node, PaintStyle, Tree, WidgetId};
use crate::ui::widgets::button::{self, ButtonProps, ButtonSize, ButtonVariant};
use crate::ui::widgets::select::{self, SelectProps};
use crate::ui::widgets::text_field::{self, TextFieldProps};
use crate::ui::widgets::{glyph, Interact};

use super::PopupSelectSlot;

const KEY_TIMER_DESC: &str = "timer.description";

pub struct TimerBarState {
    description: String,
    project_id: Option<String>,
    billable: bool,
    last_running_id: Option<String>,
    last_running_start: Option<i64>,
    desc_sync_rev: u64,

    clock_node: Option<Node>,
    id_desc_slot: Option<WidgetId>,
    sel_project: PopupSelectSlot,
    id_bill_btn: Option<WidgetId>,
    id_toggle_btn: Option<WidgetId>,
}

impl Default for TimerBarState {
    fn default() -> Self {
        Self::new()
    }
}

impl TimerBarState {
    pub fn new() -> Self {
        Self {
            description: String::new(),
            project_id: None,
            billable: false,
            last_running_id: None,
            last_running_start: None,
            desc_sync_rev: 0,
            clock_node: None,
            id_desc_slot: None,
            sel_project: PopupSelectSlot::default(),
            id_bill_btn: None,
            id_toggle_btn: None,
        }
    }

    pub fn sync_from_store(&mut self, store: &SharedStore) {
        let running = store.borrow().running_entry().unwrap_or(None);
        match running {
            Some(e) => {
                self.last_running_start = Some(e.start);
                if self.last_running_id.as_deref() != Some(e.id.as_str()) {
                    self.last_running_id = Some(e.id);
                    self.description = e.description;
                    self.project_id = e.project_id;
                    self.billable = e.billable;
                    self.desc_sync_rev += 1;
                }
            }
            None => {
                self.last_running_start = None;
                if self.last_running_id.is_some() {
                    self.last_running_id = None;
                    self.description.clear();
                    self.project_id = None;
                    self.billable = false;
                    self.desc_sync_rev += 1;
                }
            }
        }
    }

    pub fn is_running_cached(&self) -> bool {
        self.last_running_id.is_some()
    }

    /// Updates only the elapsed clock label inside the existing layout tree (no SQL or Taffy rebuild).
    pub fn update_clock_in_tree(&self, tree: &mut Tree, now_ms: i64) -> bool {
        let (Some(node), Some(start)) = (self.clock_node, self.last_running_start) else {
            return false;
        };
        let text = format::clock((now_ms - start).max(0));
        tree.replace_styled_text(node, text);
        true
    }

    #[cfg(test)]
    pub fn is_running(&self, store: &SharedStore) -> bool {
        store.borrow().running_entry().ok().flatten().is_some()
    }

    pub fn toggle_start_stop(&mut self, store: &SharedStore) -> bool {
        let running = store.borrow().running_entry().ok().flatten();
        if let Some(cur) = running {
            let mut st = store.borrow_mut();
            if self.description.trim() != cur.description.trim() {
                let _ = st.update_entry(
                    &cur.id,
                    EntryPatch {
                        description: Some(self.description.clone()),
                        ..Default::default()
                    },
                );
            }
            let _ = st.stop_timer();
            drop(st);
            self.sync_from_store(store);
        } else {
            let input = TimerStartInput {
                project_id: self.project_id.clone(),
                description: self.description.clone(),
                billable: self.billable,
            };
            let _ = store.borrow_mut().start_timer(input);
            self.sync_from_store(store);
        }
        true
    }

    fn project_menu(projects: &[Project], current: Option<&str>) -> Vec<Option<Project>> {
        let mut out = Vec::with_capacity(projects.len() + 1);
        out.push(None);
        for p in projects {
            if !p.archived || current == Some(p.id.as_str()) {
                out.push(Some(p.clone()));
            }
        }
        out
    }

    pub fn build(
        &mut self,
        tree: &mut Tree,
        parent: Node,
        store: &SharedStore,
        projects: &[Project],
        env: &ScreenEnv<'_>,
    ) {
        self.sync_from_store(store);
        let p = env.palette;
        let running = store.borrow().running_entry().ok().flatten();
        let now_ms = store.borrow().now_ms();

        let bar = tree.add_container(
            parent,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                padding: style::pad(12.0, 8.0, 12.0, 8.0),
                gap: style::gap(10.0),
                flex_shrink: 0.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: length(50.0_f32),
                },
                ..Default::default()
            },
        );
        tree.set_paint(
            bar,
            PaintStyle {
                bg: Some(p.surface),
                border_color: Some(if running.is_some() {
                    p.accent
                } else {
                    p.card_border
                }),
                border_width: p.border_width,
                radius: 10.0,
                clip: false,
            },
        );

        let desc_wrap = tree.add_container(
            bar,
            Style {
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                ..Default::default()
            },
        );
        let focused_desc = env.focused.is_some() && env.focused == self.id_desc_slot;
        let (_c, desc_id) = text_field::build_tree_node(
            tree,
            desc_wrap,
            &TextFieldProps {
                key: KEY_TIMER_DESC,
                aria_label: strings::TIMER_PLACEHOLDER,
                placeholder: strings::TIMER_PLACEHOLDER,
                value: &self.description,
                multiline: false,
                min_height: 32.0,
                focused: focused_desc,
                error: false,
                disabled: false,
                sync_rev: self.desc_sync_rev,
            },
            p,
        );
        self.id_desc_slot = Some(desc_id);

        // Project dropdown
        let menu = Self::project_menu(projects, self.project_id.as_deref());
        let labels: Vec<String> = menu
            .iter()
            .map(|opt| match opt {
                None => strings::TIMER_NO_PROJECT.to_string(),
                Some(pr) => pr.name.clone(),
            })
            .collect();
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let sel_idx = menu
            .iter()
            .position(|opt| opt.as_ref().map(|x| x.id.as_str()) == self.project_id.as_deref())
            .or(Some(0));

        let proj_box = tree.add_container(
            bar,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                gap: style::gap(6.0),
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        if let Some(cur_pid) = self.project_id.as_deref() {
            if let Some(proj) = projects.iter().find(|pr| pr.id == cur_pid) {
                let dot_c = parse_hex_color(&proj.color).unwrap_or(p.accent);
                let dot = tree.add_container(
                    proj_box,
                    Style {
                        size: style::size(8.0, 8.0),
                        flex_shrink: 0.0,
                        ..Default::default()
                    },
                );
                tree.set_paint(
                    dot,
                    PaintStyle {
                        bg: Some(dot_c),
                        border_color: None,
                        border_width: 0.0,
                        radius: 4.0,
                        clip: false,
                    },
                );
            }
        }
        let sel_int =
            Interact::from_state(self.sel_project.trigger_id, env.hover, env.pressed, false);
        let (proj_node, _) = select::build(
            tree,
            proj_box,
            &SelectProps {
                aria_label: strings::ENTRY_FIELD_PROJECT,
                options: &label_refs,
                selected: sel_idx,
                placeholder: strings::TIMER_NO_PROJECT,
                min_width: 148.0,
                open: self.sel_project.is_open(),
                disabled: false,
                interact: sel_int,
            },
            p,
        );
        self.sel_project.trigger_id = Some(tree.set_key(proj_node, "timer.project"));

        // Billable toggle button ($)
        let mut bill_props = ButtonProps::new("$");
        bill_props.aria_label = Some(strings::TIMER_BILLABLE);
        bill_props.variant = if self.billable {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Ghost
        };
        bill_props.size = ButtonSize::Small;
        bill_props.interact = Interact::from_state(self.id_bill_btn, env.hover, env.pressed, false);
        let (bill_node, _) = button::build(tree, bar, &bill_props, p);
        let bill_id = tree.set_key(bill_node, "timer.billable");
        if let Some(d) = tree.data_mut(bill_node) {
            d.a11y.toggled = Some(self.billable);
        }
        self.id_bill_btn = Some(bill_id);

        // Running clock readout
        let clock_text = match &running {
            Some(r) => format::clock((now_ms - r.start).max(0)),
            None => "0:00:00".into(),
        };
        let clock_box = tree.add_container(
            bar,
            Style {
                size: Size {
                    width: length(64.0_f32),
                    height: auto(),
                },
                justify_content: Some(JustifyContent::FlexEnd),
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        let clock_node = tree.add_leaf(
            clock_box,
            Style {
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
            Leaf::StyledText {
                text: clock_text,
                size: 13.5,
                weight: Weight::SemiBold,
                align: Align::Right,
                wrap: Wrap::NoWrap,
                color: if running.is_some() { p.accent } else { p.muted },
            },
        );
        let _ = tree.set_key(clock_node, "timer.clock");
        self.clock_node = Some(clock_node);

        // Start / Stop primary action button
        let is_run = running.is_some();
        let mut act_props = ButtonProps::new("");
        act_props.aria_label = Some(if is_run {
            strings::TIMER_STOP
        } else {
            strings::TIMER_START
        });
        act_props.icon = Some(if is_run { glyph::STOP } else { glyph::PLAY });
        act_props.variant = if is_run {
            ButtonVariant::Danger
        } else {
            ButtonVariant::Primary
        };
        act_props.size = ButtonSize::Icon;
        act_props.interact =
            Interact::from_state(self.id_toggle_btn, env.hover, env.pressed, false);
        let (act_node, _) = button::build(tree, bar, &act_props, p);
        self.id_toggle_btn = Some(tree.set_key(act_node, "timer.toggle"));
    }

    pub fn on_click(
        &mut self,
        id: WidgetId,
        store: &SharedStore,
        projects: &[Project],
        palette: &crate::ui::theme::Palette,
    ) -> bool {
        if Some(id) == self.id_toggle_btn {
            return self.toggle_start_stop(store);
        }
        if Some(id) == self.id_bill_btn {
            self.billable = !self.billable;
            let cur = store.borrow().running_entry().ok().flatten();
            if let Some(cur) = cur {
                let _ = store.borrow_mut().update_entry(
                    &cur.id,
                    EntryPatch {
                        billable: Some(self.billable),
                        ..Default::default()
                    },
                );
            }
            return true;
        }
        if Some(id) == self.sel_project.trigger_id {
            let menu = Self::project_menu(projects, self.project_id.as_deref());
            let labels: Vec<String> = menu
                .iter()
                .map(|opt| match opt {
                    None => strings::TIMER_NO_PROJECT.to_string(),
                    Some(pr) => pr.name.clone(),
                })
                .collect();
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let sel = menu
                .iter()
                .position(|opt| opt.as_ref().map(|x| x.id.as_str()) == self.project_id.as_deref());
            self.sel_project.open(&refs, sel, palette);
            return true;
        }
        false
    }

    pub fn on_text_change(&mut self, key: &str, value: &str, store: &SharedStore) -> bool {
        if key == KEY_TIMER_DESC {
            self.description = value.to_string();
            let cur = store.borrow().running_entry().ok().flatten();
            if let Some(cur) = cur {
                let _ = store.borrow_mut().update_entry(
                    &cur.id,
                    EntryPatch {
                        description: Some(self.description.clone()),
                        ..Default::default()
                    },
                );
            }
            return false;
        }
        false
    }

    pub fn on_text_submit(&mut self, key: &str, value: &str, store: &SharedStore) -> bool {
        if key == KEY_TIMER_DESC {
            self.description = value.to_string();
            let cur = store.borrow().running_entry().ok().flatten();
            if let Some(cur) = cur {
                let _ = store.borrow_mut().update_entry(
                    &cur.id,
                    EntryPatch {
                        description: Some(self.description.clone()),
                        ..Default::default()
                    },
                );
                return true;
            } else {
                return self.toggle_start_stop(store);
            }
        }
        false
    }

    pub fn poll_popup(&mut self, store: &SharedStore, projects: &[Project]) -> bool {
        if let Some(res) = self.sel_project.poll() {
            if let Some(idx) = res {
                let menu = Self::project_menu(projects, self.project_id.as_deref());
                if let Some(choice) = menu.get(idx) {
                    self.project_id = choice.as_ref().map(|pr| pr.id.clone());
                    if let Some(pr) = choice {
                        self.billable = pr.billable_default;
                    }
                    let cur = store.borrow().running_entry().ok().flatten();
                    if let Some(cur) = cur {
                        let _ = store.borrow_mut().update_entry(
                            &cur.id,
                            EntryPatch {
                                project_id: Some(self.project_id.clone()),
                                billable: Some(self.billable),
                                ..Default::default()
                            },
                        );
                    }
                }
            }
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::shared_store;
    use crate::demo::DemoStore;
    use crate::locale::SystemLocale;
    use crate::ui::a11y;
    use crate::ui::text::TextSystem;
    use crate::ui::theme::Palette;

    #[test]
    fn timer_bar_builds_a11y_mirrors_running_entry_and_toggles_timer() {
        let text = TextSystem::new().expect("dwrite");
        let palette = Palette::LIGHT;
        let env = ScreenEnv {
            width_dip: 1000.0,
            height_dip: 200.0,
            hover: None,
            pressed: None,
            focused: None,
            palette: &palette,
            text: &text,
        };
        let store = shared_store(Box::new(DemoStore::new(SystemLocale::en_us())));
        let projects = store.borrow().list_projects().unwrap();
        let mut bar = TimerBarState::new();
        let mut tree = Tree::new(style::column());
        let root = tree.root();
        bar.build(&mut tree, root, &store, &projects, &env);
        tree.layout(1000.0, 200.0, &text);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));
        assert!(bar.is_running(&store));

        // Toggle off -> stops running timer.
        let btn_id = bar.id_toggle_btn.unwrap();
        assert!(bar.on_click(btn_id, &store, &projects, &palette));
        assert!(!bar.is_running(&store));

        // Submit text starts new timer cleanly.
        assert!(bar.on_text_submit(KEY_TIMER_DESC, "Review PR", &store));
        assert!(bar.is_running(&store));
        assert_eq!(
            store.borrow().running_entry().unwrap().unwrap().description,
            "Review PR"
        );

        // Project dot appearing/disappearing must not shift Project/Billable/Toggle widget IDs.
        let mut t1 = Tree::new(style::column());
        let r1 = t1.root();
        bar.project_id = None;
        bar.build(&mut t1, r1, &store, &projects, &env);
        let ids_without_dot = (
            bar.sel_project.trigger_id,
            bar.id_bill_btn,
            bar.id_toggle_btn,
        );

        let mut t2 = Tree::new(style::column());
        let r2 = t2.root();
        bar.project_id = Some(projects[0].id.clone());
        bar.build(&mut t2, r2, &store, &projects, &env);
        let ids_with_dot = (
            bar.sel_project.trigger_id,
            bar.id_bill_btn,
            bar.id_toggle_btn,
        );
        assert_eq!(ids_without_dot, ids_with_dot);
    }
}
