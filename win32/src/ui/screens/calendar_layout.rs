//! Day column overlap layout and activity session grouping matching Electron `layout.ts`.

use std::collections::HashMap;

use crate::db::{Activity, TimeEntry};
use crate::stats::union_ms;
use crate::time::{add_days_start, overlap};

pub const HOUR_DIP: f32 = 52.0;
pub const SNAP_MIN: i32 = 15;
pub const DIP_PER_MIN: f32 = HOUR_DIP / 60.0;
pub const CLAUDE_APP: &str = "Claude Code";

#[derive(Clone, Debug, PartialEq)]
pub struct EntrySeg {
    pub entry: TimeEntry,
    pub start: i64,
    pub end: i64,
    pub col: usize,
    pub cols: usize,
    /// True when the entry started before this local midnight.
    pub clipped_top: bool,
    /// True when the entry extends past this local midnight.
    pub clipped_bottom: bool,
}

/// Splits entries into one segment per day `[day_start, add_days_start(day_start, 1))`
/// and places mutually overlapping clusters side by side cleanly.
pub fn layout_day(entries: &[TimeEntry], day_start: i64, now_ms: i64) -> Vec<EntrySeg> {
    let day_end = add_days_start(day_start, 1);
    let mut segs: Vec<EntrySeg> = entries
        .iter()
        .filter_map(|e| {
            let end = e.end.unwrap_or(now_ms);
            if overlap(e.start, end, day_start, day_end) <= 0 {
                return None;
            }
            Some(EntrySeg {
                entry: e.clone(),
                start: e.start.max(day_start),
                end: end.min(day_end),
                col: 0,
                cols: 1,
                clipped_top: e.start < day_start,
                clipped_bottom: end > day_end,
            })
        })
        .collect();

    segs.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| b.end.cmp(&a.end)));

    let mut cluster_indices: Vec<usize> = Vec::new();
    let mut cluster_end = i64::MIN;
    let mut col_ends: Vec<i64> = Vec::new();

    let flush = |segs: &mut [EntrySeg], cluster: &mut Vec<usize>, cols_end: &mut Vec<i64>| {
        if cluster.is_empty() {
            return;
        }
        let n = cluster
            .iter()
            .map(|&idx| segs[idx].col + 1)
            .max()
            .unwrap_or(1)
            .max(1);
        for &idx in cluster.iter() {
            segs[idx].cols = n;
        }
        cluster.clear();
        cols_end.clear();
    };

    for idx in 0..segs.len() {
        let s_start = segs[idx].start;
        let s_end = segs[idx].end;
        if s_start >= cluster_end {
            flush(&mut segs, &mut cluster_indices, &mut col_ends);
            cluster_end = i64::MIN;
        }
        let col = match col_ends.iter().position(|&end| end <= s_start) {
            Some(c) => {
                col_ends[c] = s_end;
                c
            }
            None => {
                col_ends.push(s_end);
                col_ends.len() - 1
            }
        };
        segs[idx].col = col;
        cluster_indices.push(idx);
        cluster_end = cluster_end.max(s_end);
    }
    flush(&mut segs, &mut cluster_indices, &mut col_ends);

    segs
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionDetail {
    pub label: String,
    pub sub: String,
    pub ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionApp {
    pub name: String,
    pub ms: i64,
    pub details: Vec<SessionDetail>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivitySession {
    pub key: String,
    pub start: i64,
    pub end: i64,
    pub active_ms: i64,
    pub apps: Vec<SessionApp>,
}

pub fn app_name_of(a: &Activity) -> &str {
    if a.kind == "claude" {
        CLAUDE_APP
    } else {
        &a.app
    }
}

/// Groups a day's activity into continuous sessions split by gaps wider than `gap_ms`
/// (`5 * MIN` default) and drops sessions whose active union time is below `min_ms` (`3 * MIN`).
pub fn activity_sessions(
    items: &[Activity],
    day_start: i64,
    gap_ms: i64,
    min_ms: i64,
) -> Vec<ActivitySession> {
    let day_end = add_days_start(day_start, 1);
    let mut rows: Vec<(&Activity, i64, i64)> = items
        .iter()
        .filter_map(|a| {
            let s = a.start.max(day_start);
            let e = a.end.min(day_end);
            (e > s).then_some((a, s, e))
        })
        .collect();
    rows.sort_by_key(|&(_, s, _)| s);

    struct RawGroup<'a> {
        start: i64,
        end: i64,
        rows: Vec<(&'a Activity, i64, i64)>,
    }

    let mut groups: Vec<RawGroup<'_>> = Vec::new();
    for r in rows {
        if let Some(last) = groups.last_mut() {
            if r.1 - last.end <= gap_ms {
                last.end = last.end.max(r.2);
                last.rows.push(r);
                continue;
            }
        }
        groups.push(RawGroup {
            start: r.1,
            end: r.2,
            rows: vec![r],
        });
    }

    let mut out = Vec::new();
    for g in groups {
        let intervals: Vec<(i64, i64)> = g.rows.iter().map(|&(_, s, e)| (s, e)).collect();
        let active_ms = union_ms(&intervals, g.start, g.end);
        if active_ms < min_ms {
            continue;
        }

        type DetailMap = HashMap<(String, String), SessionDetail>;
        let mut app_map: HashMap<String, (i64, DetailMap)> = HashMap::new();
        for (a, s, e) in g.rows {
            let dur = e - s;
            let name = app_name_of(a).to_string();
            let entry = app_map
                .entry(name.clone())
                .or_insert_with(|| (0, HashMap::new()));
            entry.0 += dur;
            let label = if a.kind == "claude" {
                if a.detail.is_empty() {
                    "Untitled session".to_string()
                } else {
                    a.detail.clone()
                }
            } else {
                a.detail.clone()
            };
            let sub = if a.kind == "claude" || a.app == name {
                a.context.clone()
            } else {
                a.app.clone()
            };
            if !label.is_empty() {
                let d = entry
                    .1
                    .entry((label.clone(), sub.clone()))
                    .or_insert_with(|| SessionDetail { label, sub, ms: 0 });
                d.ms += dur;
            }
        }

        let mut apps: Vec<SessionApp> = app_map
            .into_iter()
            .map(|(name, (ms, det_map))| {
                let mut details: Vec<SessionDetail> = det_map.into_values().collect();
                details.sort_by_key(|d| std::cmp::Reverse(d.ms));
                SessionApp { name, ms, details }
            })
            .collect();
        apps.sort_by_key(|a| std::cmp::Reverse(a.ms));

        out.push(ActivitySession {
            key: g.start.to_string(),
            start: g.start,
            end: g.end,
            active_ms,
            apps,
        });
    }
    out
}

/// Default activity session session_label helper matching Electron `sessionLabel`.
pub fn session_label(s: &ActivitySession) -> String {
    if let Some(claude) = s.apps.iter().find(|a| a.name == CLAUDE_APP) {
        if let Some(d) = claude.details.first() {
            if !d.sub.is_empty() {
                return d.sub.clone();
            }
        }
    }
    if let Some(non_claude) = s
        .apps
        .iter()
        .find(|a| a.name != CLAUDE_APP && !a.details.is_empty())
    {
        if let Some(d) = non_claude.details.first() {
            if !d.label.is_empty() {
                return d.label.clone();
            }
        }
    }
    s.apps
        .first()
        .map(|a| a.name.clone())
        .unwrap_or_else(|| "Activity".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::local_day_at;
    use crate::time::{from_local, MIN};

    fn entry(id: &str, start: i64, end: i64) -> TimeEntry {
        TimeEntry {
            id: id.into(),
            project_id: None,
            description: id.into(),
            start,
            end: Some(end),
            billable: true,
            source: "manual".into(),
        }
    }

    fn act(app: &str, kind: &str, detail: &str, context: &str, start: i64, end: i64) -> Activity {
        Activity {
            id: 0,
            kind: kind.into(),
            app: app.into(),
            bundle_id: String::new(),
            detail: detail.into(),
            context: context.into(),
            url: String::new(),
            start,
            end,
        }
    }

    #[test]
    fn layout_day_clusters_overlaps_and_clips_across_midnight() {
        let day = from_local(2026, 11, 18, 0, 0, 0, 0);
        let next = add_days_start(day, 1);

        // Cluster 1: A (9-11), B (9:30-10:30), C (10-12) -> 3 columns.
        // Non-overlapping D (14-15) -> 1 column.
        // Overnight E (yesterday 23:00 -> today 01:00) -> clipped_top.
        let entries = vec![
            entry(
                "e",
                local_day_at(2026, 11, 17, 23 * 60),
                local_day_at(2026, 11, 18, 60),
            ),
            entry(
                "a",
                local_day_at(2026, 11, 18, 9 * 60),
                local_day_at(2026, 11, 18, 11 * 60),
            ),
            entry(
                "b",
                local_day_at(2026, 11, 18, 9 * 60 + 30),
                local_day_at(2026, 11, 18, 10 * 60 + 30),
            ),
            entry(
                "c",
                local_day_at(2026, 11, 18, 10 * 60),
                local_day_at(2026, 11, 18, 12 * 60),
            ),
            entry(
                "d",
                local_day_at(2026, 11, 18, 14 * 60),
                local_day_at(2026, 11, 18, 15 * 60),
            ),
        ];

        let segs = layout_day(&entries, day, next);
        assert_eq!(segs.len(), 5);
        assert!(segs[0].clipped_top && !segs[0].clipped_bottom);
        assert_eq!(segs[0].cols, 1);

        let cluster: Vec<&EntrySeg> = segs
            .iter()
            .filter(|s| matches!(s.entry.id.as_str(), "a" | "b" | "c"))
            .collect();
        assert!(cluster.iter().all(|s| s.cols == 3));
        assert_eq!(segs.last().unwrap().entry.id, "d");
        assert_eq!(segs.last().unwrap().cols, 1);
    }

    #[test]
    fn activity_sessions_merges_short_gaps_dedups_parallel_claude_and_picks_label() {
        let day = from_local(2026, 11, 18, 0, 0, 0, 0);
        let t0 = day + 9 * 3_600_000;
        let items = vec![
            // Simultaneous Code (10m) and Claude (10m) + gap 2m + Code (5m)
            act(
                "Visual Studio Code",
                "app",
                "main.rs",
                "trackerApp",
                t0,
                t0 + 10 * MIN,
            ),
            act(
                "Claude",
                "claude",
                "Implement grid",
                "trackerApp",
                t0,
                t0 + 10 * MIN,
            ),
            act(
                "Visual Studio Code",
                "app",
                "week_grid.rs",
                "trackerApp",
                t0 + 12 * MIN,
                t0 + 17 * MIN,
            ),
            // Separate 2m micro-session -> dropped (< 3m minMs)
            act("Slack", "app", "#general", "", t0 + 40 * MIN, t0 + 42 * MIN),
        ];

        let sessions = activity_sessions(&items, day, 5 * MIN, 3 * MIN);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].active_ms, 15 * MIN);
        assert_eq!(session_label(&sessions[0]), "trackerApp");
    }
}
