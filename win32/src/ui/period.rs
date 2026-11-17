//! Period navigation model (`Day`, `Week`, `Month`, `Year`, `All time`),
//! adaptive all-time bucket fitting, and the shared `PeriodNav` control bar.

use std::cell::RefCell;
use std::rc::Rc;

use taffy::prelude::*;

use crate::format::{month_long, month_short, range_label, weekday_short};
use crate::time::{
    add_days, add_days_start, add_months, iso_week, start_of_day, start_of_month,
    start_of_week_with_first, start_of_year, to_local, HOUR,
};
use crate::ui::screen::ScreenEnv;
use crate::ui::strings;
use crate::ui::text::{Align, Weight};
use crate::ui::theme::Palette;
use crate::ui::tree::{style, A11yInfo, A11yRole, HitMode, Node, PaintStyle, Tree, WidgetId};
use crate::ui::widgets::button::{self, ButtonProps, ButtonSize, ButtonVariant};
use crate::ui::widgets::select::{self, SelectProps};
use crate::ui::widgets::{add_label, glyph, Interact};
use crate::ui::window;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PeriodKind {
    Day,
    #[default]
    Week,
    Month,
    Year,
    All,
}

impl PeriodKind {
    pub const ALL_KINDS: [Self; 5] = [Self::Day, Self::Week, Self::Month, Self::Year, Self::All];

    pub fn index(self) -> usize {
        match self {
            Self::Day => 0,
            Self::Week => 1,
            Self::Month => 2,
            Self::Year => 3,
            Self::All => 4,
        }
    }

    pub fn from_index(idx: usize) -> Option<Self> {
        Self::ALL_KINDS.get(idx).copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BucketUnit {
    Hour,
    Day,
    Month,
    Year,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeriodBucket {
    pub from: i64,
    pub to: i64,
    pub label: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Period {
    pub kind: PeriodKind,
    pub from: i64,
    pub to: i64,
    pub unit: BucketUnit,
    pub buckets: Vec<PeriodBucket>,
}

fn month_bucket(from: i64, label: String) -> PeriodBucket {
    let l = to_local(from);
    PeriodBucket {
        from,
        to: start_of_day(add_months(from, 1)),
        label,
        title: format!("{} {}", month_long(l.month), l.year),
    }
}

/// Builds a `Period` centered on `anchor_ms`. `All` yields an open query range
/// `[0, end of current month)` until `fit_period` shapes the chart buckets.
pub fn build_period(kind: PeriodKind, anchor_ms: i64, now_ms: i64, first_day_mon0: u8) -> Period {
    match kind {
        PeriodKind::All => Period {
            kind,
            from: 0,
            to: start_of_day(add_months(start_of_month(now_ms), 1)),
            unit: BucketUnit::Month,
            buckets: Vec::new(),
        },
        PeriodKind::Day => {
            let from = start_of_day(anchor_ms);
            let to = add_days_start(from, 1);
            let buckets = (0..24)
                .map(|h| PeriodBucket {
                    from: from + i64::from(h) * HOUR,
                    to: from + i64::from(h + 1) * HOUR,
                    label: if h % 3 == 0 {
                        format!("{h:02}")
                    } else {
                        String::new()
                    },
                    title: format!("{h:02}:00 – {:02}:00", h + 1),
                })
                .collect();
            Period {
                kind,
                from,
                to,
                unit: BucketUnit::Hour,
                buckets,
            }
        }
        PeriodKind::Year => {
            let from = start_of_year(anchor_ms);
            let buckets = (0..12)
                .map(|i| {
                    let a = add_months(from, i);
                    let m = to_local(a).month;
                    month_bucket(a, month_short(m).to_string())
                })
                .collect();
            Period {
                kind,
                from,
                to: add_months(from, 12),
                unit: BucketUnit::Month,
                buckets,
            }
        }
        PeriodKind::Week | PeriodKind::Month => {
            let from = if kind == PeriodKind::Week {
                start_of_week_with_first(anchor_ms, first_day_mon0)
            } else {
                start_of_month(anchor_ms)
            };
            let to = if kind == PeriodKind::Week {
                add_days_start(from, 7)
            } else {
                start_of_day(add_months(from, 1))
            };
            let mut buckets = Vec::new();
            let mut d = from;
            while d < to {
                let next = add_days_start(d, 1);
                let l = to_local(d);
                let label = if kind == PeriodKind::Week {
                    weekday_short(l.weekday_mon0()).to_string()
                } else {
                    l.day.to_string()
                };
                let title = format!(
                    "{}, {} {}",
                    weekday_short(l.weekday_mon0()),
                    l.day,
                    month_short(l.month)
                );
                buckets.push(PeriodBucket {
                    from: d,
                    to: next,
                    label,
                    title,
                });
                d = next;
            }
            Period {
                kind,
                from,
                to,
                unit: BucketUnit::Day,
                buckets,
            }
        }
    }
}

/// Adapts `PeriodKind::All` buckets once the screen knows its earliest timestamp:
/// over 48 months becomes yearly bars, over 24 labels Januaries, 17 to 24 labels
/// quarters, and up to 16 labels every month. Other period kinds pass through.
pub fn fit_period(p: Period, earliest: Option<i64>, now_ms: i64) -> Period {
    if p.kind != PeriodKind::All {
        return p;
    }
    let end = p.to;
    let mut from = start_of_month(earliest.unwrap_or(now_ms).min(now_ms));
    let mut months = 0usize;
    let mut probe = from;
    while probe < end {
        months += 1;
        probe = add_months(probe, 1);
    }
    if months > 48 {
        from = start_of_year(from);
        let mut buckets = Vec::new();
        let mut y = from;
        while y < end {
            let yr = to_local(y).year.to_string();
            let next = add_months(y, 12);
            buckets.push(PeriodBucket {
                from: y,
                to: next,
                label: yr.clone(),
                title: yr,
            });
            y = next;
        }
        return Period {
            kind: p.kind,
            from,
            to: end,
            unit: BucketUnit::Year,
            buckets,
        };
    }
    let quarters = months > 16 && months <= 24;
    let years = months > 24;
    if quarters {
        let q_rem = (to_local(from).month.saturating_sub(1) % 3) as i32;
        if q_rem > 0 {
            from = add_months(from, -q_rem);
        }
    }
    let mut buckets = Vec::new();
    let mut m = from;
    while m < end {
        let d = to_local(m);
        let jan = d.month == 1;
        let label = if years {
            if jan {
                d.year.to_string()
            } else {
                String::new()
            }
        } else if quarters && !(d.month - 1).is_multiple_of(3) {
            String::new()
        } else if jan || m == from {
            format!("{} {:02}", month_short(d.month), d.year.rem_euclid(100))
        } else {
            month_short(d.month).to_string()
        };
        buckets.push(month_bucket(m, label));
        m = add_months(m, 1);
    }
    Period {
        kind: p.kind,
        from,
        to: end,
        unit: BucketUnit::Month,
        buckets,
    }
}

pub fn period_title(p: &Period, now_ms: i64) -> String {
    if p.kind == PeriodKind::All {
        return strings::PERIOD_ALL.into();
    }
    let current = now_ms >= p.from && now_ms < p.to;
    match p.kind {
        PeriodKind::Day => {
            if current {
                strings::PERIOD_TODAY.into()
            } else {
                range_label(p.from, p.to)
            }
        }
        PeriodKind::Week => {
            let w = iso_week(p.from);
            if current {
                format!("{} · W{w}", strings::PERIOD_THIS_WEEK_PREFIX)
            } else {
                format!("{} · W{w}", range_label(p.from, p.to))
            }
        }
        PeriodKind::Month => {
            let l = to_local(p.from);
            format!("{} {}", month_long(l.month), l.year)
        }
        PeriodKind::Year => to_local(p.from).year.to_string(),
        PeriodKind::All => strings::PERIOD_ALL.into(),
    }
}

/// Interactive state and widget IDs for `PeriodNav`.
#[derive(Default)]
pub struct PeriodNavState {
    pub kind: PeriodKind,
    pub anchor_ms: i64,
    prev_id: Option<WidgetId>,
    title_id: Option<WidgetId>,
    next_id: Option<WidgetId>,
    kind_select_id: Option<WidgetId>,
    popup_result: Option<Rc<RefCell<Option<usize>>>>,
}

impl PeriodNavState {
    pub fn new(kind: PeriodKind, now_ms: i64) -> Self {
        Self {
            kind,
            anchor_ms: now_ms,
            ..Default::default()
        }
    }

    pub fn period(&self, now_ms: i64, first_day_mon0: u8) -> Period {
        build_period(self.kind, self.anchor_ms, now_ms, first_day_mon0)
    }

    pub fn step(&mut self, dir: i32) {
        self.anchor_ms = match self.kind {
            PeriodKind::All => self.anchor_ms,
            PeriodKind::Day => add_days(self.anchor_ms, dir),
            PeriodKind::Week => add_days(self.anchor_ms, 7 * dir),
            PeriodKind::Month => add_months(start_of_month(self.anchor_ms), dir),
            PeriodKind::Year => add_months(start_of_year(self.anchor_ms), 12 * dir),
        };
    }

    pub fn reset(&mut self, now_ms: i64) {
        self.anchor_ms = now_ms;
    }

    pub fn set_kind(&mut self, kind: PeriodKind) {
        self.kind = kind;
    }

    /// Builds `< Period title >` + `Period length` dropdown into `parent`.
    pub fn build(
        &mut self,
        tree: &mut Tree,
        parent: Node,
        now_ms: i64,
        first_day_mon0: u8,
        env: &ScreenEnv<'_>,
    ) {
        let hover = env.hover;
        let pressed = env.pressed;
        let palette = env.palette;
        let p = self.period(now_ms, first_day_mon0);
        let all = self.kind == PeriodKind::All;
        let current = all || (now_ms >= p.from && now_ms < p.to);

        let wrap = tree.add_container(
            parent,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                gap: style::gap(8.0),
                ..Default::default()
            },
        );

        let pill = tree.add_container(
            wrap,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                size: Size {
                    width: auto(),
                    height: length(32.0_f32),
                },
                padding: style::pad(2.0, 2.0, 2.0, 2.0),
                ..Default::default()
            },
        );
        tree.set_paint(
            pill,
            PaintStyle {
                bg: Some(palette.ctl_bg),
                border_color: Some(palette.ctl_border),
                border_width: palette.border_width,
                radius: 6.0,
                clip: false,
            },
        );
        tree.set_a11y(
            pill,
            A11yInfo {
                role: A11yRole::Group,
                name: strings::PERIOD_GROUP_LABEL.into(),
                requires_name: true,
                ..Default::default()
            },
        );

        let mut prev_props = ButtonProps::new("");
        prev_props.aria_label = Some(strings::PERIOD_PREV_LABEL);
        prev_props.icon = Some(glyph::CHEVRON_LEFT);
        prev_props.variant = ButtonVariant::Ghost;
        prev_props.size = ButtonSize::Small;
        prev_props.disabled = all;
        prev_props.interact = Interact::from_state(self.prev_id, hover, pressed, all);
        let (_, prev_id) = button::build(tree, pill, &prev_props, palette);
        self.prev_id = Some(prev_id);

        let title = period_title(&p, now_ms);
        let title_node = tree.add_container(
            pill,
            Style {
                display: Display::Flex,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                padding: style::pad(10.0, 0.0, 10.0, 0.0),
                size: Size {
                    width: length(200.0_f32),
                    height: length(26.0_f32),
                },
                ..Default::default()
            },
        );
        let title_id = tree.id(title_node);
        let title_hover = !current && hover == Some(title_id);
        if title_hover {
            tree.set_paint(
                title_node,
                PaintStyle {
                    bg: Some(palette.ctl_hover),
                    border_color: None,
                    border_width: 0.0,
                    radius: 4.0,
                    clip: false,
                },
            );
        }
        tree.set_hit(title_node, HitMode::Interactive);
        tree.set_focusable(title_node, !current);
        tree.set_disabled(title_node, current);
        tree.set_a11y(
            title_node,
            A11yInfo {
                role: A11yRole::Button,
                name: if current {
                    title.clone()
                } else {
                    format!("{title}, {}", strings::PERIOD_RESET_HINT)
                },
                disabled: current,
                clickable: true,
                requires_name: true,
                ..Default::default()
            },
        );
        add_label(
            tree,
            title_node,
            &title,
            12.5,
            Weight::Medium,
            Align::Center,
            palette.text,
        );
        self.title_id = Some(title_id);

        let mut next_props = ButtonProps::new("");
        next_props.aria_label = Some(strings::PERIOD_NEXT_LABEL);
        next_props.icon = Some(glyph::CHEVRON_RIGHT);
        next_props.variant = ButtonVariant::Ghost;
        next_props.size = ButtonSize::Small;
        next_props.disabled = all;
        next_props.interact = Interact::from_state(self.next_id, hover, pressed, all);
        let (_, next_id) = button::build(tree, pill, &next_props, palette);
        self.next_id = Some(next_id);

        let options: [&str; 5] = [
            strings::PERIOD_DAY,
            strings::PERIOD_WEEK,
            strings::PERIOD_MONTH,
            strings::PERIOD_YEAR,
            strings::PERIOD_ALL,
        ];
        let sel_interact = Interact::from_state(self.kind_select_id, hover, pressed, false);
        let (_, kind_id) = select::build(
            tree,
            wrap,
            &SelectProps {
                aria_label: strings::PERIOD_LENGTH_LABEL,
                options: &options,
                selected: Some(self.kind.index()),
                placeholder: strings::SELECT_PLACEHOLDER,
                min_width: 112.0,
                open: self.popup_result.is_some(),
                disabled: false,
                interact: sel_interact,
            },
            palette,
        );
        self.kind_select_id = Some(kind_id);
    }

    pub fn on_click(&mut self, id: WidgetId, now_ms: i64, palette: &Palette) -> bool {
        if Some(id) == self.prev_id {
            self.step(-1);
            return true;
        }
        if Some(id) == self.next_id {
            self.step(1);
            return true;
        }
        if Some(id) == self.title_id {
            self.reset(now_ms);
            return true;
        }
        if Some(id) == self.kind_select_id {
            if let (Some(hwnd), Some(rect)) = (window::hwnd(), window::widget_rect(id)) {
                let options: [&str; 5] = [
                    strings::PERIOD_DAY,
                    strings::PERIOD_WEEK,
                    strings::PERIOD_MONTH,
                    strings::PERIOD_YEAR,
                    strings::PERIOD_ALL,
                ];
                self.popup_result =
                    select::open_popup(hwnd, rect, &options, Some(self.kind.index()), palette);
            }
            return true;
        }
        false
    }

    pub fn poll_popup(&mut self) -> bool {
        let Some(res) = self.popup_result.as_ref() else {
            return false;
        };
        let picked = *res.borrow();
        if let Some(idx) = picked {
            self.popup_result = None;
            if let Some(kind) = PeriodKind::from_index(idx) {
                self.set_kind(kind);
                return true;
            }
        }
        if !crate::ui::popup::is_open() {
            self.popup_result = None;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::from_local;
    use crate::ui::a11y;
    use crate::ui::text::TextSystem;

    #[test]
    fn build_period_handles_day_week_month_year_and_first_day_pref() {
        // Wed 2026-11-18 12:00 local
        let now = from_local(2026, 11, 18, 12, 0, 0, 0);

        let day = build_period(PeriodKind::Day, now, now, 0);
        assert_eq!(day.buckets.len(), 24);
        assert_eq!(day.unit, BucketUnit::Hour);
        assert_eq!(period_title(&day, now), "Today");

        let mon_week = build_period(PeriodKind::Week, now, now, 0);
        assert_eq!(mon_week.buckets.len(), 7);
        assert_eq!(mon_week.buckets[0].label, "Mon");
        assert!(period_title(&mon_week, now).starts_with("This week"));

        let sun_week = build_period(PeriodKind::Week, now, now, 6);
        assert_eq!(sun_week.buckets.len(), 7);
        assert_eq!(sun_week.buckets[0].label, "Sun");

        let month = build_period(PeriodKind::Month, now, now, 0);
        assert_eq!(month.buckets.len(), 30);
        assert_eq!(period_title(&month, now), "November 2026");

        let year = build_period(PeriodKind::Year, now, now, 0);
        assert_eq!(year.buckets.len(), 12);
        assert_eq!(period_title(&year, now), "2026");
    }

    #[test]
    fn fit_period_switches_all_time_from_months_to_quarters_years_and_multiyear() {
        let now = from_local(2026, 11, 18, 12, 0, 0, 0);
        let raw = build_period(PeriodKind::All, now, now, 0);

        // ~6 months -> monthly bars with month labels
        let earliest_6m = from_local(2026, 6, 1, 0, 0, 0, 0);
        let fit6 = fit_period(raw.clone(), Some(earliest_6m), now);
        assert_eq!(fit6.unit, BucketUnit::Month);
        assert_eq!(fit6.buckets.len(), 6);
        assert!(fit6.buckets.iter().all(|b| !b.label.is_empty()));

        // ~20 months -> quarterly labels (most non-quarter labels blank)
        let earliest_20m = from_local(2025, 4, 1, 0, 0, 0, 0);
        let fit20 = fit_period(raw.clone(), Some(earliest_20m), now);
        assert_eq!(fit20.unit, BucketUnit::Month);
        assert!(fit20.buckets.iter().any(|b| b.label.is_empty()));

        // ~60 months -> yearly bars
        let earliest_5y = from_local(2021, 11, 1, 0, 0, 0, 0);
        let fit60 = fit_period(raw, Some(earliest_5y), now);
        assert_eq!(fit60.unit, BucketUnit::Year);
        assert!(fit60.buckets.len() >= 5);
    }

    #[test]
    fn period_nav_widget_passes_a11y_and_steps_periods() {
        let text = TextSystem::new().expect("dwrite");
        let now = from_local(2026, 11, 18, 12, 0, 0, 0);
        let env = ScreenEnv {
            width_dip: 800.0,
            height_dip: 120.0,
            hover: None,
            pressed: None,
            focused: None,
            palette: &Palette::LIGHT,
            text: &text,
        };
        let mut nav = PeriodNavState::new(PeriodKind::Week, now);
        let mut tree = Tree::new(style::column());
        let root = tree.root();
        nav.build(&mut tree, root, now, 0, &env);
        tree.layout(800.0, 120.0, &text);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));

        let prev_id = nav.prev_id.unwrap();
        assert!(nav.on_click(prev_id, now, &Palette::LIGHT));
        assert_ne!(nav.anchor_ms, now);
        let title_id = nav.title_id.unwrap();
        assert!(nav.on_click(title_id, now, &Palette::LIGHT));
        assert_eq!(nav.anchor_ms, now);
    }
}
