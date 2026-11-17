//! Single-project report screen: PeriodNav, Edit project modal, 4 SQL/interval stats,
//! Hours/Earnings stacked bars, clipped activity during entries, app & file drilldown,
//! and virtualized `EntryTree`.

use std::collections::{HashMap, HashSet};

use taffy::prelude::*;

use crate::db::{Activity, Project, SharedStore, TimeEntry};
use crate::format::{self};
use crate::stats::{earned, entry_ms};
use crate::time::{overlap, start_of_day, HOUR};
use crate::ui::input::KeyCode;
use crate::ui::period::{fit_period, period_title, PeriodKind, PeriodNavState};
use crate::ui::screen::{BuiltScreen, Screen, ScreenEnv};
use crate::ui::strings;
use crate::ui::text::{Align, Weight};
use crate::ui::theme::{app_color, mix, parse_hex_color, Palette, NO_PROJECT};
use crate::ui::tree::{style, Node, Tree, WidgetId};
use crate::ui::widgets::button::{self, ButtonProps, ButtonSize, ButtonVariant};
use crate::ui::widgets::charts;
use crate::ui::widgets::segmented::{self, SegmentedProps};
use crate::ui::widgets::{add_label, glyph, Interact};
use crate::ui::window;

use super::entry_tree::EntryTreeState;
use super::projects::{ProjectDialogOutcome, ProjectDialogState};
use super::{
    build_card, build_empty_state, build_scroll_page, build_stat_card, screen_root_tree,
    scroll_wheel_step,
};

const CLAUDE: &str = "Claude Code";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartMetric {
    Hours,
    Earned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaudeGrouping {
    Project,
    Session,
}

/// Disjoint merged `[start, end)` intervals sorted ascending.
pub fn merged_intervals(entries: &[TimeEntry], from: i64, to: i64, now_ms: i64) -> Vec<(i64, i64)> {
    let mut raw: Vec<(i64, i64)> = entries
        .iter()
        .filter_map(|e| {
            let s = e.start.max(from);
            let en = e.end.unwrap_or(now_ms).min(to);
            (en > s).then_some((s, en))
        })
        .collect();
    raw.sort_unstable_by_key(|&(s, _)| s);
    let mut out: Vec<(i64, i64)> = Vec::with_capacity(raw.len());
    for (s, e) in raw {
        if let Some(last) = out.last_mut() {
            if s <= last.1 {
                last.1 = last.1.max(e);
                continue;
            }
        }
        out.push((s, e));
    }
    out
}

pub struct ProjectReportScreen {
    store: SharedStore,
    pub project_id: String,
    period_nav: PeriodNavState,
    metric: ChartMetric,
    selected_app: Option<String>,
    claude_by: ClaudeGrouping,
    scroll_y: f32,
    viewport_h: f32,
    entry_tree: EntryTreeState,
    edit_dialog: Option<ProjectDialogState>,

    id_back: Option<WidgetId>,
    id_edit_btn: Option<WidgetId>,
    id_metric_ids: Vec<WidgetId>,
    id_app_rows: Vec<(WidgetId, String)>,
    id_claude_ids: Vec<WidgetId>,

    pub back_requested: bool,
    pub opened_entry: Option<TimeEntry>,
    on_back: Option<Box<dyn FnMut()>>,
    on_open_entry: Option<Box<dyn FnMut(TimeEntry)>>,
}

impl ProjectReportScreen {
    pub fn new(store: SharedStore, project_id: impl Into<String>) -> Self {
        let now_ms = store.borrow().now_ms();
        Self {
            store,
            project_id: project_id.into(),
            period_nav: PeriodNavState::new(PeriodKind::Month, now_ms),
            metric: ChartMetric::Hours,
            selected_app: None,
            claude_by: ClaudeGrouping::Project,
            scroll_y: 0.0,
            viewport_h: 720.0,
            entry_tree: EntryTreeState::new(),
            edit_dialog: None,
            id_back: None,
            id_edit_btn: None,
            id_metric_ids: Vec::new(),
            id_app_rows: Vec::new(),
            id_claude_ids: Vec::new(),
            back_requested: false,
            opened_entry: None,
            on_back: None,
            on_open_entry: None,
        }
    }

    pub fn with_on_back(mut self, f: impl FnMut() + 'static) -> Self {
        self.on_back = Some(Box::new(f));
        self
    }

    pub fn with_on_open_entry(mut self, f: impl FnMut(TimeEntry) + 'static) -> Self {
        self.on_open_entry = Some(Box::new(f));
        self
    }

    fn app_name_of(a: &Activity) -> &str {
        if a.kind == "claude" {
            CLAUDE
        } else {
            &a.app
        }
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
            project,
            all_time,
            entries,
            period,
            ranked_apps,
            covered_ms,
            detail_rows,
        ) = {
            let st = self.store.borrow();
            let now_ms = st.now_ms();
            let loc = st.system_locale().clone();
            let settings = st
                .get_settings()
                .unwrap_or_else(|_| crate::db::Settings::defaults_for(&loc));
            let first_day_mon0 = settings.first_day_of_week.resolve_mon0(&loc);
            let is_24h = settings.time_format.is_24h(&loc);

            let project = st.get_project(&self.project_id).ok();
            let all_time = st
                .project_all_time_stats(&self.project_id)
                .unwrap_or_default();
            let raw_p = self.period_nav.period(now_ms, first_day_mon0);
            let period = fit_period(raw_p, all_time.first_start, now_ms);

            let mut entries: Vec<TimeEntry> = st
                .list_entries(period.from, period.to)
                .unwrap_or_default()
                .into_iter()
                .filter(|e| e.project_id.as_deref() == Some(self.project_id.as_str()))
                .collect();
            entries.sort_by_key(|e| e.start);

            let iv = merged_intervals(&entries, period.from, period.to, now_ms);
            let mut app_totals: HashMap<String, i64> = HashMap::new();
            let mut covered_iv: Vec<(i64, i64)> = Vec::new();
            let mut detail_totals: HashMap<
                (String, String),
                (String, String, i64, crate::ui::theme::Color),
            > = HashMap::new();

            if !iv.is_empty() {
                let sel_filter = self.selected_app.clone();
                let claude_by = self.claude_by;
                let _ = st.visit_activity(period.from, period.to, &mut |a| {
                    // Find overlapping entry intervals via partition point.
                    let idx = iv.partition_point(|&(_, e)| e <= a.start);
                    for &(s, e) in &iv[idx..] {
                        if s >= a.end {
                            break;
                        }
                        let ov = overlap(a.start, a.end, s, e);
                        if ov <= 0 {
                            continue;
                        }
                        let cs = a.start.max(s);
                        let ce = a.end.min(e);
                        let name = Self::app_name_of(&a).to_string();
                        *app_totals.entry(name.clone()).or_insert(0) += ce - cs;
                        if a.kind == "app" {
                            covered_iv.push((cs, ce));
                        }

                        let include_detail = match sel_filter.as_deref() {
                            Some(target) => name == target,
                            None => a.kind != "claude",
                        };
                        if include_detail {
                            let is_claude = a.kind == "claude";
                            let label = if is_claude {
                                match claude_by {
                                    ClaudeGrouping::Project => {
                                        if a.context.is_empty() {
                                            strings::PROJECTS_NO_PROJECT.to_string()
                                        } else {
                                            a.context.clone()
                                        }
                                    }
                                    ClaudeGrouping::Session => {
                                        if a.detail.is_empty() {
                                            "Untitled session".into()
                                        } else {
                                            a.detail.clone()
                                        }
                                    }
                                }
                            } else if !a.detail.is_empty() {
                                a.detail.clone()
                            } else if sel_filter.is_some() {
                                "No window title".into()
                            } else {
                                a.app.clone()
                            };
                            let sub = if is_claude {
                                match claude_by {
                                    ClaudeGrouping::Project => String::new(),
                                    ClaudeGrouping::Session => a.context.clone(),
                                }
                            } else if sel_filter.is_some() {
                                a.context.clone()
                            } else if !a.detail.is_empty() {
                                a.app.clone()
                            } else {
                                String::new()
                            };
                            let c = app_color(&name);
                            let entry = detail_totals
                                .entry((label.clone(), sub.clone()))
                                .or_insert((label, sub, 0, c));
                            entry.2 += ce - cs;
                        }
                    }
                });
            }

            let mut ranked_apps: Vec<(String, i64)> = app_totals
                .into_iter()
                .filter(|(_, ms)| *ms >= 30_000)
                .collect();
            ranked_apps.sort_by_key(|a| std::cmp::Reverse(a.1));

            covered_iv.sort_unstable_by_key(|&(s, _)| s);
            let mut covered_ms = 0i64;
            let mut cur_iv: Option<(i64, i64)> = None;
            for (s, e) in covered_iv {
                match cur_iv {
                    None => cur_iv = Some((s, e)),
                    Some((cs, ce)) if s <= ce => cur_iv = Some((cs, ce.max(e))),
                    Some((cs, ce)) => {
                        covered_ms += ce - cs;
                        cur_iv = Some((s, e));
                    }
                }
            }
            if let Some((cs, ce)) = cur_iv {
                covered_ms += ce - cs;
            }

            let mut detail_rows: Vec<(String, String, i64, crate::ui::theme::Color)> =
                detail_totals
                    .into_values()
                    .filter(|d| d.2 >= 30_000)
                    .collect();
            detail_rows.sort_by_key(|a| std::cmp::Reverse(a.2));
            detail_rows.truncate(12);

            (
                now_ms,
                first_day_mon0,
                is_24h,
                project,
                all_time,
                entries,
                period,
                ranked_apps,
                covered_ms,
                detail_rows,
            )
        };

        let (_vp, col) = build_scroll_page(tree, parent, 1100.0, 28.0, 16.0, self.scroll_y);

        let Some(proj) = project else {
            let empty = build_empty_state(
                tree,
                col,
                glyph::PROJECTS,
                "This project no longer exists",
                "It may have been deleted or archived.",
                p,
            );
            let back_p = ButtonProps::new(strings::PROJ_REP_BACK);
            let (_, bid) = button::build(tree, empty, &back_p, p);
            self.id_back = Some(bid);
            return None;
        };

        // Header
        let head = tree.add_container(
            col,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::FlexEnd),
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
        let head_left = tree.add_container(
            head,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                gap: style::gap(4.0),
                ..Default::default()
            },
        );
        let mut back_lbl = strings::PROJ_REP_BACK.to_string();
        if !proj.client.trim().is_empty() {
            back_lbl.push_str(" · ");
            back_lbl.push_str(proj.client.trim());
        }
        if proj.archived {
            back_lbl.push_str(" · Archived");
        }
        let mut back_props = ButtonProps::new(&back_lbl);
        back_props.icon = Some(glyph::CHEVRON_LEFT);
        back_props.variant = ButtonVariant::Ghost;
        back_props.size = ButtonSize::Small;
        back_props.interact = Interact::from_state(self.id_back, env.hover, env.pressed, false);
        let (_, bid) = button::build(tree, head_left, &back_props, p);
        self.id_back = Some(bid);

        let title_row = tree.add_container(
            head_left,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                gap: style::gap(10.0),
                ..Default::default()
            },
        );
        let color = parse_hex_color(&proj.color).unwrap_or(NO_PROJECT);
        let dot = tree.add_container(
            title_row,
            Style {
                size: style::size(10.0, 10.0),
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        tree.set_paint(
            dot,
            crate::ui::tree::PaintStyle {
                bg: Some(color),
                border_color: None,
                border_width: 0.0,
                radius: 5.0,
                clip: false,
            },
        );
        add_label(
            tree,
            title_row,
            &proj.name,
            22.0,
            Weight::SemiBold,
            Align::Left,
            p.text,
        );

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
        let mut edit_props = ButtonProps::new(strings::PROJ_REP_EDIT);
        edit_props.icon = Some(glyph::EDIT);
        edit_props.interact = Interact::from_state(self.id_edit_btn, env.hover, env.pressed, false);
        let (_, eid) = button::build(tree, head_right, &edit_props, p);
        self.id_edit_btn = Some(eid);

        // 4 Stats row
        let total_ms: i64 = entries
            .iter()
            .map(|e| entry_ms(e, period.from, period.to, now_ms))
            .sum();
        let billable_ms: i64 = entries
            .iter()
            .filter(|e| e.billable)
            .map(|e| entry_ms(e, period.from, period.to, now_ms))
            .sum();
        let income: f64 = entries
            .iter()
            .map(|e| earned(e, Some(&proj), period.from, period.to, now_ms))
            .sum();
        let unique_days: HashSet<i64> = entries.iter().map(|e| start_of_day(e.start)).collect();

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
        let stat1_sub = if unique_days.is_empty() {
            "Nothing tracked".into()
        } else {
            format!(
                "{} {} on {} {}",
                entries.len(),
                if entries.len() == 1 {
                    "entry"
                } else {
                    "entries"
                },
                unique_days.len(),
                if unique_days.len() == 1 {
                    "day"
                } else {
                    "days"
                }
            )
        };
        build_stat_card(
            tree,
            stats_row,
            &format!("Tracked · {ptitle}"),
            &format::duration(total_ms),
            Some(&stat1_sub),
            p,
        );
        let bill_sub = if total_ms > 0 {
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
            &format::duration(billable_ms),
            Some(&bill_sub),
            p,
        );
        let earned_val = if proj.rate > 0.0 {
            format::money(income, &proj.currency)
        } else {
            "—".into()
        };
        let earned_sub = if proj.rate > 0.0 {
            format!("{} per hour", format::money(proj.rate, &proj.currency))
        } else {
            "No rate set on this project".into()
        };
        build_stat_card(
            tree,
            stats_row,
            strings::REP_STAT_EARNINGS,
            &earned_val,
            Some(&earned_sub),
            p,
        );
        let ever_sub = if all_time.earned > 0.0 {
            format!("{} earned", format::money(all_time.earned, &proj.currency))
        } else if all_time.count > 0 {
            format!("{} entries", all_time.count)
        } else {
            "No time yet".into()
        };
        build_stat_card(
            tree,
            stats_row,
            strings::PERIOD_ALL,
            &format::duration(all_time.total_ms),
            Some(&ever_sub),
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
        let metric_title = if self.metric == ChartMetric::Hours || proj.rate <= 0.0 {
            strings::REP_CHART_HOURS
        } else {
            strings::REP_CHART_EARNINGS
        };
        add_label(
            tree,
            chart_head,
            metric_title,
            14.0,
            Weight::SemiBold,
            Align::Left,
            p.text,
        );
        self.id_metric_ids.clear();
        if proj.rate > 0.0 {
            let seg_opts = [strings::REP_CHART_HOURS, strings::REP_CHART_EARNINGS];
            let (_, ids) = segmented::build(
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
            self.id_metric_ids = ids;
        }

        let muted_color = mix(color, p.surface, 0.35);
        let bars_storage: Vec<Vec<(crate::ui::theme::Color, f32)>> = period
            .buckets
            .iter()
            .map(|b| {
                if self.metric == ChartMetric::Earned && proj.rate > 0.0 {
                    let e: f64 = entries
                        .iter()
                        .map(|x| earned(x, Some(&proj), b.from, b.to, now_ms))
                        .sum();
                    vec![(color, e as f32)]
                } else {
                    let bill: i64 = entries
                        .iter()
                        .filter(|x| x.billable)
                        .map(|x| entry_ms(x, b.from, b.to, now_ms))
                        .sum();
                    let rest: i64 = entries
                        .iter()
                        .filter(|x| !x.billable)
                        .map(|x| entry_ms(x, b.from, b.to, now_ms))
                        .sum();
                    vec![
                        (color, (bill as f32) / (HOUR as f32)),
                        (muted_color, (rest as f32) / (HOUR as f32)),
                    ]
                }
            })
            .collect();
        let bar_refs: Vec<&[(crate::ui::theme::Color, f32)]> =
            bars_storage.iter().map(Vec::as_slice).collect();
        let chart_w = (env.width_dip - 100.0).clamp(260.0, 980.0);
        charts::stacked_bars(tree, chart_card, metric_title, &bar_refs, chart_w, 160.0, p);

        // Two-column Apps + Files/Windows drilldown
        let grid = tree.add_container(
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
        let left_col = tree.add_container(
            grid,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                ..Default::default()
            },
        );
        let apps_card = build_card(tree, left_col, 18.0, 10.0, p);
        let mut apps_title = "Apps while tracking".to_string();
        if total_ms > 0 && covered_ms > 0 {
            let pct = ((covered_ms as f64 / total_ms as f64) * 100.0).round() as i64;
            apps_title.push_str(&format!(" · {pct}% active"));
        }
        add_label(
            tree,
            apps_card,
            &apps_title,
            14.0,
            Weight::SemiBold,
            Align::Left,
            p.text,
        );
        self.id_app_rows.clear();
        if ranked_apps.is_empty() {
            add_label(
                tree,
                apps_card,
                "No activity recorded during this project's entries.",
                12.5,
                Weight::Regular,
                Align::Left,
                p.muted,
            );
        } else {
            let max_app_ms = ranked_apps[0].1.max(1) as f32;
            for (name, ms) in &ranked_apps {
                let selected = self.selected_app.as_deref() == Some(name.as_str());
                let row_int = Interact::from_state(
                    self.id_app_rows
                        .iter()
                        .find(|(_, n)| n == name)
                        .map(|(id, _)| *id),
                    env.hover,
                    env.pressed,
                    false,
                );
                let (_, rid) = charts::share_row_button(
                    tree,
                    apps_card,
                    name,
                    &format::duration(*ms),
                    (*ms as f32) / max_app_ms,
                    app_color(name),
                    selected,
                    row_int,
                    p,
                );
                self.id_app_rows.push((rid, name.clone()));
            }
        }

        let right_col = tree.add_container(
            grid,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                ..Default::default()
            },
        );
        let details_card = build_card(tree, right_col, 18.0, 10.0, p);
        let details_head = tree.add_container(
            details_card,
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
        let detail_title = match self.selected_app.as_deref() {
            Some(app_name) => format!("In {app_name}"),
            None => strings::PROJ_REP_FILES.to_string(),
        };
        add_label(
            tree,
            details_head,
            &detail_title,
            14.0,
            Weight::SemiBold,
            Align::Left,
            p.text,
        );
        self.id_claude_ids.clear();
        if self.selected_app.as_deref() == Some(CLAUDE) {
            let opts = [
                strings::ACT_CLAUDE_MODE_PROJECTS,
                strings::ACT_CLAUDE_MODE_SESSIONS,
            ];
            let (_, ids) = segmented::build(
                tree,
                details_head,
                &SegmentedProps {
                    group_name: "Group Claude Code by",
                    options: &opts,
                    selected: usize::from(self.claude_by == ClaudeGrouping::Session),
                    hover: env.hover,
                    disabled: false,
                },
                p,
            );
            self.id_claude_ids = ids;
        }
        if detail_rows.is_empty() {
            add_label(
                tree,
                details_card,
                "No window titles recorded for this selection.",
                12.5,
                Weight::Regular,
                Align::Left,
                p.muted,
            );
        } else {
            let max_d = detail_rows[0].2.max(1) as f32;
            for (lbl, sub, ms, col_c) in &detail_rows {
                let full_lbl = if sub.is_empty() {
                    lbl.clone()
                } else {
                    format!("{lbl} ({sub})")
                };
                charts::share_row(
                    tree,
                    details_card,
                    &full_lbl,
                    &format::duration(*ms),
                    (*ms as f32) / max_d,
                    *col_c,
                    p,
                );
            }
        }

        // Bottom virtualized EntryTree card
        let tree_card = build_card(tree, col, 18.0, 12.0, p);
        let entry_count_str = if entries.len() == 1 {
            "1 entry".to_string()
        } else {
            format!("{} entries", entries.len())
        };
        add_label(
            tree,
            tree_card,
            format!("{} · {entry_count_str}", strings::PROJ_REP_WHAT_WORKED_ON),
            14.0,
            Weight::SemiBold,
            Align::Left,
            p.text,
        );
        let projects_by_id: HashMap<String, Project> =
            [(proj.id.clone(), proj.clone())].into_iter().collect();
        if entries.is_empty() {
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
            self.entry_tree.build(
                tree,
                tree_card,
                &entries,
                &projects_by_id,
                period.from,
                period.to,
                now_ms,
                false,
                is_24h,
                env.hover,
                p,
            );
        }

        self.edit_dialog
            .as_mut()
            .map(|dlg| dlg.build(tree, overlay_root, env))
    }

    pub fn handle_click(&mut self, id: WidgetId, palette: &Palette) -> bool {
        if let Some(ref mut dlg) = self.edit_dialog {
            let (handled, out) = dlg.on_click(id, &self.store, palette);
            match out {
                ProjectDialogOutcome::Closed | ProjectDialogOutcome::Saved => {
                    self.edit_dialog = None;
                }
                ProjectDialogOutcome::Deleted => {
                    self.edit_dialog = None;
                    self.back_requested = true;
                    if let Some(cb) = self.on_back.as_mut() {
                        cb();
                    }
                }
                ProjectDialogOutcome::None => {}
            }
            if handled {
                return true;
            }
        }
        if Some(id) == self.id_back {
            self.back_requested = true;
            if let Some(cb) = self.on_back.as_mut() {
                cb();
            }
            return true;
        }
        if Some(id) == self.id_edit_btn {
            if let Ok(proj) = self.store.borrow().get_project(&self.project_id) {
                self.edit_dialog = Some(ProjectDialogState::for_edit(proj));
            }
            return true;
        }
        let now_ms = self.store.borrow().now_ms();
        if self.period_nav.on_click(id, now_ms, palette) {
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
        if let Some((_, app_name)) = self.id_app_rows.iter().find(|(rid, _)| *rid == id) {
            if self.selected_app.as_deref() == Some(app_name.as_str()) {
                self.selected_app = None;
            } else {
                self.selected_app = Some(app_name.clone());
            }
            return true;
        }
        if let Some(pos) = self.id_claude_ids.iter().position(|x| *x == id) {
            self.claude_by = if pos == 1 {
                ClaudeGrouping::Session
            } else {
                ClaudeGrouping::Project
            };
            return true;
        }
        let et = self.entry_tree.handle_click(id);
        if let Some(entry) = et.open_entry {
            self.opened_entry = Some(entry.clone());
            if let Some(cb) = self.on_open_entry.as_mut() {
                cb(entry);
            }
            return true;
        }
        et.handled
    }

    pub fn handle_key(&mut self, key: KeyCode) -> bool {
        if let Some(ref mut dlg) = self.edit_dialog {
            return dlg.on_key(key);
        }
        self.entry_tree.handle_key(key)
    }

    pub fn handle_escape(&mut self) -> bool {
        if self.edit_dialog.is_some() {
            self.edit_dialog = None;
            return true;
        }
        if self.selected_app.is_some() {
            self.selected_app = None;
            return true;
        }
        false
    }

    pub fn handle_wheel_at(&mut self, x_dip: f32, y_dip: f32, delta_dip: f32) -> bool {
        if self.edit_dialog.is_some() {
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

impl Screen for ProjectReportScreen {
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
        self.edit_dialog
            .as_mut()
            .is_some_and(|d| d.on_text_change(key, value))
    }

    fn on_text_submit(&mut self, key: &str, value: &str) -> bool {
        if let Some(ref mut dlg) = self.edit_dialog {
            let out = dlg.on_text_submit(key, value, &self.store);
            if matches!(
                out,
                ProjectDialogOutcome::Saved | ProjectDialogOutcome::Closed
            ) {
                self.edit_dialog = None;
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
        if let Some(ref mut dlg) = self.edit_dialog {
            changed |= dlg.poll_popup();
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
    fn project_report_validates_a11y_clips_activity_and_edits_project() {
        let text = TextSystem::new().expect("dwrite");
        let palette = Palette::LIGHT;
        let env = ScreenEnv {
            width_dip: 1120.0,
            height_dip: 760.0,
            hover: None,
            pressed: None,
            focused: None,
            palette: &palette,
            text: &text,
        };
        let store = shared_store(Box::new(DemoStore::new(SystemLocale::en_us())));
        let projects = store.borrow().list_projects().unwrap();
        let pid = projects[0].id.clone();

        let screen = ProjectReportScreen::new(Rc::clone(&store), pid.clone());
        let mut focus = FocusModel::new();
        let mut host = ScreenHost::mount(Box::new(screen), &env, &mut focus);
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));

        // Open Edit project modal via button click -> validates modal a11y + Esc closes.
        let edit_btn = host
            .tree()
            .all_nodes()
            .into_iter()
            .filter_map(|n| host.tree().data(n))
            .find(|d| d.a11y.name == strings::PROJ_REP_EDIT)
            .unwrap()
            .id;
        assert!(host.on_click(edit_btn));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));
        assert!(matches!(focus.active_scope(), FocusScope::Modal(_)));

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

        // Toggle chart metric to Earnings via segmented button.
        let earn_btn = host
            .tree()
            .all_nodes()
            .into_iter()
            .filter_map(|n| host.tree().data(n))
            .find(|d| d.a11y.name == strings::REP_CHART_EARNINGS)
            .unwrap()
            .id;
        assert!(host.on_click(earn_btn));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));
    }
}
