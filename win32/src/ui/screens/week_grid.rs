//! Interactive 7-day × 24-hour calendar grid rendered inside one high-speed `Canvas`
//! for 60 fps drag previews (create, move across days, resize, Alt disables 15m snap,
//! Esc cancels drag, 60Hz edge autoscroll).

use std::collections::HashMap;

use taffy::prelude::*;

use crate::db::{Activity, EntryPatch, Project, TimeEntry};
use crate::format::{self, duration, weekday_short};
use crate::stats::{entry_ms, union_ms};
use crate::time::{
    add_days_start, at_minutes, minutes_of_day, same_day, start_of_day, to_local, MIN,
};
use crate::ui::input::KeyCode;
use crate::ui::screen::{KeyModifiers, PointerReply, ScreenEnv};
use crate::ui::strings;
use crate::ui::text::{Align, Weight, Wrap};
use crate::ui::theme::{app_color, mix, parse_hex_color, Palette, NO_PROJECT};
use crate::ui::tree::{
    style, A11yInfo, A11yRole, DrawCmd, HitMode, Leaf, Node, PaintStyle, Rect, Tree, WidgetId,
};
use crate::ui::widgets::add_label;
use crate::ui::window;

use super::calendar_layout::{
    activity_sessions, layout_day, session_label, ActivitySession, DIP_PER_MIN, HOUR_DIP, SNAP_MIN,
};
use super::entry_editor::EntryDraft;

const GUTTER_W: f32 = 52.0;
const DAY_MIN_MAX: f64 = 24.0 * 60.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CalendarMode {
    #[default]
    Mix,
    Time,
    Activity,
}

impl CalendarMode {
    pub fn index(self) -> usize {
        match self {
            Self::Mix => 0,
            Self::Time => 1,
            Self::Activity => 2,
        }
    }

    pub fn from_index(idx: usize) -> Self {
        match idx {
            1 => Self::Time,
            2 => Self::Activity,
            _ => Self::Mix,
        }
    }
}

#[derive(Clone, Debug)]
enum GridDrag {
    Create {
        day_idx: usize,
        anchor_min: f64,
        cur_min: f64,
        alt: bool,
    },
    Move {
        entry: TimeEntry,
        grab_min: f64,
        grab_day: usize,
        day_idx: usize,
        orig_start_min: f64,
        cur_start_min: f64,
        duration_ms: i64,
        moved: bool,
        alt: bool,
        anchor_rect: Rect,
    },
    Resize {
        entry: TimeEntry,
        #[allow(dead_code)]
        day_idx: usize,
        cur_end_min: f64,
        moved: bool,
        alt: bool,
    },
    ClickActivity {
        session: ActivitySession,
        anchor_rect: Rect,
    },
}

#[derive(Clone, Debug)]
pub enum GridAction {
    OpenEntry(EntryDraft, #[allow(dead_code)] Rect),
    UpdateEntry { id: String, patch: EntryPatch },
    OpenActivity(ActivitySession, Rect),
}

pub struct WeekGridState {
    pub mode: CalendarMode,
    pub scroll_y: f32,
    initial_scroll_done: bool,
    last_scroll_to_hour: u32,
    last_week_start: i64,

    // Snapshot of latest week data for fast drag redraws without querying SQLite per frame.
    week_days: [i64; 7],
    entries: Vec<TimeEntry>,
    sessions_by_day: [Vec<ActivitySession>; 7],
    projects_by_id: HashMap<String, Project>,
    now_ms: i64,
    is_24h: bool,
    palette_cache: Palette,

    // Layout geometry
    viewport_node: Option<Node>,
    canvas_node: Option<Node>,
    pub canvas_id: Option<WidgetId>,
    viewport_id: Option<WidgetId>,
    last_viewport_rect: Rect,
    last_canvas_w: f32,
    canvas_focused: bool,

    // Keyboard grid cursor & interaction
    cursor_day: usize,
    cursor_min: i32,
    drag: Option<GridDrag>,
    last_pointer_dip: Option<(f32, f32)>,
    pub pending_action: Option<GridAction>,
    pub drag_frames_rendered: u64,
}

impl Default for WeekGridState {
    fn default() -> Self {
        Self::new()
    }
}

impl WeekGridState {
    pub fn new() -> Self {
        Self {
            mode: CalendarMode::Mix,
            scroll_y: (8.0 * HOUR_DIP - 8.0).max(0.0),
            initial_scroll_done: false,
            last_scroll_to_hour: 8,
            last_week_start: 0,
            week_days: [0; 7],
            entries: Vec::new(),
            sessions_by_day: Default::default(),
            projects_by_id: HashMap::new(),
            now_ms: 0,
            is_24h: true,
            palette_cache: Palette::LIGHT,
            viewport_node: None,
            canvas_node: None,
            canvas_id: None,
            viewport_id: None,
            last_viewport_rect: Rect {
                x: 200.0,
                y: 160.0,
                w: 900.0,
                h: 560.0,
            },
            last_canvas_w: 900.0,
            canvas_focused: false,
            cursor_day: 0,
            cursor_min: 9 * 60,
            drag: None,
            last_pointer_dip: None,
            pending_action: None,
            drag_frames_rendered: 0,
        }
    }

    pub fn is_dragging(&self) -> bool {
        matches!(
            self.drag,
            Some(GridDrag::Create { .. } | GridDrag::Move { .. } | GridDrag::Resize { .. })
        )
    }

    pub fn set_canvas_focused(&mut self, focused: bool) {
        self.canvas_focused = focused;
    }

    fn snap_min(min: f64, disable_snap: bool) -> f64 {
        let clamped = min.clamp(0.0, DAY_MIN_MAX);
        if disable_snap {
            clamped.round()
        } else {
            ((clamped / f64::from(SNAP_MIN)).round() * f64::from(SNAP_MIN)).clamp(0.0, DAY_MIN_MAX)
        }
    }

    fn seg_end_minutes(seg_end: i64, day_end: i64) -> f64 {
        if seg_end >= day_end {
            DAY_MIN_MAX
        } else {
            minutes_of_day(seg_end)
        }
    }

    fn viewport_rect(&self) -> Rect {
        self.viewport_id
            .and_then(window::widget_rect)
            .unwrap_or(self.last_viewport_rect)
    }

    fn canvas_rect(&self) -> Rect {
        self.canvas_id
            .and_then(window::widget_rect)
            .unwrap_or(Rect {
                x: self.last_viewport_rect.x,
                y: self.last_viewport_rect.y - self.scroll_y,
                w: self.last_canvas_w,
                h: 24.0 * HOUR_DIP,
            })
    }

    fn locate_local(&self, x_dip: f32, y_dip: f32) -> (usize, f64, f32, f32) {
        let vp = self.viewport_rect();
        let cr = self.canvas_rect();
        let canvas_w = cr.w.max(200.0);
        let col_w = ((canvas_w - GUTTER_W) / 7.0).max(20.0);
        let lx = x_dip - cr.x;
        let ly = y_dip - vp.y + self.scroll_y;
        let day_idx = ((lx - GUTTER_W) / col_w).floor().clamp(0.0, 6.0) as usize;
        let min = (ly / f64::from(DIP_PER_MIN) as f32).clamp(0.0, DAY_MIN_MAX as f32) as f64;
        (day_idx, min, lx, ly)
    }

    fn col_geometry(&self, canvas_w: f32, day_idx: usize) -> (f32, f32, f32, f32) {
        let col_w = ((canvas_w - GUTTER_W) / 7.0).max(20.0);
        let col_x = GUTTER_W + (day_idx as f32) * col_w;
        match self.mode {
            CalendarMode::Time => (col_x, col_w, col_w, 0.0),
            CalendarMode::Activity => (col_x, col_w, 0.0, col_w),
            CalendarMode::Mix => {
                let strip_w = (col_w * 0.16).clamp(8.0, 14.0);
                let lane_w = (col_w - strip_w - 2.0).max(16.0);
                (col_x, col_w, lane_w, strip_w)
            }
        }
    }

    fn entry_box(
        &self,
        canvas_w: f32,
        day_idx: usize,
        seg: &super::calendar_layout::EntrySeg,
        day_end: i64,
    ) -> Rect {
        let (col_x, _, lane_w, _) = self.col_geometry(canvas_w, day_idx);
        let sub_w = lane_w / (seg.cols.max(1) as f32);
        let x = col_x + 2.0 + (seg.col as f32) * sub_w;
        let w = (sub_w - 4.0).max(8.0);
        let top_min = minutes_of_day(seg.start) as f32;
        let end_min = Self::seg_end_minutes(seg.end, day_end) as f32;
        let y = top_min * DIP_PER_MIN + 1.0;
        let h = (((end_min - top_min) * DIP_PER_MIN) - 3.0).max(14.0);
        Rect { x, y, w, h }
    }

    fn session_box(
        &self,
        canvas_w: f32,
        day_idx: usize,
        s: &ActivitySession,
        day_end: i64,
    ) -> Rect {
        let (col_x, col_w, _, strip_w) = self.col_geometry(canvas_w, day_idx);
        let (x, w) = if self.mode == CalendarMode::Activity {
            (col_x + 2.0, (col_w - 4.0).max(12.0))
        } else {
            (col_x + col_w - strip_w - 1.0, strip_w)
        };
        let top_min = minutes_of_day(s.start) as f32;
        let end_min = Self::seg_end_minutes(s.end, day_end) as f32;
        let y = top_min * DIP_PER_MIN + 1.0;
        let h = (((end_min - top_min) * DIP_PER_MIN) - 2.0).max(6.0);
        Rect { x, y, w, h }
    }

    fn preview_entries(&self) -> Vec<TimeEntry> {
        self.entries
            .iter()
            .map(|e| {
                if let Some(GridDrag::Move {
                    entry: ref dragged,
                    day_idx,
                    cur_start_min,
                    duration_ms,
                    moved: true,
                    ..
                }) = self.drag
                {
                    if dragged.id == e.id {
                        let start =
                            at_minutes(self.week_days[day_idx], cur_start_min.round() as i32);
                        let mut cloned = e.clone();
                        cloned.start = start;
                        cloned.end = Some(start + duration_ms);
                        return cloned;
                    }
                }
                if let Some(GridDrag::Resize {
                    entry: ref dragged,
                    cur_end_min,
                    moved: true,
                    ..
                }) = self.drag
                {
                    if dragged.id == e.id {
                        let base_day = start_of_day(dragged.end.unwrap_or(self.now_ms) - 1);
                        let new_end = at_minutes(base_day, cur_end_min.round() as i32);
                        if new_end > dragged.start {
                            let mut cloned = e.clone();
                            cloned.end = Some(new_end);
                            return cloned;
                        }
                    }
                }
                e.clone()
            })
            .collect()
    }

    fn render_canvas_cmds(&self, canvas_w: f32, p: &Palette) -> Vec<DrawCmd> {
        let total_h = 24.0 * HOUR_DIP;
        let col_w = ((canvas_w - GUTTER_W) / 7.0).max(20.0);
        let mut cmds = Vec::with_capacity(256);

        // Today column tint
        for (di, &day) in self.week_days.iter().enumerate() {
            if same_day(day, self.now_ms) {
                cmds.push(DrawCmd::FillRect {
                    rect: Rect {
                        x: GUTTER_W + (di as f32) * col_w,
                        y: 0.0,
                        w: col_w,
                        h: total_h,
                    },
                    color: mix(p.accent, p.surface, 0.045),
                    radius: 0.0,
                });
            }
        }

        // Hour horizontal hairlines + gutter labels
        for h in 0..24 {
            let y = (h as f32) * HOUR_DIP;
            cmds.push(DrawCmd::Line {
                x1: GUTTER_W,
                y1: y,
                x2: canvas_w,
                y2: y,
                width: 1.0,
                color: p.hairline,
            });
            if h > 0 {
                let lbl = if self.is_24h {
                    format!("{h:02}:00")
                } else {
                    let sfx = if h >= 12 { "PM" } else { "AM" };
                    let h12 = match h % 12 {
                        0 => 12,
                        v => v,
                    };
                    format!("{h12} {sfx}")
                };
                cmds.push(DrawCmd::Text {
                    rect: Rect {
                        x: 2.0,
                        y: y - 8.0,
                        w: GUTTER_W - 8.0,
                        h: 16.0,
                    },
                    text: lbl,
                    size: 11.0,
                    weight: Weight::Regular,
                    align: Align::Right,
                    wrap: Wrap::NoWrap,
                    color: p.faint,
                });
            }
        }

        // Day column vertical dividers
        for di in 0..=7 {
            let x = GUTTER_W + (di as f32) * col_w;
            cmds.push(DrawCmd::Line {
                x1: x,
                y1: 0.0,
                x2: x,
                y2: total_h,
                width: 1.0,
                color: p.hairline,
            });
        }

        let shown = self.preview_entries();
        let show_act = self.mode != CalendarMode::Time;
        let show_ent = self.mode != CalendarMode::Activity;

        for (di, &day) in self.week_days.iter().enumerate() {
            let day_end = add_days_start(day, 1);

            // Activity sessions (behind entries in Mix mode; full width in Activity mode)
            if show_act {
                for sess in &self.sessions_by_day[di] {
                    let rect = self.session_box(canvas_w, di, sess, day_end);
                    let lead = sess
                        .apps
                        .iter()
                        .find(|a| a.name != super::calendar_layout::CLAUDE_APP)
                        .or_else(|| sess.apps.first());
                    let color = lead.map(|a| app_color(&a.name)).unwrap_or(p.muted);
                    let fill_c = if self.mode == CalendarMode::Activity {
                        mix(color, p.surface, 0.20)
                    } else {
                        mix(color, p.surface, 0.55)
                    };
                    cmds.push(DrawCmd::FillRect {
                        rect,
                        color: fill_c,
                        radius: 4.0,
                    });
                    if self.mode == CalendarMode::Activity && rect.h >= 18.0 {
                        cmds.push(DrawCmd::StrokeRect {
                            rect,
                            color,
                            width: 1.0,
                            radius: 4.0,
                        });
                        let title = lead.map(|a| a.name.as_str()).unwrap_or("Activity");
                        cmds.push(DrawCmd::Text {
                            rect: Rect {
                                x: rect.x + 6.0,
                                y: rect.y + 3.0,
                                w: (rect.w - 12.0).max(0.0),
                                h: 16.0,
                            },
                            text: format!("{title} · {}", duration(sess.active_ms)),
                            size: 11.0,
                            weight: Weight::Medium,
                            align: Align::Left,
                            wrap: Wrap::NoWrap,
                            color: p.text,
                        });
                    }
                }
            }

            // Time entries
            if show_ent {
                let segs = layout_day(&shown, day, self.now_ms);
                for seg in segs {
                    let rect = self.entry_box(canvas_w, di, &seg, day_end);
                    let proj = seg
                        .entry
                        .project_id
                        .as_deref()
                        .and_then(|pid| self.projects_by_id.get(pid));
                    let accent = proj
                        .and_then(|pr| parse_hex_color(&pr.color))
                        .unwrap_or(NO_PROJECT);
                    let live = seg.entry.end.is_none();
                    let is_dragged = match &self.drag {
                        Some(GridDrag::Move {
                            entry, moved: true, ..
                        }) => entry.id == seg.entry.id,
                        Some(GridDrag::Resize {
                            entry, moved: true, ..
                        }) => entry.id == seg.entry.id,
                        _ => false,
                    };

                    let bg = mix(accent, p.surface, if is_dragged { 0.30 } else { 0.18 });
                    cmds.push(DrawCmd::FillRect {
                        rect,
                        color: bg,
                        radius: 5.0,
                    });
                    // Left accent stripe
                    cmds.push(DrawCmd::FillRect {
                        rect: Rect {
                            x: rect.x,
                            y: rect.y,
                            w: 3.0,
                            h: rect.h,
                        },
                        color: accent,
                        radius: 2.0,
                    });
                    cmds.push(DrawCmd::StrokeRect {
                        rect,
                        color: if live || is_dragged {
                            accent
                        } else {
                            mix(accent, p.surface, 0.45)
                        },
                        width: if live || is_dragged { 1.5 } else { 1.0 },
                        radius: 5.0,
                    });

                    let title = if !seg.entry.description.trim().is_empty() {
                        seg.entry.description.as_str()
                    } else if let Some(pr) = proj {
                        pr.name.as_str()
                    } else {
                        strings::PROJ_REP_NO_DESC
                    };
                    if rect.h >= 16.0 && rect.w > 22.0 {
                        cmds.push(DrawCmd::Text {
                            rect: Rect {
                                x: rect.x + 6.0,
                                y: rect.y + 3.0,
                                w: (rect.w - 10.0).max(0.0),
                                h: 16.0,
                            },
                            text: title.to_string(),
                            size: 11.5,
                            weight: Weight::SemiBold,
                            align: Align::Left,
                            wrap: Wrap::NoWrap,
                            color: p.text,
                        });
                    }
                    if rect.h >= 38.0 && rect.w > 26.0 {
                        let sub = match proj {
                            Some(pr) => format!(
                                "{} · {}",
                                pr.name,
                                duration(entry_ms(&seg.entry, day, day_end, self.now_ms))
                            ),
                            None => duration(entry_ms(&seg.entry, day, day_end, self.now_ms)),
                        };
                        cmds.push(DrawCmd::Text {
                            rect: Rect {
                                x: rect.x + 6.0,
                                y: rect.y + 20.0,
                                w: (rect.w - 10.0).max(0.0),
                                h: 14.0,
                            },
                            text: sub,
                            size: 10.5,
                            weight: Weight::Regular,
                            align: Align::Left,
                            wrap: Wrap::NoWrap,
                            color: p.muted,
                        });
                    }
                }
            }

            // Create ghost preview on active drag day
            if let Some(GridDrag::Create {
                day_idx,
                anchor_min,
                cur_min,
                alt,
            }) = self.drag
            {
                if day_idx == di {
                    let (lo, hi) = if anchor_min <= cur_min {
                        (
                            anchor_min,
                            cur_min.max(anchor_min + if alt { 5.0 } else { f64::from(SNAP_MIN) }),
                        )
                    } else {
                        (cur_min, anchor_min)
                    };
                    let (col_x, _, lane_w, _) = self.col_geometry(canvas_w, di);
                    let gy = (lo as f32) * DIP_PER_MIN;
                    let gh = ((hi - lo) as f32 * DIP_PER_MIN).max(12.0);
                    let ghost = Rect {
                        x: col_x + 2.0,
                        y: gy,
                        w: (lane_w - 4.0).max(16.0),
                        h: gh,
                    };
                    cmds.push(DrawCmd::FillRect {
                        rect: ghost,
                        color: p.accent_bg,
                        radius: 5.0,
                    });
                    cmds.push(DrawCmd::StrokeRect {
                        rect: ghost,
                        color: p.accent,
                        width: 1.5,
                        radius: 5.0,
                    });
                    let s_ts = at_minutes(day, lo.round() as i32);
                    let e_ts = at_minutes(day, hi.round() as i32);
                    cmds.push(DrawCmd::Text {
                        rect: Rect {
                            x: ghost.x + 6.0,
                            y: ghost.y + 3.0,
                            w: (ghost.w - 12.0).max(0.0),
                            h: 16.0,
                        },
                        text: format!(
                            "{} – {}",
                            format::time(s_ts, self.is_24h),
                            format::time(e_ts, self.is_24h)
                        ),
                        size: 11.0,
                        weight: Weight::SemiBold,
                        align: Align::Left,
                        wrap: Wrap::NoWrap,
                        color: p.accent,
                    });
                }
            }

            // Current-time horizontal indicator
            if same_day(day, self.now_ms) {
                let ny = (minutes_of_day(self.now_ms) as f32) * DIP_PER_MIN;
                let nx = GUTTER_W + (di as f32) * col_w;
                cmds.push(DrawCmd::FillCircle {
                    cx: nx,
                    cy: ny,
                    r: 3.5,
                    color: p.danger,
                });
                cmds.push(DrawCmd::Line {
                    x1: nx,
                    y1: ny,
                    x2: nx + col_w,
                    y2: ny,
                    width: 1.5,
                    color: p.danger,
                });
            }
        }

        // Keyboard grid selection cursor (visible when canvas holds keyboard focus)
        if self.canvas_focused {
            let (col_x, _, lane_w, _) = self.col_geometry(canvas_w, self.cursor_day.min(6));
            let cy = (self.cursor_min as f32) * DIP_PER_MIN;
            cmds.push(DrawCmd::StrokeRect {
                rect: Rect {
                    x: col_x + 2.0,
                    y: cy,
                    w: (lane_w.max(20.0) - 4.0).max(12.0),
                    h: 30.0 * DIP_PER_MIN,
                },
                color: p.accent,
                width: 2.0,
                radius: 4.0,
            });
        }

        cmds
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &mut self,
        tree: &mut Tree,
        parent: Node,
        week_start: i64,
        entries: &[TimeEntry],
        activity: &[Activity],
        projects_by_id: &HashMap<String, Project>,
        now_ms: i64,
        scroll_to_hour: u32,
        is_24h: bool,
        env: &ScreenEnv<'_>,
    ) {
        let p = env.palette;
        self.palette_cache = *p;
        self.now_ms = now_ms;
        self.is_24h = is_24h;
        self.projects_by_id = projects_by_id.clone();
        self.entries = entries.to_vec();

        for i in 0..7 {
            self.week_days[i] = add_days_start(week_start, i as i32);
            self.sessions_by_day[i] = if self.mode != CalendarMode::Time {
                activity_sessions(activity, self.week_days[i], 5 * MIN, 3 * MIN)
            } else {
                Vec::new()
            };
        }

        if !self.initial_scroll_done
            || self.last_scroll_to_hour != scroll_to_hour
            || self.last_week_start != week_start
        {
            self.scroll_y =
                ((scroll_to_hour as f32) * HOUR_DIP - 8.0).clamp(0.0, 24.0 * HOUR_DIP - 200.0);
            self.initial_scroll_done = true;
            self.last_scroll_to_hour = scroll_to_hour;
            self.last_week_start = week_start;
            self.cursor_min = (scroll_to_hour as i32) * 60;
        }

        let card = tree.add_container(
            parent,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                min_size: Size {
                    width: length(0.0_f32),
                    height: length(0.0_f32),
                },
                ..Default::default()
            },
        );
        tree.set_paint(
            card,
            PaintStyle {
                bg: Some(p.surface),
                border_color: Some(p.card_border),
                border_width: p.border_width,
                radius: 10.0,
                clip: true,
            },
        );

        // Day headers row
        let head = tree.add_container(
            card,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                size: Size {
                    width: percent(1.0_f32),
                    height: length(50.0_f32),
                },
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        tree.set_paint(
            head,
            PaintStyle {
                bg: Some(p.surface),
                border_color: Some(p.hairline),
                border_width: p.border_width,
                radius: 0.0,
                clip: false,
            },
        );
        let _gutter_pad = tree.add_container(
            head,
            Style {
                size: Size {
                    width: length(GUTTER_W),
                    height: percent(1.0_f32),
                },
                flex_shrink: 0.0,
                ..Default::default()
            },
        );
        for (di, &day) in self.week_days.iter().enumerate() {
            let day_end = add_days_start(day, 1);
            let l = to_local(day);
            let today = same_day(day, now_ms);
            let day_box = tree.add_container(
                head,
                Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Column,
                    align_items: Some(AlignItems::Center),
                    justify_content: Some(JustifyContent::Center),
                    flex_grow: 1.0,
                    flex_basis: length(0.0_f32),
                    gap: style::gap(1.0),
                    ..Default::default()
                },
            );
            let dow = weekday_short(l.weekday_mon0());
            let top_lbl = format!("{dow} {}", l.day);
            add_label(
                tree,
                day_box,
                &top_lbl,
                12.5,
                if today {
                    Weight::SemiBold
                } else {
                    Weight::Medium
                },
                Align::Center,
                if today { p.accent } else { p.text },
            );
            let ms = if self.mode == CalendarMode::Activity {
                let ints: Vec<(i64, i64)> = activity
                    .iter()
                    .filter(|a| a.kind == "app")
                    .map(|a| (a.start, a.end))
                    .collect();
                union_ms(&ints, day, day_end)
            } else {
                entries
                    .iter()
                    .map(|e| entry_ms(e, day, day_end, now_ms))
                    .sum()
            };
            if ms > 0 {
                let dstr = duration(ms);
                add_label(
                    tree,
                    day_box,
                    &dstr,
                    11.0,
                    Weight::Regular,
                    Align::Center,
                    p.muted,
                );
            } else {
                let _ = di;
            }
        }

        // Scrollable 24-hour canvas viewport
        let viewport = tree.add_container(
            card,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                flex_grow: 1.0,
                flex_basis: length(0.0_f32),
                size: Size {
                    width: percent(1.0_f32),
                    height: auto(),
                },
                min_size: Size {
                    width: length(0.0_f32),
                    height: length(0.0_f32),
                },
                ..Default::default()
            },
        );
        tree.set_paint(
            viewport,
            PaintStyle {
                bg: None,
                border_color: None,
                border_width: 0.0,
                radius: 0.0,
                clip: true,
            },
        );
        tree.set_scroll(viewport, 0.0, self.scroll_y);

        let canvas_w = (env.width_dip - 48.0).max(520.0);
        self.last_canvas_w = canvas_w;
        self.last_viewport_rect = Rect {
            x: 24.0,
            y: 180.0,
            w: canvas_w,
            h: (env.height_dip - 200.0).max(240.0),
        };
        self.canvas_focused = env.focused.is_some() && env.focused == self.canvas_id;

        let cmds = self.render_canvas_cmds(canvas_w, p);
        let canvas = tree.add_leaf(
            viewport,
            Style {
                flex_shrink: 0.0,
                size: Size {
                    width: percent(1.0_f32),
                    height: length(24.0 * HOUR_DIP),
                },
                ..Default::default()
            },
            Leaf::Canvas(cmds),
        );
        tree.set_hit(canvas, HitMode::Interactive);
        tree.set_focusable(canvas, true);
        tree.set_a11y(
            canvas,
            A11yInfo {
                role: A11yRole::Group,
                name: strings::CAL_GRID_LABEL.into(),
                clickable: true,
                requires_name: true,
                ..Default::default()
            },
        );

        self.viewport_node = Some(viewport);
        self.canvas_node = Some(canvas);
        self.viewport_id = Some(tree.id(viewport));
        self.canvas_id = Some(tree.id(canvas));
    }

    pub fn on_pointer_down(
        &mut self,
        id: WidgetId,
        x_dip: f32,
        y_dip: f32,
        mods: KeyModifiers,
    ) -> PointerReply {
        if Some(id) != self.canvas_id {
            return PointerReply::default();
        }
        let cr = self.canvas_rect();
        let canvas_w = cr.w.max(200.0);
        let (day_idx, min, lx, ly) = self.locate_local(x_dip, y_dip);
        if lx < GUTTER_W {
            return PointerReply::default();
        }
        let day = self.week_days[day_idx];
        let day_end = add_days_start(day, 1);

        // 1. Check activity session tiles / strip
        if self.mode != CalendarMode::Time {
            for sess in &self.sessions_by_day[day_idx] {
                let sbox = self.session_box(canvas_w, day_idx, sess, day_end);
                if sbox.contains(lx, ly) {
                    let vp = self.viewport_rect();
                    let anchor = Rect {
                        x: cr.x + sbox.x,
                        y: vp.y + sbox.y - self.scroll_y,
                        w: sbox.w,
                        h: sbox.h,
                    };
                    self.drag = Some(GridDrag::ClickActivity {
                        session: sess.clone(),
                        anchor_rect: anchor,
                    });
                    return PointerReply {
                        handled: true,
                        capture: true,
                        dragging: false,
                        rebuild: false,
                        fast_redraw: false,
                    };
                }
            }
        }

        if self.mode == CalendarMode::Activity {
            return PointerReply::default();
        }

        // 2. Check existing entry blocks (reverse order so later overlaps win)
        let segs = layout_day(&self.entries, day, self.now_ms);
        let vp = self.viewport_rect();
        for seg in segs.into_iter().rev() {
            let ebox = self.entry_box(canvas_w, day_idx, &seg, day_end);
            if ebox.contains(lx, ly) {
                let live = seg.entry.end.is_none();
                if !live && !seg.clipped_bottom && ebox.h >= 16.0 && ly >= ebox.y + ebox.h - 7.0 {
                    let end_min = Self::seg_end_minutes(seg.end, day_end);
                    self.drag = Some(GridDrag::Resize {
                        entry: seg.entry,
                        day_idx,
                        cur_end_min: end_min,
                        moved: false,
                        alt: mods.alt,
                    });
                } else {
                    let duration_ms = seg.entry.end.unwrap_or(self.now_ms) - seg.entry.start;
                    let anchor = Rect {
                        x: cr.x + ebox.x,
                        y: vp.y + ebox.y - self.scroll_y,
                        w: ebox.w,
                        h: ebox.h,
                    };
                    self.drag = Some(GridDrag::Move {
                        orig_start_min: minutes_of_day(seg.entry.start),
                        cur_start_min: minutes_of_day(seg.entry.start),
                        entry: seg.entry,
                        grab_min: min,
                        grab_day: day_idx,
                        day_idx,
                        duration_ms,
                        moved: false,
                        alt: mods.alt,
                        anchor_rect: anchor,
                    });
                }
                self.last_pointer_dip = Some((x_dip, y_dip));
                return PointerReply {
                    handled: true,
                    capture: true,
                    dragging: true,
                    rebuild: false,
                    fast_redraw: true,
                };
            }
        }

        // 3. Empty lane -> Create drag
        let anchor_min = if mods.alt {
            min.round()
        } else {
            ((min / f64::from(SNAP_MIN)).floor() * f64::from(SNAP_MIN)).clamp(0.0, DAY_MIN_MAX)
        };
        let end_min = (anchor_min + f64::from(SNAP_MIN)).clamp(0.0, DAY_MIN_MAX);
        self.drag = Some(GridDrag::Create {
            day_idx,
            anchor_min,
            cur_min: end_min,
            alt: mods.alt,
        });
        self.last_pointer_dip = Some((x_dip, y_dip));
        PointerReply {
            handled: true,
            capture: true,
            dragging: true,
            rebuild: false,
            fast_redraw: true,
        }
    }

    pub fn on_pointer_move(&mut self, x_dip: f32, y_dip: f32, mods: KeyModifiers) -> PointerReply {
        let Some(mut drag) = self.drag.take() else {
            return PointerReply::default();
        };
        if matches!(drag, GridDrag::ClickActivity { .. }) {
            self.drag = Some(drag);
            return PointerReply::default();
        }
        self.last_pointer_dip = Some((x_dip, y_dip));
        let (day_idx, min, _, _) = self.locate_local(x_dip, y_dip);

        match &mut drag {
            GridDrag::Create { cur_min, alt, .. } => {
                *alt = mods.alt;
                *cur_min = Self::snap_min(min, mods.alt);
            }
            GridDrag::Move {
                entry,
                grab_min,
                grab_day,
                day_idx: ref mut d_idx,
                orig_start_min,
                cur_start_min,
                moved,
                alt,
                ..
            } => {
                *alt = mods.alt;
                if entry.end.is_some() {
                    let delta = if mods.alt {
                        (min - *grab_min).round()
                    } else {
                        ((min - *grab_min) / f64::from(SNAP_MIN)).round() * f64::from(SNAP_MIN)
                    };
                    let base = if mods.alt {
                        *orig_start_min
                    } else {
                        Self::snap_min(*orig_start_min, false)
                    };
                    *cur_start_min = (base + delta).clamp(0.0, DAY_MIN_MAX - 15.0);
                    *d_idx = day_idx;
                    *moved = *moved || (min - *grab_min).abs() > 4.0 || day_idx != *grab_day;
                }
            }
            GridDrag::Resize {
                entry,
                cur_end_min,
                moved,
                alt,
                ..
            } => {
                *alt = mods.alt;
                let start_m = minutes_of_day(entry.start);
                let min_step = if mods.alt { 1.0 } else { f64::from(SNAP_MIN) };
                let same_day_start = same_day(entry.start, entry.end.unwrap_or(self.now_ms) - 1);
                let floor = if same_day_start {
                    start_m + min_step
                } else {
                    min_step
                };
                *cur_end_min = Self::snap_min(min, mods.alt).max(floor).min(DAY_MIN_MAX);
                *moved = true;
            }
            GridDrag::ClickActivity { .. } => {}
        }

        self.drag = Some(drag);
        PointerReply {
            handled: true,
            capture: true,
            dragging: true,
            rebuild: false,
            fast_redraw: true,
        }
    }

    pub fn on_pointer_up(&mut self, _x_dip: f32, _y_dip: f32, _mods: KeyModifiers) -> PointerReply {
        let Some(drag) = self.drag.take() else {
            return PointerReply::default();
        };
        self.last_pointer_dip = None;
        let cr = self.canvas_rect();
        let vp = self.viewport_rect();

        match drag {
            GridDrag::ClickActivity {
                session,
                anchor_rect,
            } => {
                self.pending_action = Some(GridAction::OpenActivity(session, anchor_rect));
            }
            GridDrag::Create {
                day_idx,
                anchor_min,
                cur_min,
                alt,
            } => {
                let min_step = if alt { 5.0 } else { f64::from(SNAP_MIN) };
                let (lo, hi) = if anchor_min <= cur_min {
                    (anchor_min, cur_min.max(anchor_min + min_step))
                } else {
                    (cur_min, anchor_min)
                };
                // Plain click without dragging creates a 30-min block cleanly.
                let span_min = if (hi - lo) <= f64::from(SNAP_MIN)
                    && (cur_min - anchor_min).abs() <= f64::from(SNAP_MIN)
                {
                    30.0
                } else {
                    hi - lo
                };
                let start = at_minutes(self.week_days[day_idx], lo.round() as i32);
                let end = start + (span_min.round() as i64) * MIN;
                let (col_x, _, lane_w, _) = self.col_geometry(cr.w.max(200.0), day_idx);
                let anchor = Rect {
                    x: cr.x + col_x,
                    y: vp.y + (lo as f32) * DIP_PER_MIN - self.scroll_y,
                    w: lane_w,
                    h: (span_min as f32) * DIP_PER_MIN,
                };
                self.pending_action = Some(GridAction::OpenEntry(
                    EntryDraft {
                        id: None,
                        project_id: None,
                        description: String::new(),
                        start,
                        end: Some(end),
                        billable: false,
                        source: "calendar".into(),
                    },
                    anchor,
                ));
            }
            GridDrag::Move {
                entry,
                day_idx,
                cur_start_min,
                duration_ms,
                moved,
                anchor_rect,
                ..
            } => {
                if !moved {
                    self.pending_action = Some(GridAction::OpenEntry(
                        EntryDraft::from_entry(&entry),
                        anchor_rect,
                    ));
                } else {
                    let start = at_minutes(self.week_days[day_idx], cur_start_min.round() as i32);
                    if start != entry.start {
                        self.pending_action = Some(GridAction::UpdateEntry {
                            id: entry.id,
                            patch: EntryPatch {
                                start: Some(start),
                                end: Some(Some(start + duration_ms)),
                                ..Default::default()
                            },
                        });
                    }
                }
            }
            GridDrag::Resize {
                entry,
                cur_end_min,
                moved,
                ..
            } => {
                if moved {
                    let base_day = start_of_day(entry.end.unwrap_or(self.now_ms) - 1);
                    let new_end = at_minutes(base_day, cur_end_min.round() as i32);
                    if new_end > entry.start && Some(new_end) != entry.end {
                        self.pending_action = Some(GridAction::UpdateEntry {
                            id: entry.id,
                            patch: EntryPatch {
                                end: Some(Some(new_end)),
                                ..Default::default()
                            },
                        });
                    }
                }
            }
        }

        PointerReply {
            handled: true,
            capture: false,
            dragging: false,
            rebuild: true,
            fast_redraw: false,
        }
    }

    pub fn cancel_drag(&mut self) -> bool {
        if self.drag.take().is_some() {
            self.last_pointer_dip = None;
            true
        } else {
            false
        }
    }

    pub fn wants_autoscroll(&self) -> bool {
        if !self.is_dragging() {
            return false;
        }
        let Some((_, py)) = self.last_pointer_dip else {
            return false;
        };
        let vp = self.viewport_rect();
        let max_scroll = (24.0 * HOUR_DIP - vp.h).max(0.0);
        (py < vp.y + 32.0 && self.scroll_y > 0.5)
            || (py > vp.y + vp.h - 32.0 && self.scroll_y < max_scroll - 0.5)
    }

    pub fn on_autoscroll(&mut self) -> PointerReply {
        if !self.is_dragging() {
            return PointerReply::default();
        }
        let Some((px, py)) = self.last_pointer_dip else {
            return PointerReply::default();
        };
        let vp = self.viewport_rect();
        let max_scroll = (24.0 * HOUR_DIP - vp.h).max(0.0);
        let delta = if py < vp.y + 32.0 {
            -10.0
        } else if py > vp.y + vp.h - 32.0 {
            10.0
        } else {
            0.0
        };
        let next = (self.scroll_y + delta).clamp(0.0, max_scroll);
        if (next - self.scroll_y).abs() < 0.2 {
            return PointerReply::default();
        }
        self.scroll_y = next;
        let alt = match &self.drag {
            Some(
                GridDrag::Create { alt, .. }
                | GridDrag::Move { alt, .. }
                | GridDrag::Resize { alt, .. },
            ) => *alt,
            _ => false,
        };
        self.on_pointer_move(
            px,
            py,
            KeyModifiers {
                alt,
                ..Default::default()
            },
        )
    }

    pub fn sync_drag_tree(&mut self, tree: &mut Tree) {
        if let Some(vp_node) = self.viewport_node {
            self.last_viewport_rect = tree.rect(vp_node);
            tree.set_scroll(vp_node, 0.0, self.scroll_y);
        }
        if let Some(c_node) = self.canvas_node {
            let cw = tree.rect(c_node).w.max(200.0);
            self.last_canvas_w = cw;
            let cmds = self.render_canvas_cmds(cw, &self.palette_cache);
            tree.replace_canvas(c_node, cmds);
            self.drag_frames_rendered += 1;
        }
    }

    pub fn on_wheel_at(&mut self, _x_dip: f32, _y_dip: f32, delta_dip: f32) -> bool {
        let vp = self.viewport_rect();
        let max_scroll = (24.0 * HOUR_DIP - vp.h).max(0.0);
        let next = (self.scroll_y - delta_dip).clamp(0.0, max_scroll);
        if (next - self.scroll_y).abs() > 0.2 {
            self.scroll_y = next;
            true
        } else {
            false
        }
    }

    pub fn on_key(&mut self, key: KeyCode) -> bool {
        if !self.canvas_focused {
            return false;
        }
        match key {
            KeyCode::Left => {
                self.cursor_day = self.cursor_day.saturating_sub(1);
                true
            }
            KeyCode::Right => {
                self.cursor_day = (self.cursor_day + 1).min(6);
                true
            }
            KeyCode::Up => {
                self.cursor_min = (self.cursor_min - 30).max(0);
                let cy = (self.cursor_min as f32) * DIP_PER_MIN;
                if cy < self.scroll_y {
                    self.scroll_y = cy;
                }
                true
            }
            KeyCode::Down => {
                self.cursor_min = (self.cursor_min + 30).min(23 * 60 + 30);
                let cy = ((self.cursor_min + 30) as f32) * DIP_PER_MIN;
                let vh = self.viewport_rect().h;
                if cy > self.scroll_y + vh {
                    self.scroll_y = cy - vh;
                }
                true
            }
            KeyCode::PageUp => self.on_wheel_at(0.0, 0.0, 200.0),
            KeyCode::PageDown => self.on_wheel_at(0.0, 0.0, -200.0),
            KeyCode::Enter | KeyCode::Space => {
                let day = self.week_days[self.cursor_day.min(6)];
                let slot_s = at_minutes(day, self.cursor_min);
                let slot_e = slot_s + 30 * MIN;
                let anchor = self.viewport_rect();
                if let Some(existing) = self.entries.iter().find(|e| {
                    let end = e.end.unwrap_or(self.now_ms);
                    e.start < slot_e && end > slot_s
                }) {
                    self.pending_action = Some(GridAction::OpenEntry(
                        EntryDraft::from_entry(existing),
                        anchor,
                    ));
                } else {
                    let desc = self.sessions_by_day[self.cursor_day.min(6)]
                        .iter()
                        .find(|s| s.start < slot_e && s.end > slot_s)
                        .map(session_label)
                        .unwrap_or_default();
                    self.pending_action = Some(GridAction::OpenEntry(
                        EntryDraft {
                            id: None,
                            project_id: None,
                            description: desc,
                            start: slot_s,
                            end: Some(slot_e),
                            billable: false,
                            source: "calendar".into(),
                        },
                        anchor,
                    ));
                }
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Store;
    use crate::demo::DemoStore;
    use crate::locale::SystemLocale;
    use crate::time::DAY;
    use crate::ui::a11y;
    use crate::ui::text::TextSystem;

    #[test]
    fn week_grid_create_move_alt_no_snap_esc_cancel_and_fast_drag_work() {
        let text = TextSystem::new().expect("dwrite");
        let palette = Palette::LIGHT;
        let demo = DemoStore::new(SystemLocale::en_us());
        let now = demo.now_ms();
        let week_start = crate::time::start_of_week(now);
        let entries = demo.list_entries(week_start, week_start + 7 * DAY).unwrap();
        let activity = demo
            .list_activity(week_start, week_start + 7 * DAY)
            .unwrap();
        let projs: HashMap<String, Project> = demo
            .list_projects()
            .unwrap()
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();

        let env = ScreenEnv {
            width_dip: 1200.0,
            height_dip: 800.0,
            hover: None,
            pressed: None,
            focused: None,
            palette: &palette,
            text: &text,
        };
        let mut grid = WeekGridState::new();
        let mut tree = Tree::new(style::column());
        let root = tree.root();
        grid.build(
            &mut tree, root, week_start, &entries, &activity, &projs, now, 8, true, &env,
        );
        tree.layout(1200.0, 800.0, &text);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));

        let cid = grid.canvas_id.unwrap();
        // Start create drag in empty Sunday column (col 6, ~06:00).
        let x_sun = GUTTER_W + 6.3 * ((env.width_dip - 48.0 - GUTTER_W) / 7.0);
        let y_start = grid.last_viewport_rect.y + 6.0 * HOUR_DIP - grid.scroll_y;
        let down = grid.on_pointer_down(cid, x_sun, y_start, KeyModifiers::default());
        assert!(down.handled && down.capture && down.dragging);

        // Move with Alt held -> no 15m snap, fast_redraw without rebuild.
        let y_drag = y_start + 47.0; // ~54 min
        let mv = grid.on_pointer_move(
            x_sun,
            y_drag,
            KeyModifiers {
                alt: true,
                ..Default::default()
            },
        );
        assert!(mv.fast_redraw && !mv.rebuild);
        grid.sync_drag_tree(&mut tree);
        assert_eq!(grid.drag_frames_rendered, 1);

        // Esc cancels drag without producing pending_action.
        assert!(grid.cancel_drag());
        assert!(grid.pending_action.is_none());

        // Create drag finishing normally emits OpenEntry draft.
        grid.on_pointer_down(cid, x_sun, y_start, KeyModifiers::default());
        grid.on_pointer_move(x_sun, y_start + 60.0 * DIP_PER_MIN, KeyModifiers::default());
        let up = grid.on_pointer_up(x_sun, y_start + 60.0 * DIP_PER_MIN, KeyModifiers::default());
        assert!(up.rebuild && !up.dragging);
        match grid.pending_action.take().unwrap() {
            GridAction::OpenEntry(draft, _) => {
                assert_eq!(draft.end.unwrap() - draft.start, 60 * MIN);
            }
            other => panic!("expected OpenEntry, got {other:?}"),
        }
    }
}
