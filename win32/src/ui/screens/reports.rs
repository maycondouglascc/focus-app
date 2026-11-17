//! Reports screen: project filter, PeriodNav, CSV export modal (`IFileSaveDialog`
//! with Excel locale `;`/`,`/BOM or Standard `,`/`.`), 4 KPI cards, stacked bars,
//! `ProportionBar`, and virtualized `EntryTree` opening `EntryEditor`.

use std::collections::{HashMap, HashSet};

use taffy::prelude::*;

use crate::csv_export::{format_entries_csv, pick_csv_save_path, write_csv_file, CsvDialect};
use crate::db::{Project, SharedStore, TimeEntry};
use crate::format::{duration, money};
use crate::stats::{earned, earnings_by_currency, entry_ms};
use crate::time::{start_of_day, HOUR};
use crate::ui::input::KeyCode;
use crate::ui::period::{fit_period, period_title, PeriodKind, PeriodNavState};
use crate::ui::screen::{BuiltScreen, Screen, ScreenEnv};
use crate::ui::strings;
use crate::ui::text::{Align, Weight, Wrap};
use crate::ui::theme::{parse_hex_color, Color, Palette, NO_PROJECT};
use crate::ui::tree::{style, Leaf, Node, Tree, WidgetId};
use crate::ui::widgets::button::{self, ButtonProps, ButtonVariant};
use crate::ui::widgets::charts;
use crate::ui::widgets::dialog;
use crate::ui::widgets::segmented::{self, SegmentedProps};
use crate::ui::widgets::select::{self, SelectProps};
use crate::ui::widgets::{add_label, glyph, Interact};
use crate::ui::window;

use super::entry_editor::{EntryDraft, EntryEditorOutcome, EntryEditorState};
use super::entry_tree::EntryTreeState;
use super::project_report::ChartMetric;
use super::{
    build_card, build_scroll_page, build_stat_card, screen_root_tree, scroll_wheel_step,
    PopupSelectSlot,
};

const NONE_KEY: &str = "__none";

#[derive(Clone, PartialEq, Eq)]
enum ProjectFilter {
    All,
    Project(String),
    NoneOnly,
}

struct CsvExportDialog {
    standard: bool,
    status_err: Option<String>,
    id_close: Option<WidgetId>,
    id_dialect_ids: Vec<WidgetId>,
    id_cancel: Option<WidgetId>,
    id_confirm: Option<WidgetId>,
}

pub struct ReportsScreen {
    store: SharedStore,
    period_nav: PeriodNavState,
    filter: ProjectFilter,
    metric: ChartMetric,
    scroll_y: f32,
    viewport_h: f32,
    entry_tree: EntryTreeState,
    entry_editor: Option<EntryEditorState>,
    csv_dialog: Option<CsvExportDialog>,
    export_notice: Option<String>,

    sel_filter: PopupSelectSlot,
    id_export_btn: Option<WidgetId>,
    id_start_timer_btn: Option<WidgetId>,
    id_metric_ids: Vec<WidgetId>,

    cached_projects: Vec<Project>,
    cached_entries: Vec<TimeEntry>,
    cached_period_title: String,
    pub go_timer_requested: bool,
}

impl ReportsScreen {
    pub fn new(store: SharedStore) -> Self {
        let now_ms = store.borrow().now_ms();
        Self {
            store,
            period_nav: PeriodNavState::new(PeriodKind::Month, now_ms),
            filter: ProjectFilter::All,
            metric: ChartMetric::Hours,
            scroll_y: 0.0,
            viewport_h: 720.0,
            entry_tree: EntryTreeState::new(),
            entry_editor: None,
            csv_dialog: None,
            export_notice: None,
            sel_filter: PopupSelectSlot::default(),
            id_export_btn: None,
            id_start_timer_btn: None,
            id_metric_ids: Vec::new(),
            cached_projects: Vec::new(),
            cached_entries: Vec::new(),
            cached_period_title: String::new(),
            go_timer_requested: false,
        }
    }

    fn filter_options(projects: &[Project]) -> Vec<(ProjectFilter, String)> {
        let mut out = Vec::with_capacity(projects.len() + 2);
        out.push((ProjectFilter::All, strings::REP_ALL_PROJECTS.to_string()));
        for pr in projects {
            out.push((ProjectFilter::Project(pr.id.clone()), pr.name.clone()));
        }
        out.push((ProjectFilter::NoneOnly, strings::REP_NO_PROJECT.to_string()));
        out
    }

    fn run_csv_export(&mut self, standard: bool) -> Result<bool, String> {
        let Some(hwnd) = window::hwnd() else {
            return Err("Window unavailable".into());
        };
        let clean_title: String = self
            .cached_period_title
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        let default_name = format!("focus-{}.csv", clean_title.trim_matches('-'));
        let Some(path) = pick_csv_save_path(hwnd, &default_name) else {
            return Ok(false);
        };
        let bytes = {
            let st = self.store.borrow();
            let loc = st.system_locale();
            let settings = st
                .get_settings()
                .unwrap_or_else(|_| crate::db::Settings::defaults_for(loc));
            let is_24h = settings.time_format.is_24h(loc);
            let now_ms = st.now_ms();
            let by_id: HashMap<&str, &Project> = self
                .cached_projects
                .iter()
                .map(|p| (p.id.as_str(), p))
                .collect();
            let dialect = CsvDialect::from_locale(loc, standard);
            format_entries_csv(&self.cached_entries, &by_id, now_ms, is_24h, dialect)
        };
        write_csv_file(&path, &bytes).map_err(|e| e.to_string())?;
        self.export_notice = Some(format!("Exported {} rows", self.cached_entries.len()));
        Ok(true)
    }

    pub fn build_content(
        &mut self,
        tree: &mut Tree,
        overlay_root: Node,
        parent: Node,
        env: &ScreenEnv<'_>,
    ) -> Option<Node> {
        self.viewport_h = env.height_dip;
        let p = env.palette;

        let (
            now_ms,
            first_day_mon0,
            is_24h,
            default_currency,
            has_any,
            projects,
            period,
            entries,
            active_ms,
        ) = {
            let st = self.store.borrow();
            let now_ms = st.now_ms();
            let loc = st.system_locale().clone();
            let settings = st
                .get_settings()
                .unwrap_or_else(|_| crate::db::Settings::defaults_for(&loc));
            let first_day_mon0 = settings.first_day_of_week.resolve_mon0(&loc);
            let is_24h = settings.time_format.is_24h(&loc);
            let has_any = st.has_any_entries().unwrap_or(false);
            let projects = st.list_projects().unwrap_or_default();

            let raw_p = self.period_nav.period(now_ms, first_day_mon0);
            let earliest = if raw_p.kind == PeriodKind::All {
                match &self.filter {
                    ProjectFilter::All => st.earliest_entry_start(None, false).ok().flatten(),
                    ProjectFilter::Project(pid) => {
                        st.earliest_entry_start(Some(pid), false).ok().flatten()
                    }
                    ProjectFilter::NoneOnly => st.earliest_entry_start(None, true).ok().flatten(),
                }
            } else {
                None
            };
            let period = fit_period(raw_p, earliest, now_ms);

            let all_entries = st.list_entries(period.from, period.to).unwrap_or_default();
            let entries: Vec<TimeEntry> = match &self.filter {
                ProjectFilter::All => all_entries,
                ProjectFilter::Project(pid) => all_entries
                    .into_iter()
                    .filter(|e| e.project_id.as_deref() == Some(pid.as_str()))
                    .collect(),
                ProjectFilter::NoneOnly => all_entries
                    .into_iter()
                    .filter(|e| e.project_id.is_none())
                    .collect(),
            };
            let active_ms = st.active_union_ms(period.from, period.to).unwrap_or(0);
            (
                now_ms,
                first_day_mon0,
                is_24h,
                settings.currency,
                has_any,
                projects,
                period,
                entries,
                active_ms,
            )
        };
        self.cached_projects = projects.clone();
        self.cached_entries = entries.clone();
        self.cached_period_title = period_title(&period, now_ms);

        let projects_by_id: HashMap<String, Project> = projects
            .iter()
            .map(|pr| (pr.id.clone(), pr.clone()))
            .collect();
        let proj_refs: HashMap<&str, &Project> =
            projects.iter().map(|pr| (pr.id.as_str(), pr)).collect();

        let total_ms: i64 = entries
            .iter()
            .map(|e| entry_ms(e, period.from, period.to, now_ms))
            .sum();
        let billable_ms: i64 = entries
            .iter()
            .filter(|e| e.billable)
            .map(|e| entry_ms(e, period.from, period.to, now_ms))
            .sum();
        let earned_list =
            earnings_by_currency(&entries, &proj_refs, period.from, period.to, now_ms);
        let unique_days: HashSet<i64> = entries.iter().map(|e| start_of_day(e.start)).collect();

        let chart_currency = earned_list
            .iter()
            .find(|(c, _)| *c == default_currency)
            .or_else(|| earned_list.first())
            .map(|(c, _)| c.clone())
            .unwrap_or(default_currency);
        let mixed = earned_list.len() > 1;

        // Per-project breakdown for proportion bar + stacked chart
        let mut per_proj: HashMap<String, i64> = HashMap::new();
        for e in &entries {
            let k = e.project_id.clone().unwrap_or_else(|| NONE_KEY.to_string());
            *per_proj.entry(k).or_insert(0) += entry_ms(e, period.from, period.to, now_ms);
        }
        let mut proj_rows: Vec<(String, i64)> = per_proj.into_iter().collect();
        proj_rows.sort_by_key(|a| std::cmp::Reverse(a.1));

        let proj_meta = |k: &str| -> (String, Color) {
            if k == NONE_KEY {
                (strings::REP_NO_PROJECT.to_string(), NO_PROJECT)
            } else if let Some(pr) = projects_by_id.get(k) {
                (
                    pr.name.clone(),
                    parse_hex_color(&pr.color).unwrap_or(NO_PROJECT),
                )
            } else {
                ("Deleted project".into(), NO_PROJECT)
            }
        };

        let (_vp, col) = build_scroll_page(tree, parent, 1140.0, 28.0, 16.0, self.scroll_y);

        // Header: Project filter Select + PeriodNav + Export CSV
        let head = tree.add_container(
            col,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::SpaceBetween),
                flex_wrap: FlexWrap::Wrap,
                gap: style::gap(12.0),
                flex_shrink: 0.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );
        let opts = Self::filter_options(&projects);
        let labels: Vec<&str> = opts.iter().map(|(_, s)| s.as_str()).collect();
        let sel_idx = opts.iter().position(|(f, _)| *f == self.filter);
        let f_int = Interact::from_state(self.sel_filter.trigger_id, env.hover, env.pressed, false);
        let (_, fid) = select::build(
            tree,
            head,
            &SelectProps {
                aria_label: "Project filter",
                options: &labels,
                selected: sel_idx,
                placeholder: strings::REP_ALL_PROJECTS,
                min_width: 200.0,
                open: self.sel_filter.is_open(),
                disabled: false,
                interact: f_int,
            },
            p,
        );
        self.sel_filter.trigger_id = Some(fid);

        let head_right = tree.add_container(
            head,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                gap: style::gap(8.0),
                ..Default::default()
            },
        );
        self.period_nav
            .build(tree, head_right, now_ms, first_day_mon0, env);
        let mut exp_p = ButtonProps::new(strings::REP_EXPORT_CSV);
        exp_p.icon = Some(glyph::DOWNLOAD);
        exp_p.disabled = entries.is_empty();
        exp_p.interact = Interact::from_state(
            self.id_export_btn,
            env.hover,
            env.pressed,
            entries.is_empty(),
        );
        let (_, eid) = button::build(tree, head_right, &exp_p, p);
        self.id_export_btn = Some(eid);

        if let Some(ref note) = self.export_notice {
            add_label(tree, col, note, 12.0, Weight::Medium, Align::Right, p.done);
        }

        // First-use empty banner
        self.id_start_timer_btn = None;
        if !has_any {
            let banner = build_card(tree, col, 18.0, 8.0, p);
            let row = tree.add_container(
                banner,
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    align_items: Some(AlignItems::Center),
                    justify_content: Some(JustifyContent::SpaceBetween),
                    gap: style::gap(12.0),
                    ..Default::default()
                },
            );
            let left_t = tree.add_container(
                row,
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
                left_t,
                "Reports fill in as you log time",
                14.0,
                Weight::SemiBold,
                Align::Left,
                p.text,
            );
            add_label(
                tree,
                left_t,
                "Hours, billable time, and earnings appear by day, week, month, or year.",
                12.5,
                Weight::Regular,
                Align::Left,
                p.muted,
            );
            let mut st_p = ButtonProps::new("Start tracking");
            st_p.icon = Some(glyph::PLAY);
            st_p.variant = ButtonVariant::Primary;
            let (_, sid) = button::build(tree, row, &st_p, p);
            self.id_start_timer_btn = Some(sid);
        }

        // 4 Stats cards
        let stats_row = tree.add_container(
            col,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                gap: style::gap(12.0),
                flex_shrink: 0.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );
        let ptitle = period_title(&period, now_ms);
        let s1_sub = if unique_days.is_empty() {
            "No days tracked".into()
        } else {
            format!(
                "{} per working day",
                duration(total_ms / unique_days.len() as i64)
            )
        };
        build_stat_card(
            tree,
            stats_row,
            &format!("Tracked · {ptitle}"),
            &duration(total_ms),
            Some(&s1_sub),
            p,
        );
        let s2_sub = if total_ms > 0 {
            format!(
                "{}% of tracked time",
                ((billable_ms as f64 / total_ms as f64) * 100.0).round() as i64
            )
        } else {
            "—".into()
        };
        build_stat_card(
            tree,
            stats_row,
            strings::REP_STAT_BILLABLE,
            &duration(billable_ms),
            Some(&s2_sub),
            p,
        );
        let s3_val = earned_list
            .first()
            .map(|(c, v)| money(*v, c))
            .unwrap_or_else(|| "—".into());
        let s3_sub = if earned_list.len() > 1 {
            earned_list[1..]
                .iter()
                .map(|(c, v)| format!("+ {}", money(*v, c)))
                .collect::<Vec<_>>()
                .join("  ")
        } else if earned_list.len() == 1 && billable_ms > 0 {
            format!(
                "{} per billable hour",
                money(
                    earned_list[0].1 / (billable_ms as f64 / HOUR as f64),
                    &earned_list[0].0
                )
            )
        } else {
            "Set a rate on a project to see earnings".into()
        };
        build_stat_card(
            tree,
            stats_row,
            strings::REP_STAT_EARNINGS,
            &s3_val,
            Some(&s3_sub),
            p,
        );
        let s4_sub = if active_ms > 0 {
            format!(
                "{}% logged as time",
                ((total_ms as f64 / active_ms as f64) * 100.0).round() as i64
            )
        } else {
            "No activity recorded".into()
        };
        build_stat_card(
            tree,
            stats_row,
            "Active on computer",
            &duration(active_ms),
            Some(&s4_sub),
            p,
        );

        // Hours / Earnings stacked bar chart
        let chart_card = build_card(tree, col, 18.0, 12.0, p);
        let chart_head = tree.add_container(
            chart_card,
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
        let chart_title = match self.metric {
            ChartMetric::Hours => strings::REP_CHART_HOURS.to_string(),
            ChartMetric::Earned => {
                if mixed {
                    format!("{} (in {chart_currency})", strings::REP_CHART_EARNINGS)
                } else {
                    strings::REP_CHART_EARNINGS.to_string()
                }
            }
        };
        add_label(
            tree,
            chart_head,
            &chart_title,
            14.0,
            Weight::SemiBold,
            Align::Left,
            p.text,
        );
        let seg_opts = [strings::REP_CHART_HOURS, strings::REP_CHART_EARNINGS];
        let (_, mids) = segmented::build(
            tree,
            chart_head,
            &SegmentedProps {
                group_name: "Chart metric",
                options: &seg_opts,
                selected: usize::from(self.metric == ChartMetric::Earned),
                hover: env.hover,
                disabled: false,
            },
            p,
        );
        self.id_metric_ids = mids;

        let bars_storage: Vec<Vec<(Color, f32)>> = period
            .buckets
            .iter()
            .map(|b| {
                proj_rows
                    .iter()
                    .filter_map(|(k, _)| {
                        let (_, color) = proj_meta(k);
                        let val: f32 = entries
                            .iter()
                            .filter(|e| e.project_id.as_deref().unwrap_or(NONE_KEY) == k)
                            .map(|e| match self.metric {
                                ChartMetric::Hours => {
                                    (entry_ms(e, b.from, b.to, now_ms) as f32) / (HOUR as f32)
                                }
                                ChartMetric::Earned => {
                                    let proj = e
                                        .project_id
                                        .as_deref()
                                        .and_then(|pid| projects_by_id.get(pid));
                                    if proj.is_some_and(|pr| pr.currency == chart_currency) {
                                        earned(e, proj, b.from, b.to, now_ms) as f32
                                    } else {
                                        0.0
                                    }
                                }
                            })
                            .sum();
                        (val > 0.0).then_some((color, val))
                    })
                    .collect()
            })
            .collect();
        let bar_refs: Vec<&[(Color, f32)]> = bars_storage.iter().map(Vec::as_slice).collect();
        let chart_w = (env.width_dip - 100.0).clamp(260.0, 980.0);
        charts::stacked_bars(tree, chart_card, &chart_title, &bar_refs, chart_w, 170.0, p);

        // Proportion bar + Virtualized EntryTree card
        let tree_card = build_card(tree, col, 18.0, 12.0, p);
        add_label(
            tree,
            tree_card,
            "Projects and time entries",
            14.0,
            Weight::SemiBold,
            Align::Left,
            p.text,
        );
        if proj_rows.is_empty() {
            add_label(
                tree,
                tree_card,
                strings::REP_EMPTY,
                12.5,
                Weight::Regular,
                Align::Left,
                p.muted,
            );
        } else {
            let prop_segs: Vec<(Color, f32)> = proj_rows
                .iter()
                .map(|(k, ms)| {
                    let (_, c) = proj_meta(k);
                    (c, *ms as f32)
                })
                .collect();
            charts::proportion_bar(
                tree,
                tree_card,
                strings::REP_BY_PROJECT,
                &prop_segs,
                chart_w,
                10.0,
                p,
            );
            self.entry_tree.build(
                tree,
                tree_card,
                &entries,
                &projects_by_id,
                period.from,
                period.to,
                now_ms,
                true,
                is_24h,
                env.hover,
                p,
            );
        }

        if let Some(ref mut ed) = self.entry_editor {
            return Some(ed.build(tree, overlay_root, &self.store, &projects, env));
        }
        if let Some(ref mut cdlg) = self.csv_dialog {
            let dlg = dialog::build(
                tree,
                overlay_root,
                strings::REP_EXPORT_DLG_TITLE,
                420.0,
                env.hover,
                p,
            );
            cdlg.id_close = Some(dlg.close_id);
            let d_opts = [strings::REP_EXPORT_EXCEL, strings::REP_EXPORT_STANDARD];
            let (_, dids) = segmented::build(
                tree,
                dlg.body,
                &SegmentedProps {
                    group_name: "CSV dialect",
                    options: &d_opts,
                    selected: usize::from(cdlg.standard),
                    hover: env.hover,
                    disabled: false,
                },
                p,
            );
            cdlg.id_dialect_ids = dids;

            if let Some(ref err) = cdlg.status_err {
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

            let foot = tree.add_container(
                dlg.body,
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    justify_content: Some(JustifyContent::FlexEnd),
                    gap: style::gap(8.0),
                    padding: style::pad(0.0, 6.0, 0.0, 0.0),
                    ..Default::default()
                },
            );
            let mut cp = ButtonProps::new(strings::CANCEL_LABEL);
            let (_, cid) = button::build(tree, foot, &cp, p);
            cdlg.id_cancel = Some(cid);
            cp.label = "Choose file…";
            cp.variant = ButtonVariant::Primary;
            let (_, okid) = button::build(tree, foot, &cp, p);
            cdlg.id_confirm = Some(okid);
            return Some(dlg.card);
        }
        None
    }

    pub fn handle_click(&mut self, id: WidgetId, palette: &Palette) -> bool {
        if let Some(ref mut ed) = self.entry_editor {
            let (handled, out) = ed.on_click(id, &self.store, &self.cached_projects, palette);
            if matches!(
                out,
                EntryEditorOutcome::Closed
                    | EntryEditorOutcome::Saved
                    | EntryEditorOutcome::Deleted
            ) {
                self.entry_editor = None;
            }
            if handled {
                return true;
            }
        }
        if let Some(ref mut cdlg) = self.csv_dialog {
            if Some(id) == cdlg.id_close || Some(id) == cdlg.id_cancel {
                self.csv_dialog = None;
                return true;
            }
            if let Some(pos) = cdlg.id_dialect_ids.iter().position(|x| *x == id) {
                cdlg.standard = pos == 1;
                return true;
            }
            if Some(id) == cdlg.id_confirm {
                let standard = cdlg.standard;
                match self.run_csv_export(standard) {
                    Ok(_) => self.csv_dialog = None,
                    Err(e) => {
                        if let Some(ref mut cdlg) = self.csv_dialog {
                            cdlg.status_err = Some(e);
                        }
                    }
                }
                return true;
            }
        }
        let now_ms = self.store.borrow().now_ms();
        if self.period_nav.on_click(id, now_ms, palette) {
            return true;
        }
        if Some(id) == self.sel_filter.trigger_id {
            let opts = Self::filter_options(&self.cached_projects);
            let refs: Vec<&str> = opts.iter().map(|(_, s)| s.as_str()).collect();
            let sel = opts.iter().position(|(f, _)| *f == self.filter);
            self.sel_filter.open(&refs, sel, palette);
            return true;
        }
        if Some(id) == self.id_export_btn && !self.cached_entries.is_empty() {
            let default_std = self
                .store
                .borrow()
                .get_settings()
                .map(|s| s.csv_standard)
                .unwrap_or(false);
            self.export_notice = None;
            self.csv_dialog = Some(CsvExportDialog {
                standard: default_std,
                status_err: None,
                id_close: None,
                id_dialect_ids: Vec::new(),
                id_cancel: None,
                id_confirm: None,
            });
            return true;
        }
        if Some(id) == self.id_start_timer_btn {
            self.go_timer_requested = true;
            return true;
        }
        if let Some(pos) = self.id_metric_ids.iter().position(|x| *x == id) {
            self.metric = if pos == 1 {
                ChartMetric::Earned
            } else {
                ChartMetric::Hours
            };
            return true;
        }
        let et = self.entry_tree.handle_click(id);
        if let Some(entry) = et.open_entry {
            self.entry_editor = Some(EntryEditorState::new(EntryDraft::from_entry(&entry)));
            return true;
        }
        et.handled
    }

    pub fn handle_key(&mut self, key: KeyCode) -> bool {
        if self.entry_editor.is_some() || self.csv_dialog.is_some() {
            return false;
        }
        self.entry_tree.handle_key(key)
    }

    pub fn handle_escape(&mut self) -> bool {
        if self.csv_dialog.is_some() {
            self.csv_dialog = None;
            return true;
        }
        if self.entry_editor.is_some() {
            self.entry_editor = None;
            return true;
        }
        false
    }

    pub fn handle_wheel_at(&mut self, x_dip: f32, y_dip: f32, delta_dip: f32) -> bool {
        if self.entry_editor.is_some() || self.csv_dialog.is_some() {
            return false;
        }
        if let Some(vp_id) = self.entry_tree.viewport_id {
            if let Some(rect) = window::widget_rect(vp_id) {
                if rect.contains(x_dip, y_dip) && self.entry_tree.handle_wheel(delta_dip) {
                    return true;
                }
            }
        }
        let max_scroll = (1100.0 - self.viewport_h).max(0.0);
        scroll_wheel_step(&mut self.scroll_y, delta_dip, max_scroll)
    }
}

impl Screen for ReportsScreen {
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
        self.entry_editor
            .as_mut()
            .is_some_and(|e| e.on_text_change(key, value))
    }

    fn on_text_submit(&mut self, key: &str, value: &str) -> bool {
        if let Some(ref mut ed) = self.entry_editor {
            let out = ed.on_text_submit(key, value, &self.store);
            if matches!(out, EntryEditorOutcome::Saved | EntryEditorOutcome::Closed) {
                self.entry_editor = None;
            }
            return true;
        }
        false
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
        self.handle_wheel_at(-1.0, -1.0, delta_dip)
    }

    fn on_wheel_at(&mut self, x_dip: f32, y_dip: f32, _dx_dip: f32, dy_dip: f32) -> bool {
        self.handle_wheel_at(x_dip, y_dip, dy_dip)
    }

    fn on_popup_poll(&mut self) -> bool {
        let mut changed = self.period_nav.poll_popup();
        if let Some(res) = self.sel_filter.poll() {
            if let Some(idx) = res {
                let opts = Self::filter_options(&self.cached_projects);
                if let Some((f, _)) = opts.get(idx) {
                    self.filter = f.clone();
                }
            }
            changed = true;
        }
        if let Some(ref mut ed) = self.entry_editor {
            changed |= ed.poll_popup(&self.cached_projects);
        }
        changed
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
    use std::rc::Rc;

    #[test]
    fn reports_screen_validates_a11y_exports_dialog_and_toggles_earnings() {
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
        let screen = ReportsScreen::new(Rc::clone(&store));
        let mut focus = FocusModel::new();
        let mut host = ScreenHost::mount(Box::new(screen), &env, &mut focus);
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));

        // Open Export CSV dialog -> switch dialect -> Esc closes.
        let exp_btn = host
            .tree()
            .all_nodes()
            .into_iter()
            .filter_map(|n| host.tree().data(n))
            .find(|d| d.a11y.name == strings::REP_EXPORT_CSV)
            .unwrap()
            .id;
        assert!(host.on_click(exp_btn));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));
        assert!(matches!(focus.active_scope(), FocusScope::Modal(_)));

        let std_btn = host
            .tree()
            .all_nodes()
            .into_iter()
            .filter_map(|n| host.tree().data(n))
            .find(|d| d.a11y.name == strings::REP_EXPORT_STANDARD)
            .unwrap()
            .id;
        assert!(host.on_click(std_btn));
        assert!(host.rebuild_if_dirty(&env, &mut focus));

        let (_, dirty) = host.on_key(
            crate::ui::input::KeyEvent {
                key: KeyCode::Escape,
                shift: false,
                ctrl: false,
                alt: false,
            },
            &mut focus,
        );
        assert!(dirty);
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(focus.active_scope(), FocusScope::Main);
    }
}
