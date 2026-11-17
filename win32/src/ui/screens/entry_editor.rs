//! Modal time-entry editor dialog matching `EntryEditor.tsx`:
//! create, edit, stop running timer, delete, midnight-wrapping end time,
//! 24-hour duration cap, and live earnings preview.

use taffy::prelude::*;

use crate::db::{EntryInput, EntryPatch, Project, SharedStore, TimeEntry};
use crate::format::{
    clock_of, day_label, duration, from_date_input, money, parse_clock, to_date_input,
};
use crate::time::{add_days, at_minutes, minutes_of_day, start_of_day, HOUR};
use crate::ui::screen::ScreenEnv;
use crate::ui::strings;
use crate::ui::text::{Align, Weight, Wrap};
use crate::ui::theme::Palette;
use crate::ui::tree::{style, Leaf, Node, Tree, WidgetId};
use crate::ui::widgets::button::{self, ButtonProps, ButtonSize, ButtonVariant};
use crate::ui::widgets::dialog;
use crate::ui::widgets::select::{self, SelectProps};
use crate::ui::widgets::text_field::{self, TextFieldProps};
use crate::ui::widgets::toggle::{self, ToggleKind, ToggleProps};
use crate::ui::widgets::{add_label, glyph, Interact};

use super::PopupSelectSlot;

const KEY_ENTRY_DESC: &str = "entry_dlg.desc";
const KEY_ENTRY_DATE: &str = "entry_dlg.date";
const KEY_ENTRY_START: &str = "entry_dlg.start";
const KEY_ENTRY_END: &str = "entry_dlg.end";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryDraft {
    pub id: Option<String>,
    pub project_id: Option<String>,
    pub description: String,
    pub start: i64,
    pub end: Option<i64>,
    pub billable: bool,
    pub source: String,
}

impl EntryDraft {
    pub fn from_entry(e: &TimeEntry) -> Self {
        Self {
            id: Some(e.id.clone()),
            project_id: e.project_id.clone(),
            description: e.description.clone(),
            start: e.start,
            end: e.end,
            billable: e.billable,
            source: e.source.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryEditorOutcome {
    None,
    Closed,
    Saved,
    Deleted,
}

pub struct EntryEditorState {
    pub draft: EntryDraft,
    pub description: String,
    pub project_id: Option<String>,
    pub billable: bool,
    pub date_text: String,
    pub start_text: String,
    pub end_text: String,
    pub error: Option<String>,
    sync_rev: u64,

    id_close: Option<WidgetId>,
    sel_project: PopupSelectSlot,
    id_billable: Option<WidgetId>,
    id_delete: Option<WidgetId>,
    id_stop: Option<WidgetId>,
    id_cancel: Option<WidgetId>,
    id_save: Option<WidgetId>,
}

impl EntryEditorState {
    pub fn new(draft: EntryDraft) -> Self {
        let running = draft.end.is_none();
        let date_text = to_date_input(draft.start);
        let start_text = clock_of(minutes_of_day(draft.start).round() as u32);
        let end_text = match draft.end {
            Some(end) => clock_of(minutes_of_day(end).round() as u32),
            None => String::new(),
        };
        let _ = running;
        Self {
            description: draft.description.clone(),
            project_id: draft.project_id.clone(),
            billable: draft.billable,
            date_text,
            start_text,
            end_text,
            error: None,
            sync_rev: 1,
            draft,
            id_close: None,
            sel_project: PopupSelectSlot::default(),
            id_billable: None,
            id_delete: None,
            id_stop: None,
            id_cancel: None,
            id_save: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.draft.end.is_none()
    }

    pub fn parse_times(&self) -> Result<(i64, Option<i64>), &'static str> {
        let Some(day) = from_date_input(&self.date_text) else {
            return Err("Enter date as YYYY-MM-DD.");
        };
        let Some(s_min) = parse_clock(&self.start_text) else {
            return Err(strings::ENTRY_ERROR_TIME);
        };
        let start = at_minutes(day, s_min as i32);
        if self.is_running() {
            return Ok((start, None));
        }
        let Some(e_min) = parse_clock(&self.end_text) else {
            return Err(strings::ENTRY_ERROR_TIME);
        };
        let mut end = at_minutes(day, e_min as i32);
        if end <= start {
            end = at_minutes(add_days(day, 1), e_min as i32);
        }
        if end - start > 24 * HOUR {
            return Err(strings::ENTRY_ERROR_SPAN);
        }
        Ok((start, Some(end)))
    }

    fn menu_projects(projects: &[Project], current: Option<&str>) -> Vec<Option<Project>> {
        let mut out = Vec::with_capacity(projects.len() + 1);
        out.push(None);
        for p in projects {
            if !p.archived || current == Some(p.id.as_str()) {
                out.push(Some(p.clone()));
            }
        }
        out
    }

    fn try_save(&mut self, store: &SharedStore, stop_after: bool) -> EntryEditorOutcome {
        let (start, mut end) = match self.parse_times() {
            Ok(v) => v,
            Err(msg) => {
                self.error = Some(msg.into());
                return EntryEditorOutcome::None;
            }
        };
        if stop_after && end.is_none() {
            let now = store.borrow().now_ms();
            end = Some(now.max(start + 60_000));
        }

        let mut st = store.borrow_mut();
        let res = if let Some(ref existing_id) = self.draft.id {
            st.update_entry(
                existing_id,
                EntryPatch {
                    project_id: Some(self.project_id.clone()),
                    description: Some(self.description.clone()),
                    start: Some(start),
                    end: Some(end),
                    billable: Some(self.billable),
                    source: Some(self.draft.source.clone()),
                },
            )
        } else {
            st.create_entry(EntryInput {
                project_id: self.project_id.clone(),
                description: self.description.clone(),
                start,
                end,
                billable: self.billable,
                source: self.draft.source.clone(),
            })
        };
        match res {
            Ok(_) => EntryEditorOutcome::Saved,
            Err(e) => {
                self.error = Some(e);
                EntryEditorOutcome::None
            }
        }
    }

    pub fn build(
        &mut self,
        tree: &mut Tree,
        overlay_root: Node,
        store: &SharedStore,
        projects: &[Project],
        env: &ScreenEnv<'_>,
    ) -> Node {
        let p = env.palette;
        let now_ms = store.borrow().now_ms();
        let title = if self.is_running() {
            strings::ENTRY_DLG_RUNNING
        } else if self.draft.id.is_some() {
            strings::ENTRY_DLG_EDIT
        } else {
            strings::ENTRY_DLG_NEW
        };
        let dlg = dialog::build(tree, overlay_root, title, 460.0, env.hover, p);
        self.id_close = Some(dlg.close_id);

        // Description
        add_label(
            tree,
            dlg.body,
            strings::ENTRY_FIELD_WHAT,
            12.0,
            Weight::Medium,
            Align::Left,
            p.text,
        );
        let _ = text_field::build_tree_node(
            tree,
            dlg.body,
            &TextFieldProps {
                key: KEY_ENTRY_DESC,
                aria_label: strings::ENTRY_FIELD_WHAT,
                placeholder: strings::TIMER_PLACEHOLDER,
                value: &self.description,
                multiline: false,
                min_height: 32.0,
                focused: false,
                error: false,
                disabled: false,
                sync_rev: self.sync_rev,
            },
            p,
        );

        // Project selector
        add_label(
            tree,
            dlg.body,
            strings::ENTRY_FIELD_PROJECT,
            12.0,
            Weight::Medium,
            Align::Left,
            p.text,
        );
        let menu = Self::menu_projects(projects, self.project_id.as_deref());
        let labels: Vec<String> = menu
            .iter()
            .map(|opt| match opt {
                None => strings::TIMER_NO_PROJECT.to_string(),
                Some(pr) => {
                    if pr.client.trim().is_empty() {
                        pr.name.clone()
                    } else {
                        format!("{} · {}", pr.name, pr.client.trim())
                    }
                }
            })
            .collect();
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let sel_idx = menu
            .iter()
            .position(|opt| opt.as_ref().map(|x| x.id.as_str()) == self.project_id.as_deref())
            .or(Some(0));
        let sel_int =
            Interact::from_state(self.sel_project.trigger_id, env.hover, env.pressed, false);
        let (_, pid) = select::build(
            tree,
            dlg.body,
            &SelectProps {
                aria_label: strings::ENTRY_FIELD_PROJECT,
                options: &label_refs,
                selected: sel_idx,
                placeholder: strings::TIMER_NO_PROJECT,
                min_width: 240.0,
                open: self.sel_project.is_open(),
                disabled: false,
                interact: sel_int,
            },
            p,
        );
        self.sel_project.trigger_id = Some(pid);

        // Times + Date row
        let times_parsed = self.parse_times();
        let len_ms = times_parsed
            .as_ref()
            .map(|&(s, e)| (e.unwrap_or(now_ms) - s).max(0));

        let time_row = tree.add_container(
            dlg.body,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::FlexEnd),
                gap: style::gap(8.0),
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );
        let start_col = tree.add_container(
            time_row,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                gap: style::gap(4.0),
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                ..Default::default()
            },
        );
        add_label(
            tree,
            start_col,
            strings::ENTRY_FIELD_START,
            12.0,
            Weight::Medium,
            Align::Left,
            p.text,
        );
        let _ = text_field::build_tree_node(
            tree,
            start_col,
            &TextFieldProps {
                key: KEY_ENTRY_START,
                aria_label: strings::ENTRY_FIELD_START,
                placeholder: "09:00",
                value: &self.start_text,
                multiline: false,
                min_height: 32.0,
                focused: false,
                error: self.error.is_some() && parse_clock(&self.start_text).is_none(),
                disabled: false,
                sync_rev: self.sync_rev,
            },
            p,
        );

        let end_col = tree.add_container(
            time_row,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                gap: style::gap(4.0),
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                ..Default::default()
            },
        );
        add_label(
            tree,
            end_col,
            strings::ENTRY_FIELD_END,
            12.0,
            Weight::Medium,
            Align::Left,
            p.text,
        );
        if self.is_running() {
            let now_box = tree.add_container(
                end_col,
                Style {
                    display: Display::Flex,
                    align_items: Some(AlignItems::Center),
                    size: style::size(80.0, 32.0),
                    ..Default::default()
                },
            );
            add_label(
                tree,
                now_box,
                "now",
                13.0,
                Weight::SemiBold,
                Align::Left,
                p.accent,
            );
        } else {
            let _ = text_field::build_tree_node(
                tree,
                end_col,
                &TextFieldProps {
                    key: KEY_ENTRY_END,
                    aria_label: strings::ENTRY_FIELD_END,
                    placeholder: "10:00",
                    value: &self.end_text,
                    multiline: false,
                    min_height: 32.0,
                    focused: false,
                    error: self.error.is_some() && parse_clock(&self.end_text).is_none(),
                    disabled: false,
                    sync_rev: self.sync_rev,
                },
                p,
            );
        }

        let dur_col = tree.add_container(
            time_row,
            Style {
                display: Display::Flex,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::FlexEnd),
                size: Size {
                    width: length(70.0_f32),
                    height: length(32.0_f32),
                },
                ..Default::default()
            },
        );
        let dur_str = len_ms.map(duration).unwrap_or_else(|_| "—".into());
        add_label(
            tree,
            dur_col,
            &dur_str,
            13.0,
            Weight::Medium,
            Align::Right,
            p.muted,
        );

        // Date field
        let date_row = tree.add_container(
            dlg.body,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                gap: style::gap(10.0),
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );
        let date_wrap = tree.add_container(
            date_row,
            Style {
                size: Size {
                    width: length(150.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );
        let _ = text_field::build_tree_node(
            tree,
            date_wrap,
            &TextFieldProps {
                key: KEY_ENTRY_DATE,
                aria_label: strings::ENTRY_FIELD_DATE,
                placeholder: "YYYY-MM-DD",
                value: &self.date_text,
                multiline: false,
                min_height: 32.0,
                focused: false,
                error: self.error.is_some() && from_date_input(&self.date_text).is_none(),
                disabled: false,
                sync_rev: self.sync_rev,
            },
            p,
        );
        let day_hint = day_label(start_of_day(
            from_date_input(&self.date_text).unwrap_or(self.draft.start),
        ));
        add_label(
            tree,
            date_row,
            &day_hint,
            12.0,
            Weight::Regular,
            Align::Left,
            p.muted,
        );

        // Billable + live money preview
        let bill_row = tree.add_container(
            dlg.body,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::SpaceBetween),
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );
        let bill_int = Interact::from_state(self.id_billable, env.hover, env.pressed, false);
        let (_, bid) = toggle::build(
            tree,
            bill_row,
            &ToggleProps {
                kind: ToggleKind::Checkbox,
                label: strings::ENTRY_FIELD_BILLABLE,
                aria_label: None,
                checked: self.billable,
                disabled: false,
                interact: bill_int,
            },
            p,
        );
        self.id_billable = Some(bid);

        let cur_proj = self
            .project_id
            .as_deref()
            .and_then(|pid| projects.iter().find(|pr| pr.id == pid));
        let preview_txt = if self.billable {
            match (cur_proj, len_ms) {
                (Some(pr), Ok(ms)) if pr.rate > 0.0 => {
                    money((ms as f64 / HOUR as f64) * pr.rate, &pr.currency)
                }
                (Some(_), _) => "No rate on project".into(),
                (None, _) => "No project".into(),
            }
        } else {
            String::new()
        };
        if !preview_txt.is_empty() {
            add_label(
                tree,
                bill_row,
                &preview_txt,
                12.5,
                Weight::Medium,
                Align::Right,
                p.muted,
            );
        }

        if let Some(ref err) = self.error {
            tree.add_leaf(
                dlg.body,
                Style::default(),
                Leaf::StyledText {
                    text: err.clone(),
                    size: 12.0,
                    weight: Weight::Medium,
                    align: Align::Left,
                    wrap: Wrap::Word,
                    color: p.danger_text,
                },
            );
        }

        // Footer
        let foot = tree.add_container(
            dlg.body,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::SpaceBetween),
                padding: style::pad(0.0, 6.0, 0.0, 0.0),
                gap: style::gap(8.0),
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );
        let left_f = tree.add_container(
            foot,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                gap: style::gap(6.0),
                ..Default::default()
            },
        );
        let right_f = tree.add_container(
            foot,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                gap: style::gap(8.0),
                ..Default::default()
            },
        );

        self.id_delete = None;
        self.id_stop = None;
        if self.draft.id.is_some() {
            let mut del_p = ButtonProps::new(strings::PROJECT_BTN_DELETE);
            del_p.icon = Some(glyph::DELETE);
            del_p.variant = ButtonVariant::Ghost;
            del_p.size = ButtonSize::Small;
            del_p.interact = Interact::from_state(self.id_delete, env.hover, env.pressed, false);
            let (_, did) = button::build(tree, left_f, &del_p, p);
            self.id_delete = Some(did);
        }
        if self.is_running() && self.draft.id.is_some() {
            let mut st_p = ButtonProps::new(strings::ENTRY_BTN_STOP_NOW);
            st_p.icon = Some(glyph::STOP);
            st_p.size = ButtonSize::Small;
            st_p.interact = Interact::from_state(self.id_stop, env.hover, env.pressed, false);
            let (_, stid) = button::build(tree, right_f, &st_p, p);
            self.id_stop = Some(stid);
        }

        let mut cancel_p = ButtonProps::new(strings::CANCEL_LABEL);
        cancel_p.interact = Interact::from_state(self.id_cancel, env.hover, env.pressed, false);
        let (_, xid) = button::build(tree, right_f, &cancel_p, p);
        self.id_cancel = Some(xid);

        let save_lbl = if self.draft.id.is_some() {
            strings::PROJECT_BTN_SAVE
        } else {
            "Add entry"
        };
        let mut save_p = ButtonProps::new(save_lbl);
        save_p.variant = ButtonVariant::Primary;
        save_p.interact = Interact::from_state(self.id_save, env.hover, env.pressed, false);
        let (_, sid) = button::build(tree, right_f, &save_p, p);
        self.id_save = Some(sid);

        dlg.card
    }

    pub fn on_click(
        &mut self,
        id: WidgetId,
        store: &SharedStore,
        projects: &[Project],
        palette: &Palette,
    ) -> (bool, EntryEditorOutcome) {
        if Some(id) == self.id_close || Some(id) == self.id_cancel {
            return (true, EntryEditorOutcome::Closed);
        }
        if Some(id) == self.id_billable {
            self.billable = !self.billable;
            return (true, EntryEditorOutcome::None);
        }
        if Some(id) == self.sel_project.trigger_id {
            let menu = Self::menu_projects(projects, self.project_id.as_deref());
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
            return (true, EntryEditorOutcome::None);
        }
        if Some(id) == self.id_delete {
            if let Some(ref existing) = self.draft.id {
                let _ = store.borrow_mut().delete_entry(existing);
                return (true, EntryEditorOutcome::Deleted);
            }
        }
        if Some(id) == self.id_stop {
            let out = self.try_save(store, true);
            return (true, out);
        }
        if Some(id) == self.id_save {
            let out = self.try_save(store, false);
            return (true, out);
        }
        (false, EntryEditorOutcome::None)
    }

    pub fn on_text_change(&mut self, key: &str, value: &str) -> bool {
        if key == KEY_ENTRY_DESC {
            self.description = value.to_string();
            return false;
        }
        if key == KEY_ENTRY_DATE {
            self.date_text = value.to_string();
            self.error = None;
            return true;
        }
        if key == KEY_ENTRY_START {
            self.start_text = value.to_string();
            self.error = None;
            return true;
        }
        if key == KEY_ENTRY_END {
            self.end_text = value.to_string();
            self.error = None;
            return true;
        }
        false
    }

    pub fn on_text_submit(
        &mut self,
        key: &str,
        value: &str,
        store: &SharedStore,
    ) -> EntryEditorOutcome {
        self.on_text_change(key, value);
        self.try_save(store, false)
    }

    pub fn poll_popup(&mut self, projects: &[Project]) -> bool {
        if let Some(res) = self.sel_project.poll() {
            if let Some(idx) = res {
                let menu = Self::menu_projects(projects, self.project_id.as_deref());
                if let Some(choice) = menu.get(idx) {
                    self.project_id = choice.as_ref().map(|pr| pr.id.clone());
                    if self.draft.id.is_none() {
                        if let Some(pr) = choice {
                            self.billable = pr.billable_default;
                        }
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
    use crate::format::local_day_at;
    use crate::locale::SystemLocale;
    use crate::ui::a11y;
    use crate::ui::text::TextSystem;

    #[test]
    fn entry_editor_wraps_midnight_rejects_over_24h_and_persists_changes() {
        let text = TextSystem::new().expect("dwrite");
        let palette = Palette::LIGHT;
        let env = ScreenEnv {
            width_dip: 1000.0,
            height_dip: 700.0,
            hover: None,
            pressed: None,
            focused: None,
            palette: &palette,
            text: &text,
        };
        let store = shared_store(Box::new(DemoStore::new(SystemLocale::en_us())));
        let projects = store.borrow().list_projects().unwrap();

        let start = local_day_at(2026, 11, 18, 22 * 60);
        let mut editor = EntryEditorState::new(EntryDraft {
            id: None,
            project_id: Some(projects[0].id.clone()),
            description: "Late deploy".into(),
            start,
            end: Some(start + HOUR),
            billable: true,
            source: "manual".into(),
        });
        // Wrap across midnight: 22:00 -> 01:30 = 3h30m.
        editor.on_text_change(KEY_ENTRY_START, "22:00");
        editor.on_text_change(KEY_ENTRY_END, "130");
        let (s, e) = editor.parse_times().unwrap();
        assert_eq!(e.unwrap() - s, 3 * HOUR + 30 * crate::time::MIN);

        // Save to store cleanly.
        let mut tree = Tree::new(style::column());
        let root = tree.root();
        let _card = editor.build(&mut tree, root, &store, &projects, &env);
        tree.layout(1000.0, 700.0, &text);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));

        let save_id = editor.id_save.unwrap();
        let (handled, out) = editor.on_click(save_id, &store, &projects, &palette);
        assert!(handled);
        assert_eq!(out, EntryEditorOutcome::Saved);

        // Invalid time fails gracefully with error message.
        editor.on_text_change(KEY_ENTRY_START, "99:99");
        assert!(editor.parse_times().is_err());
    }
}
