//! Quick-timer & away-recovery tray flyout hosted in `PopupWindow`.
//!
//! Anchored near the taskbar notification icon via `Shell_NotifyIconGetRect`
//! with an inline project picker (never spawning a nested second `PopupWindow`).

use std::path::{Path, PathBuf};

use taffy::prelude::*;
use windows::Win32::Foundation::HWND;

use crate::db::{Db, EntryPatch, Project, TimeEntry, TimerStartInput};
use crate::db_thread::DbHandle;
use crate::format;
use crate::ui::focus::FocusModel;
use crate::ui::input::KeyCode;
use crate::ui::popup::{self, PopupAlign, PopupDelegate, PopupEvent, PopupOutcome, ScreenRect};
use crate::ui::prefs::SystemPrefs;
use crate::ui::strings;
use crate::ui::text::{Align, Weight, Wrap};
use crate::ui::theme::{parse_hex_color, Color, Palette, NO_PROJECT};
use crate::ui::tree::{style, A11yInfo, A11yRole, HitMode, Leaf, PaintStyle, Tree, WidgetId};
use crate::ui::widgets::button::{self, ButtonProps, ButtonSize, ButtonVariant};
use crate::ui::widgets::text_field::{self, TextFieldProps};
use crate::ui::widgets::{add_label, glyph, interactive_a11y, Interact};

const FLYOUT_WIDTH_DIP: f32 = 316.0;
const BASE_TIMER_H_DIP: f32 = 222.0;
const AWAY_H_DIP: f32 = 166.0;

#[derive(Debug, Clone)]
pub enum FlyoutMode {
    Timer,
    AwayPrompt {
        entry_id: String,
        away_started_ms: i64,
        label: String,
        elapsed_away_ms: i64,
    },
}

struct FlyoutState {
    db_path: PathBuf,
    db_handle: DbHandle,
    mode: FlyoutMode,
    palette: Palette,
    projects: Vec<Project>,
    running: Option<TimeEntry>,
    selected_project: Option<String>,
    description: String,
    desc_sync_rev: u64,
    picker_open: bool,
    hover: Option<WidgetId>,
    pressed: Option<WidgetId>,
    id_open_app: Option<WidgetId>,
    id_picker_toggle: Option<WidgetId>,
    id_project_rows: Vec<(WidgetId, Option<String>)>,
    id_primary_btn: Option<WidgetId>,
    id_secondary_btn: Option<WidgetId>,
    on_open_app: fn(),
    on_data_changed: fn(),
}

fn system_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl FlyoutState {
    fn from_db(
        db_path: PathBuf,
        db_handle: DbHandle,
        mode: FlyoutMode,
        palette: Palette,
        on_open_app: fn(),
        on_data_changed: fn(),
    ) -> Self {
        let (projects, running) = match Db::open_readonly(&db_path) {
            Ok(db) => {
                let ps = db
                    .list_projects()
                    .map(|list| list.into_iter().filter(|p| !p.archived).collect())
                    .unwrap_or_default();
                let r = db.running_entry().ok().flatten();
                (ps, r)
            }
            Err(_) => (Vec::new(), None),
        };
        let (selected_project, description) = if let Some(ref r) = running {
            (r.project_id.clone(), r.description.clone())
        } else {
            (projects.first().map(|p| p.id.clone()), String::new())
        };
        Self {
            db_path,
            db_handle,
            mode,
            palette,
            projects,
            running,
            selected_project,
            description,
            desc_sync_rev: 1,
            picker_open: false,
            hover: None,
            pressed: None,
            id_open_app: None,
            id_picker_toggle: None,
            id_project_rows: Vec::new(),
            id_primary_btn: None,
            id_secondary_btn: None,
            on_open_app,
            on_data_changed,
        }
    }

    fn reload_running(&mut self) {
        if let Ok(db) = Db::open_readonly(&self.db_path) {
            self.projects = db
                .list_projects()
                .map(|list| list.into_iter().filter(|p| !p.archived).collect())
                .unwrap_or_default();
            self.running = db.running_entry().ok().flatten();
            if let Some(ref r) = self.running {
                self.selected_project = r.project_id.clone();
                self.description = r.description.clone();
                self.desc_sync_rev += 1;
            }
        }
    }

    fn selected_project_name_and_color(&self) -> (&str, Color) {
        if let Some(ref pid) = self.selected_project {
            if let Some(pr) = self.projects.iter().find(|p| &p.id == pid) {
                let color = parse_hex_color(&pr.color).unwrap_or(NO_PROJECT);
                return (&pr.name, color);
            }
        }
        (strings::FLYOUT_NO_PROJECT, NO_PROJECT)
    }

    fn desired_height_dip(&self) -> f32 {
        match self.mode {
            FlyoutMode::AwayPrompt { .. } => AWAY_H_DIP,
            FlyoutMode::Timer => {
                if !self.picker_open {
                    BASE_TIMER_H_DIP
                } else {
                    let item_count = (self.projects.len() + 1).min(6) as f32;
                    BASE_TIMER_H_DIP + item_count * 30.0 + 14.0
                }
            }
        }
    }

    fn build_tree(&mut self, now_ms: i64) -> Tree {
        let p = &self.palette;
        let mut tree = Tree::new(Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            gap: style::gap(10.0),
            padding: style::pad(14.0, 14.0, 14.0, 14.0),
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
                bg: Some(p.surface),
                border_color: Some(p.card_border),
                border_width: p.border_width,
                radius: 8.0,
                clip: true,
            },
        );
        tree.set_a11y(
            root,
            A11yInfo {
                role: A11yRole::Group,
                name: strings::FLYOUT_TITLE.into(),
                requires_name: true,
                ..Default::default()
            },
        );

        self.id_open_app = None;
        self.id_picker_toggle = None;
        self.id_project_rows.clear();
        self.id_primary_btn = None;
        self.id_secondary_btn = None;

        match &self.mode {
            FlyoutMode::AwayPrompt {
                label,
                elapsed_away_ms,
                ..
            } => {
                add_label(
                    &mut tree,
                    root,
                    strings::FLYOUT_AWAY_TITLE,
                    14.0,
                    Weight::SemiBold,
                    Align::Left,
                    p.text,
                );
                let msg = format!("{} ({})", label, format::short_duration(*elapsed_away_ms));
                tree.add_leaf(
                    root,
                    Style::default(),
                    Leaf::StyledText {
                        text: msg,
                        size: 12.5,
                        weight: Weight::Regular,
                        color: p.muted,
                        align: Align::Left,
                        wrap: Wrap::Word,
                    },
                );
                let spacer = tree.add_container(
                    root,
                    Style {
                        flex_grow: 1.0,
                        ..Default::default()
                    },
                );
                let _ = spacer;

                let actions = tree.add_container(
                    root,
                    Style {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Row,
                        justify_content: Some(JustifyContent::FlexEnd),
                        gap: style::gap(8.0),
                        ..Default::default()
                    },
                );
                let mut discard_props = ButtonProps::new(strings::FLYOUT_DISCARD_AWAY);
                discard_props.variant = ButtonVariant::Secondary;
                discard_props.interact =
                    Interact::from_state(None, self.hover, self.pressed, false);
                let (_, discard_id) = button::build(&mut tree, actions, &discard_props, p);
                self.id_secondary_btn = Some(discard_id);

                let mut keep_props = ButtonProps::new(strings::FLYOUT_KEEP_TIME);
                keep_props.variant = ButtonVariant::Primary;
                let (_, keep_id) = button::build(&mut tree, actions, &keep_props, p);
                self.id_primary_btn = Some(keep_id);
            }
            FlyoutMode::Timer => {
                // Header: status + live clock + "Open Focus"
                let hdr = tree.add_container(
                    root,
                    Style {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Row,
                        align_items: Some(AlignItems::Center),
                        justify_content: Some(JustifyContent::SpaceBetween),
                        ..Default::default()
                    },
                );
                let hdr_left = tree.add_container(
                    hdr,
                    Style {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Row,
                        align_items: Some(AlignItems::Center),
                        gap: style::gap(8.0),
                        ..Default::default()
                    },
                );
                if let Some(ref run) = self.running {
                    let elapsed = (now_ms - run.start).max(0);
                    add_label(
                        &mut tree,
                        hdr_left,
                        format::clock(elapsed),
                        16.0,
                        Weight::SemiBold,
                        Align::Left,
                        p.accent,
                    );
                } else {
                    add_label(
                        &mut tree,
                        hdr_left,
                        strings::FLYOUT_TITLE,
                        14.0,
                        Weight::SemiBold,
                        Align::Left,
                        p.text,
                    );
                }
                let mut open_props = ButtonProps::new(strings::FLYOUT_OPEN_APP);
                open_props.variant = ButtonVariant::Ghost;
                open_props.size = ButtonSize::Small;
                let (_, open_id) = button::build(&mut tree, hdr, &open_props, p);
                self.id_open_app = Some(open_id);

                // Description input
                let tf = TextFieldProps {
                    key: "flyout-desc",
                    aria_label: strings::FLYOUT_DESC_LABEL,
                    placeholder: strings::FLYOUT_DESC_PLACEHOLDER,
                    value: &self.description,
                    multiline: false,
                    min_height: 34.0,
                    focused: false,
                    error: false,
                    disabled: false,
                    sync_rev: self.desc_sync_rev,
                };
                text_field::build_tree_node(&mut tree, root, &tf, p);

                // Project picker button (expands inline)
                let (proj_name, proj_color) = self.selected_project_name_and_color();
                let picker_btn = tree.add_container(
                    root,
                    Style {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Row,
                        align_items: Some(AlignItems::Center),
                        justify_content: Some(JustifyContent::SpaceBetween),
                        padding: style::pad(10.0, 0.0, 10.0, 0.0),
                        size: Size {
                            width: percent(1.0_f32),
                            height: length(32.0_f32),
                        },
                        ..Default::default()
                    },
                );
                tree.set_paint(
                    picker_btn,
                    PaintStyle {
                        bg: Some(p.ctl_bg),
                        border_color: Some(if self.picker_open {
                            p.accent
                        } else {
                            p.ctl_border
                        }),
                        border_width: p.border_width,
                        radius: 6.0,
                        clip: false,
                    },
                );
                tree.set_hit(picker_btn, HitMode::Interactive);
                tree.set_focusable(picker_btn, true);
                let mut pk_a11y = interactive_a11y(
                    A11yRole::Button,
                    format!("{}: {}", strings::FLYOUT_PROJECT_LABEL, proj_name),
                    false,
                );
                pk_a11y.expanded = Some(self.picker_open);
                tree.set_a11y(picker_btn, pk_a11y);
                let left = tree.add_container(
                    picker_btn,
                    Style {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Row,
                        align_items: Some(AlignItems::Center),
                        gap: style::gap(8.0),
                        ..Default::default()
                    },
                );
                let dot = tree.add_leaf(
                    left,
                    Style {
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
                        bg: Some(proj_color),
                        radius: 4.0,
                        ..Default::default()
                    },
                );
                add_label(
                    &mut tree,
                    left,
                    proj_name,
                    12.5,
                    Weight::Medium,
                    Align::Left,
                    p.text,
                );
                let chev = if self.picker_open {
                    glyph::CHEVRON_UP
                } else {
                    glyph::CHEVRON_DOWN
                };
                add_label(
                    &mut tree,
                    picker_btn,
                    chev.to_string(),
                    11.0,
                    Weight::Regular,
                    Align::Right,
                    p.muted,
                );
                self.id_picker_toggle = Some(tree.id(picker_btn));

                if self.picker_open {
                    let list = tree.add_container(
                        root,
                        Style {
                            display: Display::Flex,
                            flex_direction: FlexDirection::Column,
                            gap: style::gap(2.0),
                            padding: style::pad(4.0, 4.0, 4.0, 4.0),
                            size: Size {
                                width: percent(1.0_f32),
                                height: auto(),
                            },
                            ..Default::default()
                        },
                    );
                    tree.set_paint(
                        list,
                        PaintStyle {
                            bg: Some(p.backdrop),
                            border_color: Some(p.card_border),
                            border_width: p.border_width,
                            radius: 6.0,
                            clip: true,
                        },
                    );

                    // Option: No project
                    let mut choices: Vec<(Option<String>, String, Color)> =
                        vec![(None, strings::FLYOUT_NO_PROJECT.to_string(), NO_PROJECT)];
                    for pr in self.projects.iter().take(5) {
                        let col = parse_hex_color(&pr.color).unwrap_or(NO_PROJECT);
                        choices.push((Some(pr.id.clone()), pr.name.clone(), col));
                    }
                    for (opt_id, opt_name, opt_col) in choices {
                        let selected = self.selected_project == opt_id;
                        let row = tree.add_container(
                            list,
                            Style {
                                display: Display::Flex,
                                flex_direction: FlexDirection::Row,
                                align_items: Some(AlignItems::Center),
                                gap: style::gap(8.0),
                                padding: style::pad(8.0, 0.0, 8.0, 0.0),
                                size: Size {
                                    width: percent(1.0_f32),
                                    height: length(28.0_f32),
                                },
                                ..Default::default()
                            },
                        );
                        tree.set_paint(
                            row,
                            PaintStyle {
                                bg: Some(if selected { p.accent_bg } else { p.surface }),
                                radius: 4.0,
                                ..Default::default()
                            },
                        );
                        tree.set_hit(row, HitMode::Interactive);
                        tree.set_focusable(row, true);
                        let mut a11y = interactive_a11y(A11yRole::Button, &opt_name, false);
                        a11y.selected = Some(selected);
                        tree.set_a11y(row, a11y);
                        let d = tree.add_leaf(
                            row,
                            Style {
                                size: Size {
                                    width: length(8.0_f32),
                                    height: length(8.0_f32),
                                },
                                ..Default::default()
                            },
                            Leaf::Box,
                        );
                        tree.set_paint(
                            d,
                            PaintStyle {
                                bg: Some(opt_col),
                                radius: 4.0,
                                ..Default::default()
                            },
                        );
                        add_label(
                            &mut tree,
                            row,
                            &opt_name,
                            12.0,
                            Weight::Regular,
                            Align::Left,
                            p.text,
                        );
                        self.id_project_rows.push((tree.id(row), opt_id));
                    }
                }

                let spacer = tree.add_container(
                    root,
                    Style {
                        flex_grow: 1.0,
                        ..Default::default()
                    },
                );
                let _ = spacer;

                let footer = tree.add_container(
                    root,
                    Style {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Row,
                        align_items: Some(AlignItems::Center),
                        justify_content: Some(JustifyContent::FlexEnd),
                        gap: style::gap(8.0),
                        ..Default::default()
                    },
                );
                if self.running.is_some() {
                    let mut upd_props = ButtonProps::new(strings::FLYOUT_SAVE_NOTE);
                    upd_props.variant = ButtonVariant::Secondary;
                    let (_, upd_id) = button::build(&mut tree, footer, &upd_props, p);
                    self.id_secondary_btn = Some(upd_id);

                    let mut stop_props = ButtonProps::new(strings::FLYOUT_STOP);
                    stop_props.variant = ButtonVariant::Danger;
                    stop_props.icon = Some(glyph::STOP);
                    let (_, stop_id) = button::build(&mut tree, footer, &stop_props, p);
                    self.id_primary_btn = Some(stop_id);
                } else {
                    let mut start_props = ButtonProps::new(strings::FLYOUT_START);
                    start_props.variant = ButtonVariant::Primary;
                    start_props.icon = Some(glyph::PLAY);
                    let (_, start_id) = button::build(&mut tree, footer, &start_props, p);
                    self.id_primary_btn = Some(start_id);
                }
            }
        }
        tree
    }

    fn rebuild(&mut self, tree: &mut Tree, focus: &mut FocusModel) {
        let now_ms = system_now_ms();
        *tree = self.build_tree(now_ms);
        let root_id = tree.id(tree.root());
        focus.sync_from_tree(tree, |_, _| crate::ui::focus::FocusScope::Popup(root_id));
    }

    fn start_or_update(&mut self, restart_if_running: bool) -> bool {
        let now_ms = system_now_ms();
        if let Some(ref run) = self.running {
            let patch = EntryPatch {
                project_id: Some(self.selected_project.clone()),
                description: Some(self.description.trim().to_string()),
                ..Default::default()
            };
            if self.db_handle.update_entry(run.id.clone(), patch).is_ok() {
                (self.on_data_changed)();
                return true;
            }
            return false;
        }
        if restart_if_running || self.running.is_none() {
            let billable = self
                .selected_project
                .as_ref()
                .and_then(|pid| self.projects.iter().find(|p| &p.id == pid))
                .is_none_or(|p| p.billable_default);
            let input = TimerStartInput {
                project_id: self.selected_project.clone(),
                description: self.description.trim().to_string(),
                billable,
            };
            if self.db_handle.start_timer(input, now_ms).is_ok() {
                self.reload_running();
                (self.on_data_changed)();
                return true;
            }
        }
        false
    }
}

struct FlyoutDelegate {
    state: FlyoutState,
}

impl PopupDelegate for FlyoutDelegate {
    fn tick_ms(&self) -> Option<u32> {
        match self.state.mode {
            FlyoutMode::Timer if self.state.running.is_some() => Some(1_000),
            _ => None,
        }
    }

    fn on_event(
        &mut self,
        event: PopupEvent,
        tree: &mut Tree,
        focus: &mut FocusModel,
    ) -> PopupOutcome {
        match event {
            PopupEvent::Closed => PopupOutcome::default(),
            PopupEvent::Tick => {
                if self.state.running.is_some() {
                    self.state.rebuild(tree, focus);
                    return PopupOutcome {
                        handled: true,
                        dirty: true,
                        ..Default::default()
                    };
                }
                PopupOutcome::default()
            }
            PopupEvent::TextChange(key, val) => {
                if key == "flyout-desc" {
                    self.state.description = val;
                    return PopupOutcome {
                        handled: true,
                        ..Default::default()
                    };
                }
                PopupOutcome::default()
            }
            PopupEvent::TextSubmit(key, val) => {
                if key == "flyout-desc" {
                    self.state.description = val;
                    if self.state.start_or_update(false) {
                        self.state.rebuild(tree, focus);
                        return PopupOutcome {
                            handled: true,
                            dirty: true,
                            close: true,
                            ..Default::default()
                        };
                    }
                }
                PopupOutcome::default()
            }
            PopupEvent::Click(id) => {
                if Some(id) == self.state.id_open_app {
                    (self.state.on_open_app)();
                    return PopupOutcome {
                        handled: true,
                        close: true,
                        ..Default::default()
                    };
                }
                if Some(id) == self.state.id_picker_toggle {
                    self.state.picker_open = !self.state.picker_open;
                    let h = self.state.desired_height_dip();
                    self.state.rebuild(tree, focus);
                    return PopupOutcome {
                        handled: true,
                        dirty: true,
                        resize_h_dip: Some(h),
                        ..Default::default()
                    };
                }
                if let Some((_, pid)) = self
                    .state
                    .id_project_rows
                    .iter()
                    .find(|(wid, _)| *wid == id)
                    .cloned()
                {
                    self.state.selected_project = pid;
                    self.state.picker_open = false;
                    let h = self.state.desired_height_dip();
                    self.state.rebuild(tree, focus);
                    return PopupOutcome {
                        handled: true,
                        dirty: true,
                        resize_h_dip: Some(h),
                        ..Default::default()
                    };
                }
                match self.state.mode.clone() {
                    FlyoutMode::AwayPrompt {
                        entry_id,
                        away_started_ms,
                        ..
                    } => {
                        if Some(id) == self.state.id_secondary_btn {
                            let patch = EntryPatch {
                                end: Some(Some(away_started_ms)),
                                ..Default::default()
                            };
                            let _ = self.state.db_handle.update_entry(entry_id, patch);
                            (self.state.on_data_changed)();
                        }
                        if Some(id) == self.state.id_primary_btn
                            || Some(id) == self.state.id_secondary_btn
                        {
                            return PopupOutcome {
                                handled: true,
                                close: true,
                                ..Default::default()
                            };
                        }
                    }
                    FlyoutMode::Timer => {
                        if Some(id) == self.state.id_primary_btn {
                            if self.state.running.is_some() {
                                if self.state.db_handle.stop_timers(system_now_ms()).is_ok() {
                                    (self.state.on_data_changed)();
                                }
                            } else {
                                self.state.start_or_update(true);
                            }
                            return PopupOutcome {
                                handled: true,
                                close: true,
                                ..Default::default()
                            };
                        }
                        if Some(id) == self.state.id_secondary_btn
                            && self.state.start_or_update(false)
                        {
                            return PopupOutcome {
                                handled: true,
                                close: true,
                                ..Default::default()
                            };
                        }
                    }
                }
                PopupOutcome::default()
            }
            PopupEvent::Key(ev) if ev.key == KeyCode::Enter && self.state.picker_open => {
                self.state.picker_open = false;
                let h = self.state.desired_height_dip();
                self.state.rebuild(tree, focus);
                PopupOutcome {
                    handled: true,
                    dirty: true,
                    resize_h_dip: Some(h),
                    ..Default::default()
                }
            }
            _ => PopupOutcome::default(),
        }
    }
}

pub fn is_open() -> bool {
    popup::is_open()
}

pub fn close() {
    popup::close();
}

pub fn open(
    anchor: Option<ScreenRect>,
    db_path: &Path,
    db_handle: DbHandle,
    mode: FlyoutMode,
    on_open_app: fn(),
    on_data_changed: fn(),
) -> Option<HWND> {
    let sys_prefs = SystemPrefs::query();
    let palette = Palette::for_system(sys_prefs.high_contrast);
    let mut state = FlyoutState::from_db(
        db_path.to_path_buf(),
        db_handle,
        mode,
        palette,
        on_open_app,
        on_data_changed,
    );
    let tree = state.build_tree(system_now_ms());
    let h = state.desired_height_dip();
    popup::open(
        None,
        anchor,
        (FLYOUT_WIDTH_DIP, h),
        PopupAlign::BelowEnd,
        tree,
        Box::new(FlyoutDelegate { state }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_thread::DbThread;
    use crate::ui::a11y;
    use crate::ui::text::TextSystem;

    fn noop() {}

    #[test]
    fn flyout_trees_pass_a11y_validation_and_layout() {
        let dir = std::env::temp_dir().join(format!("focus-flyout-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let db_path = dir.join("focus.db");
        let mut db_thread = DbThread::spawn(db_path.clone()).unwrap();
        let handle = db_thread.handle();
        let text = TextSystem::new().expect("dwrite");

        let mut timer_state = FlyoutState::from_db(
            db_path.clone(),
            handle.clone(),
            FlyoutMode::Timer,
            Palette::LIGHT,
            noop,
            noop,
        );
        timer_state.picker_open = true;
        let mut tree = timer_state.build_tree(100_000);
        tree.layout(FLYOUT_WIDTH_DIP, timer_state.desired_height_dip(), &text);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));

        let mut away_state = FlyoutState::from_db(
            db_path,
            handle,
            FlyoutMode::AwayPrompt {
                entry_id: "e1".into(),
                away_started_ms: 50_000,
                label: "Code review".into(),
                elapsed_away_ms: 600_000,
            },
            Palette::LIGHT,
            noop,
            noop,
        );
        let mut away_tree = away_state.build_tree(650_000);
        away_tree.layout(FLYOUT_WIDTH_DIP, away_state.desired_height_dip(), &text);
        assert_eq!(a11y::validate_tree_names(&away_tree), Ok(()));

        db_thread.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
