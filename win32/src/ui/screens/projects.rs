//! Projects list screen (`ProjectsScreen`) using SQL-aggregated summaries
//! plus the reusable `ProjectDialogState` modal for create/edit/archive/delete.

use std::sync::atomic::{AtomicU64, Ordering};

use taffy::prelude::*;

use crate::db::{Project, ProjectInput, ProjectPatch, ProjectSummary, SharedStore};
use crate::format::{self, ALL_CURRENCIES};
use crate::time::start_of_month;
use crate::ui::input::KeyCode;
use crate::ui::screen::{BuiltScreen, Screen, ScreenEnv};
use crate::ui::strings;
use crate::ui::text::{Align, Weight, Wrap};
use crate::ui::theme::{parse_hex_color, Palette, PROJECT_COLORS, PROJECT_COLOR_HEX};
use crate::ui::tree::{style, A11yInfo, A11yRole, HitMode, Leaf, Node, PaintStyle, Tree, WidgetId};
use crate::ui::widgets::button::{self, ButtonProps, ButtonSize, ButtonVariant};
use crate::ui::widgets::dialog;
use crate::ui::widgets::select::{self, SelectProps};
use crate::ui::widgets::table::{self, SortDir, SortState, TableColumn, TableProps, TableRow};
use crate::ui::widgets::text_field::{self, TextFieldProps};
use crate::ui::widgets::toggle::{self, ToggleKind, ToggleProps};
use crate::ui::widgets::{add_label, glyph, interactive_a11y, Interact};

use super::{build_card, build_empty_state, screen_root_tree, PageScroll, PopupSelectSlot};

const KEY_PROJ_NAME: &str = "project_dlg.name";
const KEY_PROJ_CLIENT: &str = "project_dlg.client";
const KEY_PROJ_RATE: &str = "project_dlg.rate";

static NEXT_PROJECT_SYNC_REV: AtomicU64 = AtomicU64::new(1);

fn next_project_sync_rev() -> u64 {
    NEXT_PROJECT_SYNC_REV.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectDialogOutcome {
    None,
    Closed,
    Saved,
    Deleted,
}

/// Self-contained modal dialog state for creating or editing a `Project`.
pub struct ProjectDialogState {
    editing: Option<Project>,
    pub name: String,
    pub client: String,
    pub rate: String,
    pub currency: String,
    pub color: String,
    pub billable: bool,
    pub confirm_delete: bool,
    pub error: Option<String>,
    sync_rev: u64,

    id_close: Option<WidgetId>,
    id_billable: Option<WidgetId>,
    id_swatch_ids: Vec<WidgetId>,
    sel_currency: PopupSelectSlot,
    id_delete: Option<WidgetId>,
    id_keep: Option<WidgetId>,
    id_confirm_delete: Option<WidgetId>,
    id_archive: Option<WidgetId>,
    id_cancel: Option<WidgetId>,
    id_save: Option<WidgetId>,
}

impl ProjectDialogState {
    #[cfg(test)]
    pub fn sync_rev(&self) -> u64 {
        self.sync_rev
    }

    pub fn for_new(default_currency: &str, used_colors: &[String]) -> Self {
        let pick_color = PROJECT_COLOR_HEX
            .iter()
            .find(|c| !used_colors.iter().any(|u| u.eq_ignore_ascii_case(c)))
            .copied()
            .unwrap_or(PROJECT_COLOR_HEX[0]);
        Self {
            editing: None,
            name: String::new(),
            client: String::new(),
            rate: String::new(),
            currency: default_currency.to_string(),
            color: pick_color.to_string(),
            billable: true,
            confirm_delete: false,
            error: None,
            sync_rev: next_project_sync_rev(),
            id_close: None,
            id_billable: None,
            id_swatch_ids: Vec::new(),
            sel_currency: PopupSelectSlot::default(),
            id_delete: None,
            id_keep: None,
            id_confirm_delete: None,
            id_archive: None,
            id_cancel: None,
            id_save: None,
        }
    }

    pub fn for_edit(p: Project) -> Self {
        let rate_str = if p.rate > 0.0 {
            format!("{}", p.rate)
        } else {
            String::new()
        };
        Self {
            name: p.name.clone(),
            client: p.client.clone(),
            rate: rate_str,
            currency: p.currency.clone(),
            color: p.color.clone(),
            billable: p.billable_default,
            editing: Some(p),
            confirm_delete: false,
            error: None,
            sync_rev: next_project_sync_rev(),
            id_close: None,
            id_billable: None,
            id_swatch_ids: Vec::new(),
            sel_currency: PopupSelectSlot::default(),
            id_delete: None,
            id_keep: None,
            id_confirm_delete: None,
            id_archive: None,
            id_cancel: None,
            id_save: None,
        }
    }

    fn currency_list(&self) -> Vec<String> {
        let mut opts: Vec<String> = ALL_CURRENCIES.iter().map(|c| (*c).to_string()).collect();
        if !self.currency.is_empty() && !opts.iter().any(|c| c.eq_ignore_ascii_case(&self.currency))
        {
            opts.insert(0, self.currency.clone());
        }
        opts
    }

    fn parse_rate(&self) -> Option<f64> {
        let trimmed = self.rate.trim();
        if trimmed.is_empty() {
            return Some(0.0);
        }
        let normalized = trimmed.replace(',', ".");
        let v: f64 = normalized.parse().ok()?;
        if v.is_finite() && v >= 0.0 {
            Some(v)
        } else {
            None
        }
    }

    fn try_save(&mut self, store: &SharedStore) -> ProjectDialogOutcome {
        let clean_name = self.name.trim();
        if clean_name.is_empty() {
            self.error = Some(strings::PROJECT_ERR_NO_NAME.into());
            return ProjectDialogOutcome::None;
        }
        let Some(rate_val) = self.parse_rate() else {
            self.error = Some(strings::PROJECT_ERR_BAD_RATE.into());
            return ProjectDialogOutcome::None;
        };

        let mut st = store.borrow_mut();
        let res = if let Some(ref existing) = self.editing {
            st.update_project(
                &existing.id,
                ProjectPatch {
                    name: Some(clean_name.to_string()),
                    client: Some(self.client.trim().to_string()),
                    color: Some(self.color.clone()),
                    rate: Some(rate_val),
                    currency: Some(self.currency.clone()),
                    billable_default: Some(self.billable),
                    archived: None,
                },
            )
        } else {
            st.create_project(ProjectInput {
                name: clean_name.to_string(),
                client: self.client.trim().to_string(),
                color: self.color.clone(),
                rate: rate_val,
                currency: self.currency.clone(),
                billable_default: self.billable,
                archived: false,
            })
        };
        match res {
            Ok(_) => ProjectDialogOutcome::Saved,
            Err(e) => {
                self.error = Some(e);
                ProjectDialogOutcome::None
            }
        }
    }

    pub fn build(&mut self, tree: &mut Tree, overlay_root: Node, env: &ScreenEnv<'_>) -> Node {
        let p = env.palette;
        let title = if self.editing.is_some() {
            strings::PROJECT_DLG_EDIT
        } else {
            strings::PROJECT_DLG_NEW
        };
        let dlg = dialog::build(tree, overlay_root, title, 460.0, env.hover, p);
        self.id_close = Some(dlg.close_id);

        // Name
        add_label(
            tree,
            dlg.body,
            strings::PROJECT_FIELD_NAME,
            12.0,
            Weight::Medium,
            Align::Left,
            p.text,
        );
        let name_err = self.error.is_some() && self.name.trim().is_empty();
        let (_, name_id) = text_field::build_tree_node(
            tree,
            dlg.body,
            &TextFieldProps {
                key: KEY_PROJ_NAME,
                aria_label: strings::PROJECT_FIELD_NAME,
                placeholder: strings::PROJECT_FIELD_NAME_HINT,
                value: &self.name,
                multiline: false,
                min_height: 32.0,
                focused: env.focused.is_some_and(|f| {
                    tree.all_nodes().iter().any(|n| {
                        tree.data(*n).is_some_and(|d| {
                            d.id == f && d.a11y.name == strings::PROJECT_FIELD_NAME
                        })
                    })
                }),
                error: name_err,
                disabled: false,
                sync_rev: self.sync_rev,
            },
            p,
        );
        let _ = name_id;

        // Client
        add_label(
            tree,
            dlg.body,
            strings::PROJECT_FIELD_CLIENT,
            12.0,
            Weight::Medium,
            Align::Left,
            p.text,
        );
        let _ = text_field::build_tree_node(
            tree,
            dlg.body,
            &TextFieldProps {
                key: KEY_PROJ_CLIENT,
                aria_label: strings::PROJECT_FIELD_CLIENT,
                placeholder: strings::PROJECT_FIELD_CLIENT_HINT,
                value: &self.client,
                multiline: false,
                min_height: 32.0,
                focused: false,
                error: false,
                disabled: false,
                sync_rev: self.sync_rev,
            },
            p,
        );

        // Rate + Currency row
        let rc_row = tree.add_container(
            dlg.body,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                gap: style::gap(12.0),
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );
        let rate_col = tree.add_container(
            rc_row,
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
            rate_col,
            strings::PROJECT_FIELD_RATE,
            12.0,
            Weight::Medium,
            Align::Left,
            p.text,
        );
        let _ = text_field::build_tree_node(
            tree,
            rate_col,
            &TextFieldProps {
                key: KEY_PROJ_RATE,
                aria_label: strings::PROJECT_FIELD_RATE,
                placeholder: strings::PROJECT_FIELD_RATE_HINT,
                value: &self.rate,
                multiline: false,
                min_height: 32.0,
                focused: false,
                error: self.parse_rate().is_none() && self.error.is_some(),
                disabled: false,
                sync_rev: self.sync_rev,
            },
            p,
        );

        let cur_col = tree.add_container(
            rc_row,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                gap: style::gap(4.0),
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        add_label(
            tree,
            cur_col,
            strings::PROJECT_FIELD_CURRENCY,
            12.0,
            Weight::Medium,
            Align::Left,
            p.text,
        );
        let cur_opts = self.currency_list();
        let cur_refs: Vec<&str> = cur_opts.iter().map(String::as_str).collect();
        let cur_idx = cur_opts
            .iter()
            .position(|c| c.eq_ignore_ascii_case(&self.currency));
        let sel_int =
            Interact::from_state(self.sel_currency.trigger_id, env.hover, env.pressed, false);
        let (_, cur_id) = select::build(
            tree,
            cur_col,
            &SelectProps {
                aria_label: strings::PROJECT_FIELD_CURRENCY,
                options: &cur_refs,
                selected: cur_idx,
                placeholder: strings::SELECT_PLACEHOLDER,
                min_width: 112.0,
                open: self.sel_currency.is_open(),
                disabled: false,
                interact: sel_int,
            },
            p,
        );
        self.sel_currency.trigger_id = Some(cur_id);

        // Color swatches RadioGroup (12 swatches)
        add_label(
            tree,
            dlg.body,
            strings::PROJECT_FIELD_COLOR,
            12.0,
            Weight::Medium,
            Align::Left,
            p.text,
        );
        let swatches = tree.add_container(
            dlg.body,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                gap: style::gap(8.0),
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );
        tree.set_a11y(
            swatches,
            A11yInfo {
                role: A11yRole::Group,
                name: strings::PROJECT_FIELD_COLOR.into(),
                requires_name: true,
                ..Default::default()
            },
        );
        self.id_swatch_ids.clear();
        for (idx, hex) in PROJECT_COLOR_HEX.iter().enumerate() {
            let selected = self.color.eq_ignore_ascii_case(hex);
            let sw = tree.add_container(
                swatches,
                Style {
                    display: Display::Flex,
                    align_items: Some(AlignItems::Center),
                    justify_content: Some(JustifyContent::Center),
                    size: style::size(24.0, 24.0),
                    ..Default::default()
                },
            );
            tree.set_paint(
                sw,
                PaintStyle {
                    bg: Some(PROJECT_COLORS[idx]),
                    border_color: if selected { Some(p.text) } else { None },
                    border_width: if selected { 2.0 } else { 0.0 },
                    radius: 12.0,
                    clip: false,
                },
            );
            tree.set_hit(sw, HitMode::Interactive);
            tree.set_focusable(sw, true);
            let mut info = interactive_a11y(
                A11yRole::RadioButton,
                format!("{} {}", strings::PROJECT_FIELD_COLOR, hex),
                false,
            );
            info.selected = Some(selected);
            tree.set_a11y(sw, info);
            if selected {
                tree.add_leaf(
                    sw,
                    Style::default(),
                    Leaf::Icon {
                        glyph: glyph::CHECK,
                        size: 12.0,
                        color: p.on_primary,
                    },
                );
            }
            self.id_swatch_ids.push(tree.id(sw));
        }

        // Billable checkbox
        let bill_int = Interact::from_state(self.id_billable, env.hover, env.pressed, false);
        let (_, bill_id) = toggle::build(
            tree,
            dlg.body,
            &ToggleProps {
                kind: ToggleKind::Checkbox,
                label: strings::PROJECT_FIELD_BILLABLE,
                aria_label: None,
                checked: self.billable,
                disabled: false,
                interact: bill_int,
            },
            p,
        );
        self.id_billable = Some(bill_id);

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

        // Footer buttons
        let footer = tree.add_container(
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
        let left_actions = tree.add_container(
            footer,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                gap: style::gap(8.0),
                ..Default::default()
            },
        );
        let right_actions = tree.add_container(
            footer,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                gap: style::gap(8.0),
                ..Default::default()
            },
        );

        self.id_delete = None;
        self.id_keep = None;
        self.id_confirm_delete = None;
        self.id_archive = None;
        self.id_cancel = None;
        self.id_save = None;

        if self.confirm_delete {
            let mut keep_p = ButtonProps::new(strings::SETTINGS_DELETE_KEEP_BTN);
            keep_p.size = ButtonSize::Small;
            let (_, kid) = button::build(tree, left_actions, &keep_p, p);
            self.id_keep = Some(kid);

            let mut del_p = ButtonProps::new(strings::PROJECT_BTN_CONFIRM_DELETE);
            del_p.variant = ButtonVariant::Danger;
            del_p.size = ButtonSize::Small;
            let (_, cid) = button::build(tree, right_actions, &del_p, p);
            self.id_confirm_delete = Some(cid);
        } else {
            if let Some(ref existing) = self.editing {
                let mut del_p = ButtonProps::new(strings::PROJECT_BTN_DELETE);
                del_p.variant = ButtonVariant::Ghost;
                del_p.size = ButtonSize::Small;
                let (_, did) = button::build(tree, left_actions, &del_p, p);
                self.id_delete = Some(did);

                let mut arch_p = ButtonProps::new(if existing.archived {
                    strings::PROJECT_BTN_UNARCHIVE
                } else {
                    strings::PROJECT_BTN_ARCHIVE
                });
                arch_p.variant = ButtonVariant::Ghost;
                arch_p.size = ButtonSize::Small;
                let (_, aid) = button::build(tree, left_actions, &arch_p, p);
                self.id_archive = Some(aid);
            }

            let mut cancel_p = ButtonProps::new(strings::CANCEL_LABEL);
            cancel_p.interact = Interact::from_state(self.id_cancel, env.hover, env.pressed, false);
            let (_, xid) = button::build(tree, right_actions, &cancel_p, p);
            self.id_cancel = Some(xid);

            let save_lbl = if self.editing.is_some() {
                strings::PROJECT_BTN_SAVE
            } else {
                strings::PROJECT_BTN_CREATE
            };
            let mut save_p = ButtonProps::new(save_lbl);
            save_p.variant = ButtonVariant::Primary;
            save_p.interact = Interact::from_state(self.id_save, env.hover, env.pressed, false);
            let (_, sid) = button::build(tree, right_actions, &save_p, p);
            self.id_save = Some(sid);
        }

        dlg.card
    }

    pub fn on_click(
        &mut self,
        id: WidgetId,
        store: &SharedStore,
        palette: &Palette,
    ) -> (bool, ProjectDialogOutcome) {
        if Some(id) == self.id_close || Some(id) == self.id_cancel {
            return (true, ProjectDialogOutcome::Closed);
        }
        if Some(id) == self.id_billable {
            self.billable = !self.billable;
            return (true, ProjectDialogOutcome::None);
        }
        if let Some(pos) = self.id_swatch_ids.iter().position(|x| *x == id) {
            self.color = PROJECT_COLOR_HEX[pos].to_string();
            return (true, ProjectDialogOutcome::None);
        }
        if Some(id) == self.sel_currency.trigger_id {
            let opts = self.currency_list();
            let refs: Vec<&str> = opts.iter().map(String::as_str).collect();
            let idx = opts
                .iter()
                .position(|c| c.eq_ignore_ascii_case(&self.currency));
            self.sel_currency.open(&refs, idx, palette);
            return (true, ProjectDialogOutcome::None);
        }
        if Some(id) == self.id_delete {
            self.confirm_delete = true;
            return (true, ProjectDialogOutcome::None);
        }
        if Some(id) == self.id_keep {
            self.confirm_delete = false;
            return (true, ProjectDialogOutcome::None);
        }
        if Some(id) == self.id_confirm_delete {
            if let Some(ref existing) = self.editing {
                if let Err(e) = store.borrow_mut().delete_project(&existing.id) {
                    self.error = Some(e);
                    self.confirm_delete = false;
                    return (true, ProjectDialogOutcome::None);
                }
                return (true, ProjectDialogOutcome::Deleted);
            }
        }
        if Some(id) == self.id_archive {
            if let Some(ref existing) = self.editing {
                match store.borrow_mut().update_project(
                    &existing.id,
                    ProjectPatch {
                        archived: Some(!existing.archived),
                        ..Default::default()
                    },
                ) {
                    Ok(_) => return (true, ProjectDialogOutcome::Saved),
                    Err(e) => {
                        self.error = Some(e);
                        return (true, ProjectDialogOutcome::None);
                    }
                }
            }
        }
        if Some(id) == self.id_save {
            let out = self.try_save(store);
            return (true, out);
        }
        (false, ProjectDialogOutcome::None)
    }

    pub fn on_text_change(&mut self, key: &str, value: &str) -> bool {
        if key == KEY_PROJ_NAME {
            self.name = value.to_string();
            return self.error.take().is_some();
        }
        if key == KEY_PROJ_CLIENT {
            self.client = value.to_string();
            return false;
        }
        if key == KEY_PROJ_RATE {
            self.rate = value.to_string();
            return self.error.take().is_some();
        }
        false
    }

    pub fn on_text_submit(
        &mut self,
        key: &str,
        value: &str,
        store: &SharedStore,
    ) -> ProjectDialogOutcome {
        self.on_text_change(key, value);
        self.try_save(store)
    }

    pub fn on_key(&mut self, key: KeyCode) -> bool {
        if let Some(cur_pos) = PROJECT_COLOR_HEX
            .iter()
            .position(|c| self.color.eq_ignore_ascii_case(c))
        {
            match key {
                KeyCode::Right => {
                    self.color =
                        PROJECT_COLOR_HEX[(cur_pos + 1) % PROJECT_COLOR_HEX.len()].to_string();
                    return true;
                }
                KeyCode::Left => {
                    self.color = PROJECT_COLOR_HEX
                        [(cur_pos + PROJECT_COLOR_HEX.len() - 1) % PROJECT_COLOR_HEX.len()]
                    .to_string();
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    pub fn poll_popup(&mut self) -> bool {
        if let Some(res) = self.sel_currency.poll() {
            if let Some(idx) = res {
                let opts = self.currency_list();
                if let Some(cur) = opts.get(idx) {
                    self.currency = cur.clone();
                }
            }
            return true;
        }
        false
    }
}

/// Projects screen showing all projects + SQL summaries + New/Edit modal.
pub struct ProjectsScreen {
    store: SharedStore,
    show_archived: bool,
    sort: SortState,
    scroll: PageScroll,
    dialog: Option<ProjectDialogState>,

    id_new_btn: Option<WidgetId>,
    id_empty_new_btn: Option<WidgetId>,
    id_archived_toggle: Option<WidgetId>,
    table_header_ids: Vec<Option<WidgetId>>,
    row_project_ids: Vec<(WidgetId, String)>,

    /// Set when the user clicks a project row; consumed by `AppShell` or caller.
    pub opened_project: Option<String>,
    on_open_project: Option<Box<dyn FnMut(String)>>,
}

impl ProjectsScreen {
    pub fn new(store: SharedStore) -> Self {
        Self {
            store,
            show_archived: false,
            sort: SortState {
                col: 2,
                dir: SortDir::Desc,
            },
            scroll: PageScroll::default(),
            dialog: None,
            id_new_btn: None,
            id_empty_new_btn: None,
            id_archived_toggle: None,
            table_header_ids: Vec::new(),
            row_project_ids: Vec::new(),
            opened_project: None,
            on_open_project: None,
        }
    }

    pub fn has_modal(&self) -> bool {
        self.dialog.is_some()
    }

    pub fn reset_transient(&mut self) {
        self.dialog = None;
        self.opened_project = None;
        self.scroll.reset();
    }

    #[cfg(test)]
    pub fn with_on_open_project(mut self, f: impl FnMut(String) + 'static) -> Self {
        self.on_open_project = Some(Box::new(f));
        self
    }

    fn sort_projects(rows: &mut [(Project, ProjectSummary)], sort: SortState) {
        rows.sort_by(|(pa, sa), (pb, sb)| {
            let ord = match sort.col {
                0 => pa.name.to_lowercase().cmp(&pb.name.to_lowercase()),
                1 => pa.rate.total_cmp(&pb.rate),
                2 => sa.month_ms.cmp(&sb.month_ms),
                3 => sa.total_ms.cmp(&sb.total_ms),
                4 => sa.earned.total_cmp(&sb.earned),
                _ => pa.created_at.cmp(&pb.created_at),
            };
            match sort.dir {
                SortDir::Asc => ord,
                SortDir::Desc => ord.reverse(),
            }
        });
    }

    pub fn build_content(
        &mut self,
        tree: &mut Tree,
        overlay_root: Node,
        parent: Node,
        env: &ScreenEnv<'_>,
    ) -> Option<Node> {
        let p = env.palette;

        let (now_ms, all_projects, summaries) = {
            let st = self.store.borrow();
            let now = st.now_ms();
            let month_from = start_of_month(now);
            let month_to = crate::time::start_of_day(crate::time::add_months(month_from, 1));
            let projs = st.list_projects().unwrap_or_default();
            let sums = st
                .project_summaries(month_from, month_to)
                .unwrap_or_default();
            (now, projs, sums)
        };
        let _ = now_ms;

        let archived_count = all_projects.iter().filter(|pr| pr.archived).count();
        let mut visible: Vec<(Project, ProjectSummary)> = all_projects
            .iter()
            .filter(|pr| self.show_archived || !pr.archived)
            .map(|pr| {
                let sum = summaries.get(&pr.id).copied().unwrap_or_default();
                (pr.clone(), sum)
            })
            .collect();
        Self::sort_projects(&mut visible, self.sort);

        let col = self.scroll.build(tree, parent, 960.0, 28.0, 16.0);

        // Header
        let head = tree.add_container(
            col,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::SpaceBetween),
                flex_shrink: 0.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );
        let headline = if visible.is_empty() {
            strings::PROJECTS_TITLE.to_string()
        } else if visible.len() == 1 {
            strings::PROJECTS_COUNT_ONE.into()
        } else {
            format!("{} {}", visible.len(), strings::PROJECTS_COUNT_MANY_SUFFIX)
        };
        add_label(
            tree,
            head,
            &headline,
            22.0,
            Weight::SemiBold,
            Align::Left,
            p.text,
        );

        let mut new_p = ButtonProps::new(strings::PROJECTS_NEW_BTN);
        new_p.icon = Some(glyph::ADD);
        new_p.variant = ButtonVariant::Primary;
        new_p.interact = Interact::from_state(self.id_new_btn, env.hover, env.pressed, false);
        let (_, new_id) = button::build(tree, head, &new_p, p);
        self.id_new_btn = Some(new_id);

        self.table_header_ids.clear();
        self.row_project_ids.clear();
        self.id_empty_new_btn = None;

        if visible.is_empty() {
            let empty = build_empty_state(
                tree,
                col,
                glyph::PROJECTS,
                strings::PROJECTS_EMPTY_TITLE,
                strings::PROJECTS_EMPTY_SUB,
                p,
            );
            let mut empty_btn = ButtonProps::new(strings::PROJECTS_NEW_BTN);
            empty_btn.icon = Some(glyph::ADD);
            empty_btn.variant = ButtonVariant::Primary;
            let (_, eid) = button::build(tree, empty, &empty_btn, p);
            self.id_empty_new_btn = Some(eid);
        } else {
            let card = build_card(tree, col, 0.0, 0.0, p);
            let cols = [
                TableColumn {
                    label: strings::PROJECTS_COL_NAME,
                    width: None,
                    align: Align::Left,
                    sortable: true,
                },
                TableColumn {
                    label: strings::PROJECTS_COL_RATE,
                    width: Some(110.0),
                    align: Align::Right,
                    sortable: true,
                },
                TableColumn {
                    label: strings::PROJECTS_COL_MONTH,
                    width: Some(100.0),
                    align: Align::Right,
                    sortable: true,
                },
                TableColumn {
                    label: strings::PROJECTS_COL_TOTAL,
                    width: Some(100.0),
                    align: Align::Right,
                    sortable: true,
                },
                TableColumn {
                    label: strings::PROJECTS_COL_EARNED,
                    width: Some(110.0),
                    align: Align::Right,
                    sortable: true,
                },
            ];
            let mut columns = cols;
            columns[4].label = strings::PROJECTS_COL_EARNED;

            // Prepare formatted cell strings cleanly so borrows outlive `TableRow`.
            let formatted: Vec<[String; 5]> = visible
                .iter()
                .map(|(proj, sum)| {
                    let mut name_cell = proj.name.clone();
                    let mut meta_parts = Vec::new();
                    if !proj.client.trim().is_empty() {
                        meta_parts.push(proj.client.trim().to_string());
                    }
                    if proj.archived {
                        meta_parts.push(strings::PROJECTS_STATUS_ARCHIVED.into());
                    }
                    if !meta_parts.is_empty() {
                        name_cell.push_str("  ·  ");
                        name_cell.push_str(&meta_parts.join(" · "));
                    }
                    let rate_cell = if proj.rate > 0.0 {
                        format!("{}/h", format::money(proj.rate, &proj.currency))
                    } else {
                        strings::PROJECTS_INTERNAL.into()
                    };
                    let month_cell = if sum.month_ms > 0 {
                        format::hm(sum.month_ms)
                    } else {
                        "—".into()
                    };
                    let total_cell = if sum.total_ms > 0 {
                        format::hm(sum.total_ms)
                    } else {
                        "—".into()
                    };
                    let earned_cell = if sum.earned > 0.0 {
                        format::money(sum.earned, &proj.currency)
                    } else {
                        "—".into()
                    };
                    [name_cell, rate_cell, month_cell, total_cell, earned_cell]
                })
                .collect();

            let table_rows_data: Vec<([&str; 5], String)> = formatted
                .iter()
                .zip(visible.iter())
                .map(|(cells, (proj, _))| {
                    let _ = parse_hex_color(&proj.color);
                    let aria = format!("{}, {}, {}", cells[0], cells[1], cells[3]);
                    (
                        [
                            cells[0].as_str(),
                            cells[1].as_str(),
                            cells[2].as_str(),
                            cells[3].as_str(),
                            cells[4].as_str(),
                        ],
                        aria,
                    )
                })
                .collect();

            let row_refs: Vec<TableRow<'_>> = table_rows_data
                .iter()
                .map(|(cells, aria)| TableRow {
                    aria_name: aria.as_str(),
                    cells: cells.as_slice(),
                    selected: false,
                })
                .collect();

            let built_table = table::build(
                tree,
                card,
                &TableProps {
                    aria_name: strings::PROJECTS_TITLE,
                    columns: &columns,
                    rows: &row_refs,
                    sort: Some(self.sort),
                    hover: env.hover,
                },
                p,
            );
            self.table_header_ids = built_table.header_ids;
            for (rid, (proj, _)) in built_table.row_ids.into_iter().zip(visible) {
                self.row_project_ids.push((rid, proj.id));
            }
        }

        if archived_count > 0 {
            let foot = tree.add_container(
                col,
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    justify_content: Some(JustifyContent::FlexStart),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
            let lbl = if self.show_archived {
                strings::PROJECTS_HIDE_ARCHIVED.to_string()
            } else {
                format!(
                    "{} {archived_count} {}",
                    strings::PROJECTS_ARCHIVED_SHOW_PREFIX,
                    strings::PROJECTS_ARCHIVED_SUFFIX
                )
            };
            let mut btn = ButtonProps::new(&lbl);
            btn.variant = ButtonVariant::Ghost;
            btn.size = ButtonSize::Small;
            btn.interact =
                Interact::from_state(self.id_archived_toggle, env.hover, env.pressed, false);
            let (_, aid) = button::build(tree, foot, &btn, p);
            self.id_archived_toggle = Some(aid);
        } else {
            self.id_archived_toggle = None;
        }

        self.dialog
            .as_mut()
            .map(|dlg| dlg.build(tree, overlay_root, env))
    }

    pub fn open_new_dialog(&mut self) {
        let st = self.store.borrow();
        let default_cur = st
            .get_settings()
            .map(|s| s.currency)
            .unwrap_or_else(|_| "USD".into());
        let used: Vec<String> = st
            .list_projects()
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.color)
            .collect();
        self.dialog = Some(ProjectDialogState::for_new(&default_cur, &used));
    }

    pub fn handle_click(&mut self, id: WidgetId, palette: &Palette) -> bool {
        if let Some(ref mut dlg) = self.dialog {
            let (handled, outcome) = dlg.on_click(id, &self.store, palette);
            if matches!(
                outcome,
                ProjectDialogOutcome::Closed
                    | ProjectDialogOutcome::Saved
                    | ProjectDialogOutcome::Deleted
            ) {
                self.dialog = None;
            }
            if handled {
                return true;
            }
        }
        if Some(id) == self.id_new_btn || Some(id) == self.id_empty_new_btn {
            self.open_new_dialog();
            return true;
        }
        if Some(id) == self.id_archived_toggle {
            self.show_archived = !self.show_archived;
            return true;
        }
        for (col_idx, hid) in self.table_header_ids.iter().enumerate() {
            if *hid == Some(id) {
                if self.sort.col == col_idx {
                    self.sort.dir = self.sort.dir.toggled();
                } else {
                    self.sort = SortState {
                        col: col_idx,
                        dir: if col_idx == 0 {
                            SortDir::Asc
                        } else {
                            SortDir::Desc
                        },
                    };
                }
                return true;
            }
        }
        if let Some((_, pid)) = self.row_project_ids.iter().find(|(rid, _)| *rid == id) {
            let pid = pid.clone();
            self.opened_project = Some(pid.clone());
            if let Some(cb) = self.on_open_project.as_mut() {
                cb(pid);
            }
            return true;
        }
        false
    }

    pub fn handle_text_change(&mut self, key: &str, value: &str) -> bool {
        self.dialog
            .as_mut()
            .is_some_and(|d| d.on_text_change(key, value))
    }

    pub fn handle_text_submit(&mut self, key: &str, value: &str) -> bool {
        if let Some(ref mut dlg) = self.dialog {
            let out = dlg.on_text_submit(key, value, &self.store);
            if matches!(
                out,
                ProjectDialogOutcome::Saved | ProjectDialogOutcome::Closed
            ) {
                self.dialog = None;
            }
            return true;
        }
        false
    }

    pub fn handle_key(&mut self, key: KeyCode) -> bool {
        if let Some(ref mut dlg) = self.dialog {
            return dlg.on_key(key);
        }
        self.scroll.on_key(key)
    }

    pub fn sync_after_layout(
        &mut self,
        tree: &mut Tree,
        focused: Option<WidgetId>,
        scroll_to_focused: bool,
    ) {
        self.scroll.sync(tree, focused, scroll_to_focused);
    }

    pub fn handle_escape(&mut self) -> bool {
        if self.dialog.is_some() {
            self.dialog = None;
            true
        } else {
            false
        }
    }

    pub fn handle_wheel(&mut self, delta_dip: f32) -> bool {
        if self.dialog.is_some() {
            return false;
        }
        self.scroll.wheel(delta_dip)
    }

    pub fn handle_popup_poll(&mut self) -> bool {
        self.dialog.as_mut().is_some_and(|d| d.poll_popup())
    }
}

impl Screen for ProjectsScreen {
    fn build(&mut self, env: &ScreenEnv<'_>) -> BuiltScreen {
        let mut tree = screen_root_tree(env);
        let root = tree.root();
        let modal_root = self.build_content(&mut tree, root, root, env);
        BuiltScreen { tree, modal_root }
    }

    fn on_click(&mut self, id: WidgetId) -> bool {
        self.handle_click(id, &Palette::LIGHT)
    }

    fn on_text_change(&mut self, key: &str, value: &str) -> bool {
        self.handle_text_change(key, value)
    }

    fn on_text_submit(&mut self, key: &str, value: &str) -> bool {
        self.handle_text_submit(key, value)
    }

    fn on_key(
        &mut self,
        ev: crate::ui::input::KeyEvent,
        _focus: &mut crate::ui::focus::FocusModel,
    ) -> bool {
        self.handle_key(ev.key)
    }

    fn on_escape(&mut self) -> bool {
        self.handle_escape()
    }

    fn on_wheel(&mut self, delta_dip: f32) -> bool {
        self.handle_wheel(delta_dip)
    }

    fn on_popup_poll(&mut self) -> bool {
        self.handle_popup_poll()
    }

    fn sync_after_layout(
        &mut self,
        tree: &mut Tree,
        focused: Option<WidgetId>,
        scroll_to_focused: bool,
    ) {
        ProjectsScreen::sync_after_layout(self, tree, focused, scroll_to_focused);
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
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn projects_screen_uses_sql_summaries_validates_a11y_and_creates_project_via_modal() {
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
        let store = shared_store(Box::new(DemoStore::new(SystemLocale::en_us())));
        let initial_count = store.borrow().list_projects().unwrap().len();
        let opened: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let opened_cb = Rc::clone(&opened);
        let screen = ProjectsScreen::new(Rc::clone(&store)).with_on_open_project(move |id| {
            *opened_cb.borrow_mut() = Some(id);
        });

        let mut focus = FocusModel::new();
        let mut host = ScreenHost::mount(Box::new(screen), &env, &mut focus);
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));

        // Open "New project" modal dialog via first focusable button (New project).
        let new_btn = focus.tab(false).unwrap();
        assert!(host.on_click(new_btn));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));
        assert!(matches!(focus.active_scope(), FocusScope::Modal(_)));

        // Blank name shows validation error instead of closing.
        let save_id = host
            .tree()
            .all_nodes()
            .into_iter()
            .filter_map(|n| host.tree().data(n))
            .find(|d| d.a11y.name == strings::PROJECT_BTN_CREATE)
            .unwrap()
            .id;
        assert!(host.on_click(save_id));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert!(matches!(focus.active_scope(), FocusScope::Modal(_)));

        // Typing into Name clears the validation error AND marks dirty to redraw it away.
        assert!(host.on_text_change(KEY_PROJ_NAME, "Telemetry Portal"));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert!(!host.on_text_change(KEY_PROJ_RATE, "180"));
        let save_id2 = host
            .tree()
            .all_nodes()
            .into_iter()
            .filter_map(|n| host.tree().data(n))
            .find(|d| d.a11y.name == strings::PROJECT_BTN_CREATE)
            .unwrap()
            .id;
        assert!(host.on_click(save_id2));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(focus.active_scope(), FocusScope::Main);
        assert_eq!(
            store.borrow().list_projects().unwrap().len(),
            initial_count + 1
        );

        // Clicking a table row emits opened_project.
        let row_id = host
            .tree()
            .all_nodes()
            .into_iter()
            .filter_map(|n| host.tree().data(n))
            .find(|d| d.a11y.role == A11yRole::Row && d.a11y.clickable)
            .unwrap()
            .id;
        assert!(host.on_click(row_id));
        assert!(opened.borrow().is_some());
    }
}
