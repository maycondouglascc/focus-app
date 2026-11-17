//! Activity analytics screen: PeriodNav, app filter chips, weekday×hour &
//! quarter-hour heatmaps, stacked bars, app list w/ sparklines, and Claude
//! Project/Session breakdown — computed strictly within the visible period via
//! O(1)-space streaming SQLite scans.

use std::collections::{HashMap, HashSet};

use taffy::prelude::*;

use crate::db::SharedStore;
use crate::format::{duration, weekday_short};
use crate::stats::union_ms;
use crate::time::{at_minutes, overlap, start_of_day, to_local, HOUR};
use crate::ui::period::{fit_period, PeriodKind, PeriodNavState};
use crate::ui::screen::{BuiltScreen, Screen, ScreenEnv};
use crate::ui::strings;
use crate::ui::text::{Align, Weight};
use crate::ui::theme::{app_color, Palette};
use crate::ui::tree::{style, A11yInfo, A11yRole, Node, Tree, WidgetId};
use crate::ui::widgets::button::{self, ButtonProps, ButtonSize, ButtonVariant};
use crate::ui::widgets::charts;
use crate::ui::widgets::segmented::{self, SegmentedProps};
use crate::ui::widgets::{add_label, glyph, Interact};

use super::project_report::ClaudeGrouping;
use super::{
    build_card, build_empty_state, build_scroll_page, screen_root_tree, scroll_wheel_step,
};

const CLAUDE: &str = "Claude Code";

/// Splits `[start, end)` at local hour boundaries and accumulates durations into
/// a `[7 weekday][24 hour]` grid.
pub fn add_to_hour_grid(grid: &mut [[i64; 24]; 7], mut start: i64, end: i64) {
    while start < end {
        let l = to_local(start);
        let day0 = start_of_day(start);
        let next_hour_ts = at_minutes(day0, (i32::from(l.hour) + 1) * 60);
        let stop = end.min(next_hour_ts.max(start + 1));
        let dow = usize::from(l.weekday_mon0()) % 7;
        let hr = usize::from(l.hour) % 24;
        grid[dow][hr] += stop - start;
        start = stop;
    }
}

/// Splits `[start, end)` at 15-minute local boundaries and accumulates into
/// a `[4 quarter][24 hour]` grid for Day view.
pub fn add_to_quarter_grid(grid: &mut [[i64; 24]; 4], mut start: i64, end: i64) {
    while start < end {
        let l = to_local(start);
        let day0 = start_of_day(start);
        let q = (l.minute / 15).min(3);
        let next_q_min = i32::from(l.hour) * 60 + (i32::from(q) + 1) * 15;
        let next_ts = at_minutes(day0, next_q_min);
        let stop = end.min(next_ts.max(start + 1));
        let hr = usize::from(l.hour) % 24;
        grid[usize::from(q)][hr] += stop - start;
        start = stop;
    }
}

struct AppStats {
    name: String,
    ms: i64,
    per_bucket: Vec<i64>,
    is_claude: bool,
}

pub struct ActivityScreen {
    store: SharedStore,
    period_nav: PeriodNavState,
    selected_app: Option<String>,
    claude_by: ClaudeGrouping,
    scroll_y: f32,
    viewport_h: f32,

    id_all_chip: Option<WidgetId>,
    id_chips: Vec<(WidgetId, String)>,
    id_app_rows: Vec<(WidgetId, String)>,
    id_claude_ids: Vec<WidgetId>,
}

impl ActivityScreen {
    pub fn new(store: SharedStore) -> Self {
        let now_ms = store.borrow().now_ms();
        Self {
            store,
            period_nav: PeriodNavState::new(PeriodKind::Week, now_ms),
            selected_app: None,
            claude_by: ClaudeGrouping::Project,
            scroll_y: 0.0,
            viewport_h: 720.0,
            id_all_chip: None,
            id_chips: Vec::new(),
            id_app_rows: Vec::new(),
            id_claude_ids: Vec::new(),
        }
    }

    pub fn build_content(
        &mut self,
        tree: &mut Tree,
        _overlay_root: Node,
        parent: Node,
        env: &ScreenEnv<'_>,
    ) -> Option<Node> {
        self.viewport_h = env.height_dip;
        let p = env.palette;

        let (
            now_ms,
            first_day_mon0,
            monitor_enabled,
            period,
            ranked_apps,
            selected,
            active_ms,
            hour_grid,
            quarter_grid,
            detail_rows,
        ) = {
            let st = self.store.borrow();
            let now_ms = st.now_ms();
            let loc = st.system_locale().clone();
            let settings = st
                .get_settings()
                .unwrap_or_else(|_| crate::db::Settings::defaults_for(&loc));
            let first_day_mon0 = settings.first_day_of_week.resolve_mon0(&loc);
            let mon_enabled = st.monitor_status().enabled;
            let raw_p = self.period_nav.period(now_ms, first_day_mon0);
            let earliest = if raw_p.kind == PeriodKind::All {
                st.earliest_activity_start().ok().flatten()
            } else {
                None
            };
            let period = fit_period(raw_p, earliest, now_ms);
            let n_buckets = period.buckets.len();

            // First pass: collect per-app totals & bucket totals via streaming cursor.
            let mut app_map: HashMap<String, AppStats> = HashMap::new();
            let _ = st.visit_activity(period.from, period.to, &mut |a| {
                let is_claude = a.kind == "claude";
                let name = if is_claude {
                    CLAUDE.to_string()
                } else {
                    a.app.clone()
                };
                let full_ov = overlap(a.start, a.end, period.from, period.to);
                if full_ov <= 0 {
                    return;
                }
                let entry = app_map.entry(name.clone()).or_insert_with(|| AppStats {
                    name,
                    ms: 0,
                    per_bucket: vec![0; n_buckets],
                    is_claude,
                });
                entry.ms += full_ov;

                // Bucket index range via binary search on ordered contiguous buckets
                let bi_start = period.buckets.partition_point(|b| b.to <= a.start);
                for (bi, b) in period.buckets.iter().enumerate().skip(bi_start) {
                    if b.from >= a.end {
                        break;
                    }
                    let ov = overlap(a.start, a.end, b.from, b.to);
                    if ov > 0 {
                        entry.per_bucket[bi] += ov;
                    }
                }
            });

            let mut ranked_apps: Vec<AppStats> =
                app_map.into_values().filter(|a| a.ms > 0).collect();
            ranked_apps.sort_by_key(|a| std::cmp::Reverse(a.ms));

            let selected = self
                .selected_app
                .clone()
                .filter(|s| ranked_apps.iter().any(|a| &a.name == s));

            // Second pass (streaming): heatmap grids + detail rows + filtered active intervals.
            let mut hour_grid = [[0i64; 24]; 7];
            let mut quarter_grid = [[0i64; 24]; 4];
            let is_day = period.kind == PeriodKind::Day;
            let mut filtered_intervals: Vec<(i64, i64)> = Vec::new();
            let mut details: HashMap<
                (String, String),
                (String, String, i64, crate::ui::theme::Color),
            > = HashMap::new();

            let claude_by = self.claude_by;
            let sel_ref = selected.as_deref();
            let _ = st.visit_activity(period.from, period.to, &mut |a| {
                let name = if a.kind == "claude" {
                    CLAUDE
                } else {
                    a.app.as_str()
                };
                let matches = match sel_ref {
                    Some(target) => name == target,
                    None => a.kind == "app",
                };
                if !matches {
                    return;
                }
                let cs = a.start.max(period.from);
                let ce = a.end.min(period.to);
                if ce <= cs {
                    return;
                }
                if sel_ref.is_some() {
                    filtered_intervals.push((cs, ce));
                }
                if is_day {
                    add_to_quarter_grid(&mut quarter_grid, cs, ce);
                } else {
                    add_to_hour_grid(&mut hour_grid, cs, ce);
                }

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
                } else if sel_ref.is_some() {
                    "No window title".into()
                } else {
                    a.app.clone()
                };
                let sub = if is_claude {
                    match claude_by {
                        ClaudeGrouping::Project => String::new(),
                        ClaudeGrouping::Session => a.context.clone(),
                    }
                } else if sel_ref.is_some() {
                    a.context.clone()
                } else if !a.detail.is_empty() {
                    a.app.clone()
                } else {
                    String::new()
                };
                let entry = details
                    .entry((label.clone(), sub.clone()))
                    .or_insert_with(|| (label, sub, 0, app_color(name)));
                entry.2 += ce - cs;
            });

            let active_ms = if selected.is_some() {
                union_ms(&filtered_intervals, period.from, period.to)
            } else {
                st.active_union_ms(period.from, period.to).unwrap_or(0)
            };

            let mut detail_rows: Vec<(String, String, i64, crate::ui::theme::Color)> =
                details.into_values().filter(|d| d.2 >= 30_000).collect();
            detail_rows.sort_by_key(|d| std::cmp::Reverse(d.2));
            detail_rows.truncate(14);

            (
                now_ms,
                first_day_mon0,
                mon_enabled,
                period,
                ranked_apps,
                selected,
                active_ms,
                hour_grid,
                quarter_grid,
                detail_rows,
            )
        };
        let _ = first_day_mon0;

        let (_vp, col) = build_scroll_page(tree, parent, 1140.0, 28.0, 16.0, self.scroll_y);

        // Header
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
        let headline = match selected.as_deref() {
            Some(app) => format!("{} in {app}", duration(active_ms)),
            None => format!("{} active", duration(active_ms)),
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
        self.period_nav
            .build(tree, head, now_ms, first_day_mon0, env);

        if !monitor_enabled {
            let banner = build_card(tree, col, 14.0, 4.0, p);
            add_label(
                tree,
                banner,
                strings::ACT_EMPTY_DISABLED,
                13.0,
                Weight::Medium,
                Align::Left,
                p.attn,
            );
        }

        // App filter chips
        let chips_row = tree.add_container(
            col,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                align_items: Some(AlignItems::Center),
                gap: style::gap(6.0),
                flex_shrink: 0.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
        );
        tree.set_a11y(
            chips_row,
            A11yInfo {
                role: A11yRole::Group,
                name: "Filter by app".into(),
                requires_name: true,
                ..Default::default()
            },
        );
        let mut all_btn = ButtonProps::new("All apps");
        all_btn.variant = if selected.is_none() {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Secondary
        };
        all_btn.size = ButtonSize::Small;
        all_btn.interact = Interact::from_state(self.id_all_chip, env.hover, env.pressed, false);
        let (all_node, all_id) = button::build(tree, chips_row, &all_btn, p);
        if let Some(d) = tree.data_mut(all_node) {
            d.a11y.selected = Some(selected.is_none());
        }
        self.id_all_chip = Some(all_id);

        self.id_chips.clear();
        for a in ranked_apps.iter().take(8) {
            let is_sel = selected.as_deref() == Some(a.name.as_str());
            let mut cp = ButtonProps::new(&a.name);
            cp.variant = if is_sel {
                ButtonVariant::Primary
            } else {
                ButtonVariant::Secondary
            };
            cp.size = ButtonSize::Small;
            let prev_id = self
                .id_chips
                .iter()
                .find(|(_, n)| n == &a.name)
                .map(|(id, _)| *id);
            cp.interact = Interact::from_state(prev_id, env.hover, env.pressed, false);
            let (cn, cid) = button::build(tree, chips_row, &cp, p);
            if let Some(d) = tree.data_mut(cn) {
                d.a11y.selected = Some(is_sel);
            }
            self.id_chips.push((cid, a.name.clone()));
        }

        if ranked_apps.is_empty() {
            self.id_app_rows.clear();
            self.id_claude_ids.clear();
            build_empty_state(
                tree,
                col,
                glyph::ACTIVITY,
                strings::ACT_EMPTY,
                "Focus records active windows and Claude Code sessions while the monitor is enabled.",
                p,
            );
            return None;
        }

        // Top 2-column grid: Heatmap + StackedBars
        let grid_top = tree.add_container(
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
        let heat_col = tree.add_container(
            grid_top,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                ..Default::default()
            },
        );
        let heat_card = build_card(tree, heat_col, 18.0, 12.0, p);
        let is_day = period.kind == PeriodKind::Day;
        let heat_title = if is_day {
            strings::ACT_DAY_HEATMAP_TITLE
        } else {
            strings::ACT_HEATMAP_TITLE
        };
        add_label(
            tree,
            heat_card,
            heat_title,
            14.0,
            Weight::SemiBold,
            Align::Left,
            p.text,
        );

        let heat_accent = selected.as_deref().map(app_color).unwrap_or(p.accent);
        let heat_body = tree.add_container(
            heat_card,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::FlexStart),
                gap: style::gap(8.0),
                ..Default::default()
            },
        );
        let row_labels_col = tree.add_container(
            heat_body,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                gap: style::gap(4.0),
                size: Size {
                    width: length(32.0_f32),
                    height: auto(),
                },
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        if is_day {
            let qmax = quarter_grid
                .iter()
                .flatten()
                .copied()
                .max()
                .unwrap_or(1)
                .max(1) as f32;
            let norm_rows: Vec<[f32; 24]> = quarter_grid
                .iter()
                .map(|r| r.map(|v| (v as f32) / qmax))
                .collect();
            let row_refs: Vec<&[f32]> = norm_rows.iter().map(|r| r.as_slice()).collect();
            for lbl in [":00", ":15", ":30", ":45"] {
                let cell = tree.add_container(
                    row_labels_col,
                    Style {
                        size: style::size(32.0, 15.0),
                        justify_content: Some(JustifyContent::Center),
                        ..Default::default()
                    },
                );
                add_label(tree, cell, lbl, 10.5, Weight::Regular, Align::Left, p.muted);
            }
            charts::heatmap(
                tree,
                heat_body,
                heat_title,
                &row_refs,
                heat_accent,
                (15.0, 4.0),
                p,
            );
        } else {
            let hmax = hour_grid
                .iter()
                .flatten()
                .copied()
                .max()
                .unwrap_or(1)
                .max(1) as f32;
            let norm_rows: Vec<[f32; 24]> = hour_grid
                .iter()
                .map(|r| r.map(|v| (v as f32) / hmax))
                .collect();
            let row_refs: Vec<&[f32]> = norm_rows.iter().map(|r| r.as_slice()).collect();
            for wd in 0..7_u8 {
                let cell = tree.add_container(
                    row_labels_col,
                    Style {
                        size: style::size(32.0, 15.0),
                        justify_content: Some(JustifyContent::Center),
                        ..Default::default()
                    },
                );
                add_label(
                    tree,
                    cell,
                    weekday_short(wd),
                    10.5,
                    Weight::Regular,
                    Align::Left,
                    p.muted,
                );
            }
            charts::heatmap(
                tree,
                heat_body,
                heat_title,
                &row_refs,
                heat_accent,
                (15.0, 4.0),
                p,
            );
        }

        // Stacked bars card
        let bars_col = tree.add_container(
            grid_top,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                ..Default::default()
            },
        );
        let bars_card = build_card(tree, bars_col, 18.0, 12.0, p);
        add_label(
            tree,
            bars_card,
            strings::ACT_DAILY_BARS_TITLE,
            14.0,
            Weight::SemiBold,
            Align::Left,
            p.text,
        );
        let top_names: HashSet<&str> = ranked_apps
            .iter()
            .filter(|a| !a.is_claude)
            .take(6)
            .map(|a| a.name.as_str())
            .collect();
        let bars_storage: Vec<Vec<(crate::ui::theme::Color, f32)>> = (0..period.buckets.len())
            .map(|bi| {
                if let Some(ref sel_name) = selected {
                    let v = ranked_apps
                        .iter()
                        .find(|a| &a.name == sel_name)
                        .map(|a| a.per_bucket[bi])
                        .unwrap_or(0);
                    vec![(app_color(sel_name), (v as f32) / (HOUR as f32))]
                } else {
                    let mut segs: Vec<(crate::ui::theme::Color, f32)> = ranked_apps
                        .iter()
                        .filter(|a| !a.is_claude && top_names.contains(a.name.as_str()))
                        .map(|a| {
                            (
                                app_color(&a.name),
                                (a.per_bucket[bi] as f32) / (HOUR as f32),
                            )
                        })
                        .collect();
                    let rest: i64 = ranked_apps
                        .iter()
                        .filter(|a| !a.is_claude && !top_names.contains(a.name.as_str()))
                        .map(|a| a.per_bucket[bi])
                        .sum();
                    if rest > 0 {
                        segs.push((
                            crate::ui::theme::rgb(0xC5CCD8),
                            (rest as f32) / (HOUR as f32),
                        ));
                    }
                    segs
                }
            })
            .collect();
        let bar_refs: Vec<&[(crate::ui::theme::Color, f32)]> =
            bars_storage.iter().map(Vec::as_slice).collect();
        let bar_w = ((env.width_dip - 120.0) * 0.45).clamp(240.0, 480.0);
        charts::stacked_bars(
            tree,
            bars_card,
            strings::ACT_DAILY_BARS_TITLE,
            &bar_refs,
            bar_w,
            152.0,
            p,
        );

        // Bottom 2-column grid: Apps w/ sparklines + Where the time went
        let grid_bot = tree.add_container(
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
        let apps_col = tree.add_container(
            grid_bot,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                ..Default::default()
            },
        );
        let apps_card = build_card(tree, apps_col, 18.0, 10.0, p);
        add_label(
            tree,
            apps_card,
            strings::ACT_APPS_TITLE,
            14.0,
            Weight::SemiBold,
            Align::Left,
            p.text,
        );
        let max_app_ms = ranked_apps[0].ms.max(1) as f32;
        let spark_len = period.buckets.len().min(match period.kind {
            PeriodKind::Day => 24,
            PeriodKind::Month => 31,
            _ => 12,
        });
        self.id_app_rows.clear();
        for a in ranked_apps.iter().take(20) {
            let is_sel = selected.as_deref() == Some(a.name.as_str());
            let row_wrap = tree.add_container(
                apps_card,
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    align_items: Some(AlignItems::Center),
                    gap: style::gap(8.0),
                    size: Size {
                        width: percent(1.0_f32),
                        height: auto(),
                    },
                    ..Default::default()
                },
            );
            let row_btn_box = tree.add_container(
                row_wrap,
                Style {
                    flex_grow: 1.0,
                    flex_basis: length(0.0_f32),
                    ..Default::default()
                },
            );
            let prev_id = self
                .id_app_rows
                .iter()
                .find(|(_, n)| n == &a.name)
                .map(|(id, _)| *id);
            let interact = Interact::from_state(prev_id, env.hover, env.pressed, false);
            let c = app_color(&a.name);
            let label = if a.is_claude {
                format!("{} (parallel)", a.name)
            } else {
                a.name.clone()
            };
            let (_, rid) = charts::share_row_button(
                tree,
                row_btn_box,
                &label,
                &duration(a.ms),
                (a.ms as f32) / max_app_ms,
                c,
                is_sel,
                interact,
                p,
            );
            self.id_app_rows.push((rid, a.name.clone()));

            if spark_len >= 2 && a.per_bucket.len() >= 2 {
                let tail = &a.per_bucket[a.per_bucket.len() - spark_len..];
                let fvals: Vec<f32> = tail.iter().map(|&v| v as f32).collect();
                charts::spark(
                    tree,
                    row_wrap,
                    &format!("{} trend", a.name),
                    &fvals,
                    (58.0, 20.0),
                    c,
                    p,
                );
            }
        }

        // Where the time went card
        let where_col = tree.add_container(
            grid_bot,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                ..Default::default()
            },
        );
        let where_card = build_card(tree, where_col, 18.0, 10.0, p);
        let where_head = tree.add_container(
            where_card,
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
        add_label(
            tree,
            where_head,
            strings::ACT_WHERE_TITLE,
            14.0,
            Weight::SemiBold,
            Align::Left,
            p.text,
        );
        self.id_claude_ids.clear();
        if selected.as_deref() == Some(CLAUDE) {
            let opts = [
                strings::ACT_CLAUDE_MODE_PROJECTS,
                strings::ACT_CLAUDE_MODE_SESSIONS,
            ];
            let (_, cids) = segmented::build(
                tree,
                where_head,
                &SegmentedProps {
                    group_name: "Group Claude Code by",
                    options: &opts,
                    selected: usize::from(self.claude_by == ClaudeGrouping::Session),
                    hover: env.hover,
                    disabled: false,
                },
                p,
            );
            self.id_claude_ids = cids;
        }
        if detail_rows.is_empty() {
            add_label(
                tree,
                where_card,
                "No window titles recorded for this selection.",
                12.5,
                Weight::Regular,
                Align::Left,
                p.muted,
            );
        } else {
            let dmax = detail_rows[0].2.max(1) as f32;
            for (lbl, sub, ms, col_c) in &detail_rows {
                let full_lbl = if sub.is_empty() {
                    lbl.clone()
                } else {
                    format!("{lbl} ({sub})")
                };
                charts::share_row(
                    tree,
                    where_card,
                    &full_lbl,
                    &duration(*ms),
                    (*ms as f32) / dmax,
                    *col_c,
                    p,
                );
            }
        }

        None
    }

    pub fn handle_click(&mut self, id: WidgetId, palette: &Palette) -> bool {
        let now_ms = self.store.borrow().now_ms();
        if self.period_nav.on_click(id, now_ms, palette) {
            return true;
        }
        if Some(id) == self.id_all_chip {
            self.selected_app = None;
            return true;
        }
        if let Some((_, app)) = self
            .id_chips
            .iter()
            .chain(self.id_app_rows.iter())
            .find(|(cid, _)| *cid == id)
        {
            if self.selected_app.as_deref() == Some(app.as_str()) {
                self.selected_app = None;
            } else {
                self.selected_app = Some(app.clone());
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
        false
    }

    pub fn handle_escape(&mut self) -> bool {
        if self.selected_app.is_some() {
            self.selected_app = None;
            true
        } else {
            false
        }
    }

    pub fn handle_wheel(&mut self, delta_dip: f32) -> bool {
        let max_scroll = (1150.0 - self.viewport_h).max(0.0);
        scroll_wheel_step(&mut self.scroll_y, delta_dip, max_scroll)
    }

    pub fn reset_transient(&mut self) {
        self.scroll_y = 0.0;
        self.period_nav.clear_popup();
    }
}

impl Screen for ActivityScreen {
    fn build(&mut self, env: &ScreenEnv<'_>) -> BuiltScreen {
        let mut tree = screen_root_tree(env);
        let root = tree.root();
        let modal_root = self.build_content(&mut tree, root, root, env);
        BuiltScreen { tree, modal_root }
    }

    fn on_click(&mut self, id: WidgetId) -> bool {
        self.handle_click(id, &Palette::LIGHT)
    }

    fn on_escape(&mut self) -> bool {
        self.handle_escape()
    }

    fn on_wheel(&mut self, delta_dip: f32) -> bool {
        self.handle_wheel(delta_dip)
    }

    fn on_popup_poll(&mut self) -> bool {
        self.period_nav.poll_popup()
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
    fn activity_screen_validates_a11y_switches_day_quarters_and_claude_grouping() {
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
        let screen = ActivityScreen::new(Rc::clone(&store));
        let mut focus = FocusModel::new();
        let mut host = ScreenHost::mount(Box::new(screen), &env, &mut focus);
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));

        // Filter to Claude Code chip -> reveals Project/Session segmented control.
        let claude_chip = host
            .tree()
            .all_nodes()
            .into_iter()
            .filter_map(|n| host.tree().data(n))
            .find(|d| d.a11y.name == CLAUDE)
            .unwrap()
            .id;
        assert!(host.on_click(claude_chip));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));

        let sess_btn = host
            .tree()
            .all_nodes()
            .into_iter()
            .filter_map(|n| host.tree().data(n))
            .find(|d| d.a11y.name == strings::ACT_CLAUDE_MODE_SESSIONS)
            .unwrap()
            .id;
        assert!(host.on_click(sess_btn));
        assert!(host.rebuild_if_dirty(&env, &mut focus));
        assert_eq!(a11y::validate_tree_names(host.tree()), Ok(()));
    }
}
