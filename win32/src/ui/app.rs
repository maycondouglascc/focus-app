//! Top-level application shell (`AppShell`) composing the Welcome gate,
//! persistent Sidebar (nav + live timer dot + "This week" SQL project totals +
//! Settings), global keyboard shortcuts (`Ctrl+1..4,,`, `Ctrl+N`, `Ctrl+Space`),
//! and active screen content.

use taffy::prelude::*;

use crate::db::SharedStore;
use crate::format::hm;
use crate::time::{add_days, start_of_week_with_first};
use crate::ui::focus::FocusModel;
use crate::ui::input::{KeyCode, KeyEvent};
use crate::ui::screen::{BuiltScreen, KeyModifiers, PointerReply, Screen, ScreenEnv};
use crate::ui::screens::activity::ActivityScreen;
use crate::ui::screens::calendar::CalendarScreen;
use crate::ui::screens::project_report::ProjectReportScreen;
use crate::ui::screens::projects::ProjectsScreen;
use crate::ui::screens::reports::ReportsScreen;
use crate::ui::screens::settings::SettingsScreen;
use crate::ui::screens::welcome::WelcomeScreen;
use crate::ui::strings;
use crate::ui::text::{Align, Weight};
use crate::ui::theme::{parse_hex_color, Palette, NO_PROJECT};
use crate::ui::tree::{style, A11yInfo, A11yRole, HitMode, Leaf, Node, PaintStyle, Tree, WidgetId};
use crate::ui::widgets::{add_label, glyph, interactive_a11y, Interact};

const SIDEBAR_W: f32 = 216.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Section {
    Calendar,
    Activity,
    Reports,
    Projects,
    Project(String),
    Settings,
}

impl Section {
    pub fn from_cli_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "calendar" | "timer" => Some(Self::Calendar),
            "activity" => Some(Self::Activity),
            "reports" => Some(Self::Reports),
            "projects" => Some(Self::Projects),
            "settings" => Some(Self::Settings),
            "project" | "project_report" => Some(Self::Project(String::new())),
            _ => None,
        }
    }
}

pub struct AppShell {
    pub store: SharedStore,
    pub section: Section,
    welcome_override: bool,

    welcome: WelcomeScreen,
    pub calendar: CalendarScreen,
    activity: ActivityScreen,
    reports: ReportsScreen,
    projects: ProjectsScreen,
    project_report: Option<ProjectReportScreen>,
    settings: SettingsScreen,

    id_nav_calendar: Option<WidgetId>,
    id_nav_activity: Option<WidgetId>,
    id_nav_reports: Option<WidgetId>,
    id_nav_projects: Option<WidgetId>,
    id_nav_settings: Option<WidgetId>,
    id_side_empty_proj: Option<WidgetId>,
    id_side_projects: Vec<(WidgetId, String)>,
}

impl AppShell {
    pub fn new(store: SharedStore) -> Self {
        let welcome = WelcomeScreen::new(std::rc::Rc::clone(&store));
        let calendar = CalendarScreen::new(std::rc::Rc::clone(&store));
        let activity = ActivityScreen::new(std::rc::Rc::clone(&store));
        let reports = ReportsScreen::new(std::rc::Rc::clone(&store));
        let projects = ProjectsScreen::new(std::rc::Rc::clone(&store));
        let settings = SettingsScreen::new(std::rc::Rc::clone(&store));
        Self {
            store,
            section: Section::Calendar,
            welcome_override: false,
            welcome,
            calendar,
            activity,
            reports,
            projects,
            project_report: None,
            settings,
            id_nav_calendar: None,
            id_nav_activity: None,
            id_nav_reports: None,
            id_nav_projects: None,
            id_nav_settings: None,
            id_side_empty_proj: None,
            id_side_projects: Vec::new(),
        }
    }

    pub fn navigate(&mut self, section: Section) {
        self.welcome_override = false;
        match section {
            Section::Project(pid) => {
                let resolved = if pid.is_empty() {
                    self.store
                        .borrow()
                        .list_projects()
                        .ok()
                        .and_then(|ps| ps.into_iter().next())
                        .map(|p| p.id)
                } else {
                    Some(pid)
                };
                if let Some(id) = resolved {
                    self.project_report = Some(ProjectReportScreen::new(
                        std::rc::Rc::clone(&self.store),
                        id.clone(),
                    ));
                    self.section = Section::Project(id);
                } else {
                    self.section = Section::Projects;
                }
            }
            other => {
                self.section = other;
            }
        }
    }

    pub fn force_welcome(&mut self) {
        self.welcome_override = true;
    }

    fn is_onboarded(&self) -> bool {
        if self.welcome_override {
            return false;
        }
        self.store
            .borrow()
            .get_settings()
            .map(|s| s.onboarded)
            .unwrap_or(true)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_nav_row(
        tree: &mut Tree,
        parent: Node,
        icon: char,
        label: &str,
        selected: bool,
        live_dot: bool,
        prev_id: Option<WidgetId>,
        env: &ScreenEnv<'_>,
    ) -> WidgetId {
        let p = env.palette;
        let row = tree.add_container(
            parent,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::SpaceBetween),
                padding: style::pad(10.0, 6.0, 10.0, 6.0),
                gap: style::gap(8.0),
                size: Size {
                    width: percent(1.0_f32),
                    height: length(32.0_f32),
                },
                ..Default::default()
            },
        );
        let id = tree.id(row);
        let interact = Interact::from_state(prev_id, env.hover, env.pressed, false);
        let bg = if selected {
            Some(p.accent_bg)
        } else if interact.pressed {
            Some(p.ctl_pressed)
        } else if interact.hovered {
            Some(p.ctl_hover)
        } else {
            None
        };
        if let Some(bg) = bg {
            tree.set_paint(
                row,
                PaintStyle {
                    bg: Some(bg),
                    border_color: None,
                    border_width: 0.0,
                    radius: 6.0,
                    clip: false,
                },
            );
        }

        let left = tree.add_container(
            row,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                gap: style::gap(8.0),
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                ..Default::default()
            },
        );
        tree.add_leaf(
            left,
            Style::default(),
            Leaf::Icon {
                glyph: icon,
                size: 14.0,
                color: if selected { p.accent } else { p.muted },
            },
        );
        add_label(
            tree,
            left,
            label,
            13.0,
            if selected {
                Weight::SemiBold
            } else {
                Weight::Medium
            },
            Align::Left,
            if selected { p.accent } else { p.text },
        );
        if live_dot {
            let dot = tree.add_container(
                row,
                Style {
                    size: style::size(7.0, 7.0),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
            tree.set_paint(
                dot,
                PaintStyle {
                    bg: Some(p.done),
                    border_color: None,
                    border_width: 0.0,
                    radius: 3.5,
                    clip: false,
                },
            );
        }

        tree.set_hit(row, HitMode::Interactive);
        tree.set_focusable(row, true);
        let mut info = interactive_a11y(A11yRole::Button, label, false);
        info.selected = Some(selected);
        tree.set_a11y(row, info);
        id
    }

    fn build_sidebar(&mut self, tree: &mut Tree, root: Node, env: &ScreenEnv<'_>) {
        let p = env.palette;
        let (is_demo, is_running, active_projects, week_totals) = {
            let st = self.store.borrow();
            let demo = st.is_demo();
            let running = st.running_entry().ok().flatten().is_some();
            let now = st.now_ms();
            let loc = st.system_locale().clone();
            let first_day = st
                .get_settings()
                .map(|s| s.first_day_of_week.resolve_mon0(&loc))
                .unwrap_or(loc.first_day_mon0);
            let w_start = start_of_week_with_first(now, first_day);
            let w_end = add_days(w_start, 7);
            let projs: Vec<_> = st
                .list_projects()
                .unwrap_or_default()
                .into_iter()
                .filter(|pr| !pr.archived)
                .collect();
            let sums = st.project_summaries(w_start, w_end).unwrap_or_default();
            (demo, running, projs, sums)
        };

        let side = tree.add_container(
            root,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                padding: style::pad(12.0, 14.0, 12.0, 12.0),
                gap: style::gap(12.0),
                size: Size {
                    width: length(SIDEBAR_W),
                    height: percent(1.0_f32),
                },
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        tree.set_paint(
            side,
            PaintStyle {
                bg: Some(p.title_bar),
                border_color: Some(p.hairline),
                border_width: p.border_width,
                radius: 0.0,
                clip: true,
            },
        );
        tree.set_a11y(
            side,
            A11yInfo {
                role: A11yRole::Group,
                name: strings::SIDEBAR_NAV_LABEL.into(),
                requires_name: true,
                ..Default::default()
            },
        );

        // Brand + optional Demo badge
        let brand_row = tree.add_container(
            side,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::SpaceBetween),
                padding: style::pad(8.0, 4.0, 8.0, 4.0),
                ..Default::default()
            },
        );
        add_label(
            tree,
            brand_row,
            strings::APP_NAME,
            15.0,
            Weight::SemiBold,
            Align::Left,
            p.text,
        );
        if is_demo {
            let badge = tree.add_container(
                brand_row,
                Style {
                    padding: style::pad(6.0, 2.0, 6.0, 2.0),
                    ..Default::default()
                },
            );
            tree.set_paint(
                badge,
                PaintStyle {
                    bg: Some(p.accent_bg),
                    border_color: None,
                    border_width: 0.0,
                    radius: 4.0,
                    clip: false,
                },
            );
            add_label(
                tree,
                badge,
                strings::SIDEBAR_DEMO_BADGE,
                10.5,
                Weight::Medium,
                Align::Center,
                p.accent,
            );
        }

        // Primary nav items
        let nav_box = tree.add_container(
            side,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                gap: style::gap(2.0),
                ..Default::default()
            },
        );
        self.id_nav_calendar = Some(Self::build_nav_row(
            tree,
            nav_box,
            glyph::CALENDAR,
            strings::NAV_CALENDAR,
            self.section == Section::Calendar,
            is_running,
            self.id_nav_calendar,
            env,
        ));
        self.id_nav_activity = Some(Self::build_nav_row(
            tree,
            nav_box,
            glyph::ACTIVITY,
            strings::NAV_ACTIVITY,
            self.section == Section::Activity,
            false,
            self.id_nav_activity,
            env,
        ));
        self.id_nav_reports = Some(Self::build_nav_row(
            tree,
            nav_box,
            glyph::REPORTS,
            strings::NAV_REPORTS,
            self.section == Section::Reports,
            false,
            self.id_nav_reports,
            env,
        ));
        self.id_nav_projects = Some(Self::build_nav_row(
            tree,
            nav_box,
            glyph::PROJECTS,
            strings::NAV_PROJECTS,
            self.section == Section::Projects,
            false,
            self.id_nav_projects,
            env,
        ));

        // "This week" project quick list
        let week_head = tree.add_container(
            side,
            Style {
                padding: style::pad(10.0, 6.0, 4.0, 0.0),
                ..Default::default()
            },
        );
        add_label(
            tree,
            week_head,
            strings::SIDEBAR_THIS_WEEK,
            11.0,
            Weight::SemiBold,
            Align::Left,
            p.muted,
        );

        let proj_list = tree.add_container(
            side,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                gap: style::gap(2.0),
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                ..Default::default()
            },
        );
        self.id_side_projects.clear();
        self.id_side_empty_proj = None;
        if active_projects.is_empty() {
            let empty_row = tree.add_container(
                proj_list,
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    align_items: Some(AlignItems::Center),
                    padding: style::pad(10.0, 6.0, 10.0, 6.0),
                    gap: style::gap(6.0),
                    ..Default::default()
                },
            );
            tree.set_hit(empty_row, HitMode::Interactive);
            tree.set_focusable(empty_row, true);
            tree.set_a11y(
                empty_row,
                interactive_a11y(A11yRole::Button, "Create a project", false),
            );
            tree.add_leaf(
                empty_row,
                Style::default(),
                Leaf::Icon {
                    glyph: glyph::ADD,
                    size: 12.0,
                    color: p.muted,
                },
            );
            add_label(
                tree,
                empty_row,
                "Create a project",
                12.0,
                Weight::Regular,
                Align::Left,
                p.muted,
            );
            self.id_side_empty_proj = Some(tree.id(empty_row));
        } else {
            for pr in active_projects.iter().take(12) {
                let is_sel = matches!(&self.section, Section::Project(cur) if cur == &pr.id);
                let prev_id = self
                    .id_side_projects
                    .iter()
                    .find(|(_, pid)| pid == &pr.id)
                    .map(|(id, _)| *id);
                let row = tree.add_container(
                    proj_list,
                    Style {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Row,
                        align_items: Some(AlignItems::Center),
                        justify_content: Some(JustifyContent::SpaceBetween),
                        padding: style::pad(10.0, 5.0, 10.0, 5.0),
                        gap: style::gap(8.0),
                        size: Size {
                            width: percent(1.0_f32),
                            height: length(28.0_f32),
                        },
                        ..Default::default()
                    },
                );
                let rid = tree.id(row);
                let interact = Interact::from_state(prev_id, env.hover, env.pressed, false);
                let bg = if is_sel {
                    Some(p.accent_bg)
                } else if interact.hovered {
                    Some(p.ctl_hover)
                } else {
                    None
                };
                if let Some(bg) = bg {
                    tree.set_paint(
                        row,
                        PaintStyle {
                            bg: Some(bg),
                            border_color: None,
                            border_width: 0.0,
                            radius: 6.0,
                            clip: false,
                        },
                    );
                }
                let left = tree.add_container(
                    row,
                    Style {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Row,
                        align_items: Some(AlignItems::Center),
                        gap: style::gap(8.0),
                        flex_grow: 1.0,
                        flex_basis: length(0.0_f32),
                        ..Default::default()
                    },
                );
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
                        bg: Some(parse_hex_color(&pr.color).unwrap_or(NO_PROJECT)),
                        border_color: None,
                        border_width: 0.0,
                        radius: 4.0,
                        clip: false,
                    },
                );
                add_label(
                    tree,
                    left,
                    &pr.name,
                    12.5,
                    if is_sel {
                        Weight::SemiBold
                    } else {
                        Weight::Regular
                    },
                    Align::Left,
                    if is_sel { p.accent } else { p.text },
                );
                let ms = week_totals.get(&pr.id).map(|s| s.month_ms).unwrap_or(0);
                let aria = if ms > 0 {
                    let h = hm(ms);
                    add_label(tree, row, &h, 11.5, Weight::Regular, Align::Right, p.muted);
                    format!("{}, {}", pr.name, h)
                } else {
                    pr.name.clone()
                };
                tree.set_hit(row, HitMode::Interactive);
                tree.set_focusable(row, true);
                let mut info = interactive_a11y(A11yRole::Button, aria, false);
                info.selected = Some(is_sel);
                tree.set_a11y(row, info);
                self.id_side_projects.push((rid, pr.id.clone()));
            }
        }

        // Bottom Settings button
        self.id_nav_settings = Some(Self::build_nav_row(
            tree,
            side,
            glyph::SETTINGS,
            strings::NAV_SETTINGS,
            self.section == Section::Settings,
            false,
            self.id_nav_settings,
            env,
        ));
    }

    fn sync_transitions(&mut self) {
        if let Some(pid) = self.projects.opened_project.take() {
            self.navigate(Section::Project(pid));
        }
        if self
            .project_report
            .as_ref()
            .is_some_and(|r| r.back_requested)
        {
            self.project_report = None;
            self.section = Section::Projects;
        }
        if self.calendar.new_project_requested {
            self.calendar.new_project_requested = false;
            self.navigate(Section::Projects);
            self.projects.open_new_dialog();
        }
        if self.reports.go_timer_requested {
            self.reports.go_timer_requested = false;
            self.navigate(Section::Calendar);
        }
    }
}

impl Screen for AppShell {
    fn build(&mut self, env: &ScreenEnv<'_>) -> BuiltScreen {
        let top_bar = crate::ui::frame::TITLE_BAR_DIP;
        let body_h = (env.height_dip - top_bar).max(200.0);
        let mut tree = Tree::new(Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            padding: style::pad(0.0, top_bar, 0.0, 0.0),
            size: style::size(env.width_dip, env.height_dip),
            ..Default::default()
        });
        let root = tree.root();

        if !self.is_onboarded() {
            let body = tree.add_container(
                root,
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Column,
                    flex_grow: 1.0,
                    size: Size {
                        width: percent(1.0_f32),
                        height: length(body_h),
                    },
                    ..Default::default()
                },
            );
            tree.set_paint(
                body,
                PaintStyle {
                    bg: Some(env.palette.backdrop),
                    border_color: None,
                    border_width: 0.0,
                    radius: 0.0,
                    clip: true,
                },
            );
            let sub_env = ScreenEnv {
                height_dip: body_h,
                ..*env
            };
            self.welcome.build_content(&mut tree, body, &sub_env);
            return BuiltScreen {
                tree,
                modal_root: None,
            };
        }
        self.sync_transitions();

        let body = tree.add_container(
            root,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                flex_grow: 1.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: length(body_h),
                },
                ..Default::default()
            },
        );
        tree.set_paint(
            body,
            PaintStyle {
                bg: Some(env.palette.backdrop),
                border_color: None,
                border_width: 0.0,
                radius: 0.0,
                clip: true,
            },
        );

        let body_env = ScreenEnv {
            height_dip: body_h,
            ..*env
        };
        self.build_sidebar(&mut tree, body, &body_env);

        let content_w = (env.width_dip - SIDEBAR_W).max(320.0);
        let main = tree.add_container(
            body,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                size: Size {
                    width: length(content_w),
                    height: percent(1.0_f32),
                },
                min_size: Size {
                    width: length(0.0_f32),
                    height: length(0.0_f32),
                },
                ..Default::default()
            },
        );
        let sub_env = ScreenEnv {
            width_dip: content_w,
            height_dip: body_h,
            ..*env
        };

        let modal_root = match self.section {
            Section::Calendar => self.calendar.build_content(&mut tree, body, main, &sub_env),
            Section::Activity => self.activity.build_content(&mut tree, body, main, &sub_env),
            Section::Reports => self.reports.build_content(&mut tree, body, main, &sub_env),
            Section::Projects => self.projects.build_content(&mut tree, body, main, &sub_env),
            Section::Project(_) => match self.project_report.as_mut() {
                Some(pr) => pr.build_content(&mut tree, body, main, &sub_env),
                None => self.projects.build_content(&mut tree, body, main, &sub_env),
            },
            Section::Settings => self.settings.build_content(&mut tree, body, main, &sub_env),
        };

        BuiltScreen { tree, modal_root }
    }

    fn on_click(&mut self, id: WidgetId) -> bool {
        if !self.is_onboarded() {
            let handled = self.welcome.on_click(id);
            if self
                .store
                .borrow()
                .get_settings()
                .is_ok_and(|s| s.onboarded)
            {
                self.welcome_override = false;
            }
            return handled;
        }
        if Some(id) == self.id_nav_calendar {
            self.navigate(Section::Calendar);
            return true;
        }
        if Some(id) == self.id_nav_activity {
            self.navigate(Section::Activity);
            return true;
        }
        if Some(id) == self.id_nav_reports {
            self.navigate(Section::Reports);
            return true;
        }
        if Some(id) == self.id_nav_projects {
            self.navigate(Section::Projects);
            return true;
        }
        if Some(id) == self.id_nav_settings {
            self.navigate(Section::Settings);
            return true;
        }
        if Some(id) == self.id_side_empty_proj {
            self.navigate(Section::Projects);
            self.projects.open_new_dialog();
            return true;
        }
        if let Some((_, pid)) = self.id_side_projects.iter().find(|(wid, _)| *wid == id) {
            let pid = pid.clone();
            self.navigate(Section::Project(pid));
            return true;
        }

        let handled = match self.section {
            Section::Calendar => self.calendar.handle_click(id, &Palette::LIGHT),
            Section::Activity => self.activity.handle_click(id, &Palette::LIGHT),
            Section::Reports => self.reports.handle_click(id, &Palette::LIGHT),
            Section::Projects => self.projects.handle_click(id, &Palette::LIGHT),
            Section::Project(_) => self
                .project_report
                .as_mut()
                .is_some_and(|pr| pr.handle_click(id, &Palette::LIGHT)),
            Section::Settings => self.settings.handle_click(id, &Palette::LIGHT),
        };
        self.sync_transitions();
        handled
    }

    fn on_key(&mut self, ev: KeyEvent, focus: &mut FocusModel) -> bool {
        if !self.is_onboarded() {
            return self.welcome.on_key(ev, focus);
        }

        // Global shortcuts (Ctrl+1..4,, | Ctrl+N | Ctrl+Space)
        if ev.ctrl && !ev.alt && !ev.shift {
            match ev.key {
                KeyCode::Space => {
                    self.calendar.timer.toggle_start_stop(&self.store);
                    return true;
                }
                KeyCode::Other(0x31) => {
                    self.navigate(Section::Calendar);
                    return true;
                }
                KeyCode::Other(0x32) => {
                    self.navigate(Section::Activity);
                    return true;
                }
                KeyCode::Other(0x33) => {
                    self.navigate(Section::Reports);
                    return true;
                }
                KeyCode::Other(0x34) => {
                    self.navigate(Section::Projects);
                    return true;
                }
                KeyCode::Other(0xBC) => {
                    // VK_OEM_COMMA -> Ctrl+,
                    self.navigate(Section::Settings);
                    return true;
                }
                KeyCode::Other(0x4E) => {
                    // VK_N -> Ctrl+N
                    if self.section == Section::Projects {
                        self.projects.open_new_dialog();
                    } else {
                        self.navigate(Section::Calendar);
                        self.calendar.open_new_entry_at_now();
                    }
                    return true;
                }
                _ => {}
            }
        }

        match self.section {
            Section::Calendar => self.calendar.on_key(ev, focus),
            Section::Projects => self.projects.on_key(ev, focus),
            Section::Project(_) => self
                .project_report
                .as_mut()
                .is_some_and(|pr| pr.on_key(ev, focus)),
            Section::Reports => self.reports.on_key(ev, focus),
            _ => false,
        }
    }

    fn on_wheel(&mut self, delta_dip: f32) -> bool {
        self.on_wheel_at(-1.0, -1.0, 0.0, delta_dip)
    }

    fn on_wheel_at(&mut self, x_dip: f32, y_dip: f32, dx_dip: f32, dy_dip: f32) -> bool {
        if !self.is_onboarded() {
            return self.welcome.on_wheel(dy_dip);
        }
        match self.section {
            Section::Calendar => self.calendar.on_wheel_at(x_dip, y_dip, dx_dip, dy_dip),
            Section::Activity => self.activity.on_wheel(dy_dip),
            Section::Reports => self.reports.on_wheel_at(x_dip, y_dip, dx_dip, dy_dip),
            Section::Projects => self.projects.on_wheel(dy_dip),
            Section::Project(_) => self
                .project_report
                .as_mut()
                .is_some_and(|pr| pr.on_wheel_at(x_dip, y_dip, dx_dip, dy_dip)),
            Section::Settings => self.settings.on_wheel(dy_dip),
        }
    }

    fn on_escape(&mut self) -> bool {
        if !self.is_onboarded() {
            return false;
        }
        match self.section {
            Section::Calendar => self.calendar.on_escape(),
            Section::Activity => self.activity.on_escape(),
            Section::Reports => self.reports.on_escape(),
            Section::Projects => self.projects.on_escape(),
            Section::Project(_) => self
                .project_report
                .as_mut()
                .is_some_and(|pr| pr.on_escape()),
            Section::Settings => self.settings.on_escape(),
        }
    }

    fn on_text_change(&mut self, key: &str, value: &str) -> bool {
        if !self.is_onboarded() {
            return self.welcome.on_text_change(key, value);
        }
        match self.section {
            Section::Calendar => self.calendar.on_text_change(key, value),
            Section::Reports => self.reports.on_text_change(key, value),
            Section::Projects => self.projects.on_text_change(key, value),
            Section::Project(_) => self
                .project_report
                .as_mut()
                .is_some_and(|pr| pr.on_text_change(key, value)),
            Section::Settings => self.settings.on_text_change(key, value),
            _ => false,
        }
    }

    fn on_text_submit(&mut self, key: &str, value: &str) -> bool {
        if !self.is_onboarded() {
            return self.welcome.on_text_submit(key, value);
        }
        match self.section {
            Section::Calendar => self.calendar.on_text_submit(key, value),
            Section::Reports => self.reports.on_text_submit(key, value),
            Section::Projects => self.projects.on_text_submit(key, value),
            Section::Project(_) => self
                .project_report
                .as_mut()
                .is_some_and(|pr| pr.on_text_submit(key, value)),
            Section::Settings => self.settings.on_text_submit(key, value),
            _ => false,
        }
    }

    fn on_pointer_down(
        &mut self,
        id: WidgetId,
        x_dip: f32,
        y_dip: f32,
        mods: KeyModifiers,
    ) -> PointerReply {
        if self.is_onboarded() && self.section == Section::Calendar {
            self.calendar.on_pointer_down(id, x_dip, y_dip, mods)
        } else {
            PointerReply::default()
        }
    }

    fn on_pointer_move(&mut self, x_dip: f32, y_dip: f32, mods: KeyModifiers) -> PointerReply {
        if self.is_onboarded() && self.section == Section::Calendar {
            self.calendar.on_pointer_move(x_dip, y_dip, mods)
        } else {
            PointerReply::default()
        }
    }

    fn on_pointer_up(&mut self, x_dip: f32, y_dip: f32, mods: KeyModifiers) -> PointerReply {
        if self.is_onboarded() && self.section == Section::Calendar {
            self.calendar.on_pointer_up(x_dip, y_dip, mods)
        } else {
            PointerReply::default()
        }
    }

    fn on_pointer_cancel(&mut self) -> bool {
        if self.is_onboarded() && self.section == Section::Calendar {
            self.calendar.on_pointer_cancel()
        } else {
            false
        }
    }

    fn sync_drag_tree(&mut self, tree: &mut Tree) {
        if self.is_onboarded() && self.section == Section::Calendar {
            self.calendar.sync_drag_tree(tree);
        }
    }

    fn wants_tick(&self) -> bool {
        self.is_onboarded() && self.section == Section::Calendar && self.calendar.wants_tick()
    }

    fn on_tick(&mut self) -> bool {
        self.is_onboarded() && self.section == Section::Calendar && self.calendar.handle_tick()
    }

    fn wants_autoscroll(&self) -> bool {
        self.is_onboarded() && self.section == Section::Calendar && self.calendar.wants_autoscroll()
    }

    fn on_autoscroll(&mut self) -> PointerReply {
        if self.is_onboarded() && self.section == Section::Calendar {
            self.calendar.on_autoscroll()
        } else {
            PointerReply::default()
        }
    }

    fn on_popup_poll(&mut self) -> bool {
        if !self.is_onboarded() {
            return false;
        }
        match self.section {
            Section::Calendar => self.calendar.on_popup_poll(),
            Section::Activity => self.activity.on_popup_poll(),
            Section::Reports => self.reports.on_popup_poll(),
            Section::Projects => self.projects.on_popup_poll(),
            Section::Project(_) => self
                .project_report
                .as_mut()
                .is_some_and(|pr| pr.on_popup_poll()),
            Section::Settings => self.settings.on_popup_poll(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::shared_store;
    use crate::demo::DemoStore;
    use crate::locale::SystemLocale;
    use crate::ui::a11y;
    use crate::ui::focus::FocusModel;
    use crate::ui::screen::ScreenHost;
    use crate::ui::text::TextSystem;
    use std::rc::Rc;

    #[test]
    fn app_shell_validates_every_screen_and_keyboard_shortcuts() {
        let text = TextSystem::new().expect("dwrite");
        let palette = Palette::LIGHT;
        let env = ScreenEnv {
            width_dip: 1280.0,
            height_dip: 820.0,
            hover: None,
            pressed: None,
            focused: None,
            palette: &palette,
            text: &text,
        };
        let store = shared_store(Box::new(DemoStore::new(SystemLocale::en_us())));
        let shell = AppShell::new(Rc::clone(&store));
        let mut focus = FocusModel::new();
        let mut host = ScreenHost::mount(Box::new(shell), &env, &mut focus);
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));

        fn assert_bounds(tree: &Tree) {
            for n in tree.all_nodes() {
                if let Some(d) = tree.data(n) {
                    if d.focusable && !d.hidden {
                        let check_node = if d.a11y.name == crate::ui::strings::CAL_GRID_LABEL {
                            tree.parent(n).unwrap_or(n)
                        } else {
                            n
                        };
                        let r = tree.rect(check_node);
                        assert!(
                            r.y >= crate::ui::frame::TITLE_BAR_DIP - 0.5,
                            "widget '{}' overlapped titlebar at y={}",
                            d.a11y.name,
                            r.y
                        );
                        assert!(
                            r.w > 0.0 && r.h > 0.0,
                            "widget '{}' collapsed to zero size: {:?}",
                            d.a11y.name,
                            r
                        );
                    }
                }
            }
        }
        assert_bounds(host.tree());

        // Navigate across all sections via Ctrl+2, Ctrl+3, Ctrl+4, Ctrl+,, Ctrl+1, Ctrl+Space.
        for vk in [0x32_u16, 0x33, 0x34, 0xBC, 0x31] {
            let (handled, dirty) = host.on_key(
                KeyEvent {
                    key: KeyCode::Other(vk),
                    shift: false,
                    ctrl: true,
                    alt: false,
                },
                &mut focus,
            );
            assert!(handled && dirty);
            assert!(host.rebuild_if_dirty(&env, &mut focus));
            assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));
            assert_bounds(host.tree());
        }

        // Ctrl+Space toggles running timer cleanly.
        let was_running = store.borrow().running_entry().unwrap().is_some();
        let (h, d) = host.on_key(
            KeyEvent {
                key: KeyCode::Space,
                shift: false,
                ctrl: true,
                alt: false,
            },
            &mut focus,
        );
        assert!(h && d);
        assert_ne!(
            store.borrow().running_entry().unwrap().is_some(),
            was_running
        );
    }
}
