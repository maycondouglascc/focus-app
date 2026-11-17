//! Virtualized collapsible `Project -> Task -> Entries` tree backed by `VirtualList`.
//! Even with thousands of expanded rows, only the visible slice (`O(visible)`) of nodes
//! is constructed inside the `Tree`.

use std::collections::{HashMap, HashSet};

use taffy::prelude::*;

use crate::db::{Project, TimeEntry};
use crate::format::{day_label, duration, money, time};
use crate::stats::{earned, entry_ms};
use crate::ui::input::KeyCode;
use crate::ui::strings;
use crate::ui::text::{Align, Weight};
use crate::ui::theme::{parse_hex_color, Color, Palette, NO_PROJECT};
use crate::ui::tree::{style, Leaf, Node, PaintStyle, Tree, WidgetId};
use crate::ui::widgets::virtual_list::{self, VirtualListProps, VirtualListState};
use crate::ui::widgets::{add_label, glyph};

const NONE_KEY: &str = "__none";

#[derive(Clone)]
pub enum FlatTreeRow {
    Group {
        key: String,
        name: String,
        sub: String,
        color: Color,
        ms: i64,
        earned_text: String,
        expanded: bool,
    },
    Task {
        key: String,
        level: u8,
        label: String,
        sub: String,
        ms: i64,
        earned_text: String,
        expanded: bool,
    },
    Entry {
        entry: TimeEntry,
        level: u8,
        date_text: String,
        range_text: String,
        dur_text: String,
        amount_text: String,
    },
}

struct TaskBucket {
    key: String,
    label: String,
    ms: i64,
    earned: f64,
    entries: Vec<TimeEntry>,
}

struct GroupBucket {
    key: String,
    name: String,
    client: String,
    color: Color,
    currency: String,
    ms: i64,
    earned: f64,
    count: usize,
    tasks: Vec<TaskBucket>,
}

#[derive(Default)]
pub struct EntryTreeOutcome {
    pub handled: bool,
    pub open_entry: Option<TimeEntry>,
}

pub struct EntryTreeState {
    pub open: HashSet<String>,
    pub list: VirtualListState,
    pub viewport_id: Option<WidgetId>,
    flat: Vec<FlatTreeRow>,
    rendered_rows: Vec<(usize, WidgetId)>,
}

impl Default for EntryTreeState {
    fn default() -> Self {
        Self::new()
    }
}

impl EntryTreeState {
    pub fn new() -> Self {
        Self {
            open: HashSet::new(),
            list: VirtualListState::new(0, 38.0, 340.0),
            viewport_id: None,
            flat: Vec::new(),
            rendered_rows: Vec::new(),
        }
    }

    pub fn row_count(&self) -> usize {
        self.flat.len()
    }

    pub fn rendered_len(&self) -> usize {
        self.rendered_rows.len()
    }

    fn build_tasks(
        entries: Vec<TimeEntry>,
        project: Option<&Project>,
        from: i64,
        to: i64,
        now_ms: i64,
    ) -> Vec<TaskBucket> {
        let mut by_task: HashMap<String, TaskBucket> = HashMap::new();
        for e in entries {
            let trimmed = e.description.trim();
            let label = if trimmed.is_empty() {
                strings::PROJ_REP_NO_DESC.to_string()
            } else {
                trimmed.to_string()
            };
            let key = label.to_lowercase();
            let bucket = by_task.entry(key.clone()).or_insert_with(|| TaskBucket {
                key,
                label,
                ms: 0,
                earned: 0.0,
                entries: Vec::new(),
            });
            bucket.ms += entry_ms(&e, from, to, now_ms);
            bucket.earned += earned(&e, project, from, to, now_ms);
            bucket.entries.push(e);
        }
        let mut out: Vec<TaskBucket> = by_task.into_values().collect();
        for t in &mut out {
            t.entries.sort_by_key(|a| std::cmp::Reverse(a.start));
        }
        out.sort_by_key(|a| std::cmp::Reverse(a.ms));
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn flatten_rows(
        &self,
        entries: &[TimeEntry],
        projects_by_id: &HashMap<String, Project>,
        from: i64,
        to: i64,
        now_ms: i64,
        by_project: bool,
        is_24h: bool,
    ) -> Vec<FlatTreeRow> {
        let mut by_group: HashMap<String, Vec<TimeEntry>> = HashMap::new();
        for e in entries {
            let k = if by_project {
                e.project_id.clone().unwrap_or_else(|| NONE_KEY.to_string())
            } else {
                "all".to_string()
            };
            by_group.entry(k).or_default().push(e.clone());
        }

        let mut groups: Vec<GroupBucket> = by_group
            .into_iter()
            .map(|(k, list)| {
                let proj = if k == NONE_KEY {
                    None
                } else if k == "all" {
                    list.first()
                        .and_then(|e| e.project_id.as_deref())
                        .and_then(|pid| projects_by_id.get(pid))
                } else {
                    projects_by_id.get(&k)
                };
                let count = list.len();
                let tasks = Self::build_tasks(list, proj, from, to, now_ms);
                let ms = tasks.iter().map(|t| t.ms).sum();
                let earned_total = tasks.iter().map(|t| t.earned).sum();
                GroupBucket {
                    name: if k == NONE_KEY {
                        strings::REP_NO_PROJECT.to_string()
                    } else {
                        proj.map(|p| p.name.clone())
                            .unwrap_or_else(|| "Deleted project".into())
                    },
                    client: proj.map(|p| p.client.clone()).unwrap_or_default(),
                    color: proj
                        .and_then(|p| parse_hex_color(&p.color))
                        .unwrap_or(NO_PROJECT),
                    currency: proj.map(|p| p.currency.clone()).unwrap_or_default(),
                    key: k,
                    ms,
                    earned: earned_total,
                    count,
                    tasks,
                }
            })
            .collect();
        groups.sort_by_key(|a| std::cmp::Reverse(a.ms));

        let mut flat = Vec::new();
        for g in groups {
            let g_key = format!("p:{}", g.key);
            let g_open = !by_project || self.open.contains(&g_key);
            if by_project {
                let tasks_word = if g.tasks.len() == 1 { "task" } else { "tasks" };
                let entries_word = if g.count == 1 { "entry" } else { "entries" };
                let stats_sub = format!(
                    "{} {tasks_word} · {} {entries_word}",
                    g.tasks.len(),
                    g.count
                );
                let sub = if g.client.trim().is_empty() {
                    stats_sub
                } else {
                    format!("{} · {stats_sub}", g.client.trim())
                };
                let earned_text = if g.earned > 0.0 && !g.currency.is_empty() {
                    money(g.earned, &g.currency)
                } else {
                    String::new()
                };
                flat.push(FlatTreeRow::Group {
                    key: g_key.clone(),
                    name: g.name.clone(),
                    sub,
                    color: g.color,
                    ms: g.ms,
                    earned_text,
                    expanded: g_open,
                });
            }
            if !g_open {
                continue;
            }
            for t in g.tasks {
                let t_key = format!("t:{}/{}", g.key, t.key);
                let t_open = self.open.contains(&t_key);
                let count_word = if t.entries.len() == 1 {
                    "entry"
                } else {
                    "entries"
                };
                let last_day = t
                    .entries
                    .first()
                    .map(|e| day_label(e.start))
                    .unwrap_or_default();
                let sub = format!("{} {count_word} · last {last_day}", t.entries.len());
                let earned_text = if t.earned > 0.0 && !g.currency.is_empty() {
                    money(t.earned, &g.currency)
                } else {
                    String::new()
                };
                flat.push(FlatTreeRow::Task {
                    key: t_key.clone(),
                    level: if by_project { 2 } else { 1 },
                    label: t.label,
                    sub,
                    ms: t.ms,
                    earned_text,
                    expanded: t_open,
                });
                if !t_open {
                    continue;
                }
                for e in t.entries {
                    let proj = e
                        .project_id
                        .as_deref()
                        .and_then(|pid| projects_by_id.get(pid));
                    let v = earned(&e, proj, -8_640_000_000_000, 8_640_000_000_000, now_ms);
                    let cur = proj.map(|p| p.currency.as_str()).unwrap_or(&g.currency);
                    let end_txt = match e.end {
                        Some(end_ts) => time(end_ts, is_24h),
                        None => "now".into(),
                    };
                    let amount_text = if v > 0.0 && !cur.is_empty() {
                        money(v, cur)
                    } else if !e.billable {
                        "non-billable".into()
                    } else {
                        String::new()
                    };
                    flat.push(FlatTreeRow::Entry {
                        date_text: day_label(e.start),
                        range_text: format!("{} – {end_txt}", time(e.start, is_24h)),
                        dur_text: duration((e.end.unwrap_or(now_ms) - e.start).max(0)),
                        amount_text,
                        level: if by_project { 3 } else { 2 },
                        entry: e,
                    });
                }
            }
        }
        flat
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &mut self,
        tree: &mut Tree,
        parent: Node,
        entries: &[TimeEntry],
        projects_by_id: &HashMap<String, Project>,
        from: i64,
        to: i64,
        now_ms: i64,
        by_project: bool,
        is_24h: bool,
        hover: Option<WidgetId>,
        palette: &Palette,
    ) {
        self.flat = self.flatten_rows(
            entries,
            projects_by_id,
            from,
            to,
            now_ms,
            by_project,
            is_24h,
        );
        self.list.total_count = self.flat.len();
        self.list.row_height = 38.0;
        self.list.viewport_height = ((self.flat.len() as f32) * 38.0).clamp(96.0, 380.0);
        self.list.clamp_scroll();

        let flat_ref = &self.flat;
        let vb = virtual_list::build(
            tree,
            parent,
            &VirtualListProps {
                aria_name: strings::REP_ENTRIES_TITLE,
                state: self.list,
                hover,
            },
            palette,
            |t, row_node, idx, selected| {
                Self::render_flat_row(t, row_node, &flat_ref[idx], selected, palette)
            },
        );
        self.viewport_id = Some(tree.id(vb.viewport));
        self.rendered_rows = vb.rendered_rows;
    }

    fn render_flat_row(
        tree: &mut Tree,
        row_node: Node,
        item: &FlatTreeRow,
        selected: bool,
        p: &Palette,
    ) -> String {
        let _ = selected;
        match item {
            FlatTreeRow::Group {
                name,
                sub,
                color,
                ms,
                earned_text,
                expanded,
                ..
            } => {
                let left = tree.add_container(
                    row_node,
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
                        glyph: if *expanded {
                            glyph::CHEVRON_DOWN
                        } else {
                            glyph::CHEVRON_RIGHT
                        },
                        size: 10.0,
                        color: p.muted,
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
                        bg: Some(*color),
                        border_color: None,
                        border_width: 0.0,
                        radius: 4.0,
                        clip: false,
                    },
                );
                add_label(
                    tree,
                    left,
                    name,
                    13.0,
                    Weight::SemiBold,
                    Align::Left,
                    p.text,
                );
                add_label(tree, left, sub, 11.5, Weight::Regular, Align::Left, p.muted);
                let dur = duration(*ms);
                add_label(
                    tree,
                    row_node,
                    &dur,
                    12.5,
                    Weight::Medium,
                    Align::Right,
                    p.text,
                );
                if !earned_text.is_empty() {
                    let amt = tree.add_container(
                        row_node,
                        Style {
                            size: Size {
                                width: length(84.0_f32),
                                height: auto(),
                            },
                            justify_content: Some(JustifyContent::FlexEnd),
                            ..Default::default()
                        },
                    );
                    add_label(
                        tree,
                        amt,
                        earned_text,
                        12.5,
                        Weight::Regular,
                        Align::Right,
                        p.muted,
                    );
                }
                let state = if *expanded { "expanded" } else { "collapsed" };
                format!("{name}, {sub}, {dur}, {state}")
            }
            FlatTreeRow::Task {
                level,
                label,
                sub,
                ms,
                earned_text,
                expanded,
                ..
            } => {
                let indent = if *level > 1 { 18.0 } else { 0.0 };
                let left = tree.add_container(
                    row_node,
                    Style {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Row,
                        align_items: Some(AlignItems::Center),
                        padding: style::pad(indent, 0.0, 0.0, 0.0),
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
                        glyph: if *expanded {
                            glyph::CHEVRON_DOWN
                        } else {
                            glyph::CHEVRON_RIGHT
                        },
                        size: 10.0,
                        color: p.muted,
                    },
                );
                add_label(tree, left, label, 12.5, Weight::Medium, Align::Left, p.text);
                add_label(tree, left, sub, 11.5, Weight::Regular, Align::Left, p.muted);
                let dur = duration(*ms);
                add_label(
                    tree,
                    row_node,
                    &dur,
                    12.5,
                    Weight::Regular,
                    Align::Right,
                    p.text,
                );
                if !earned_text.is_empty() {
                    let amt = tree.add_container(
                        row_node,
                        Style {
                            size: Size {
                                width: length(84.0_f32),
                                height: auto(),
                            },
                            justify_content: Some(JustifyContent::FlexEnd),
                            ..Default::default()
                        },
                    );
                    add_label(
                        tree,
                        amt,
                        earned_text,
                        12.5,
                        Weight::Regular,
                        Align::Right,
                        p.muted,
                    );
                }
                let state = if *expanded { "expanded" } else { "collapsed" };
                format!("{label}, {sub}, {dur}, {state}")
            }
            FlatTreeRow::Entry {
                level,
                date_text,
                range_text,
                dur_text,
                amount_text,
                ..
            } => {
                let indent = if *level >= 3 { 36.0 } else { 18.0 };
                let left = tree.add_container(
                    row_node,
                    Style {
                        display: Display::Flex,
                        flex_direction: FlexDirection::Row,
                        align_items: Some(AlignItems::Center),
                        padding: style::pad(indent, 0.0, 0.0, 0.0),
                        gap: style::gap(12.0),
                        flex_grow: 1.0,
                        flex_basis: length(0.0_f32),
                        ..Default::default()
                    },
                );
                add_label(
                    tree,
                    left,
                    date_text,
                    12.0,
                    Weight::Medium,
                    Align::Left,
                    p.text,
                );
                add_label(
                    tree,
                    left,
                    range_text,
                    12.0,
                    Weight::Regular,
                    Align::Left,
                    p.muted,
                );
                add_label(
                    tree,
                    row_node,
                    dur_text,
                    12.0,
                    Weight::Regular,
                    Align::Right,
                    p.text,
                );
                if !amount_text.is_empty() {
                    let amt = tree.add_container(
                        row_node,
                        Style {
                            size: Size {
                                width: length(84.0_f32),
                                height: auto(),
                            },
                            justify_content: Some(JustifyContent::FlexEnd),
                            ..Default::default()
                        },
                    );
                    add_label(
                        tree,
                        amt,
                        amount_text,
                        12.0,
                        Weight::Regular,
                        Align::Right,
                        p.muted,
                    );
                }
                format!("{date_text} {range_text}, {dur_text}")
            }
        }
    }

    pub fn handle_click(&mut self, id: WidgetId) -> EntryTreeOutcome {
        let Some(&(idx, _)) = self.rendered_rows.iter().find(|(_, rid)| *rid == id) else {
            return EntryTreeOutcome::default();
        };
        self.list.selected = Some(idx);
        let Some(row) = self.flat.get(idx) else {
            return EntryTreeOutcome::default();
        };
        match row {
            FlatTreeRow::Group { key, .. } | FlatTreeRow::Task { key, .. } => {
                if !self.open.remove(key) {
                    self.open.insert(key.clone());
                }
                EntryTreeOutcome {
                    handled: true,
                    open_entry: None,
                }
            }
            FlatTreeRow::Entry { entry, .. } => EntryTreeOutcome {
                handled: true,
                open_entry: Some(entry.clone()),
            },
        }
    }

    pub fn handle_key(&mut self, key: KeyCode) -> bool {
        self.list.handle_key(key)
    }

    pub fn handle_wheel(&mut self, delta_dip: f32) -> bool {
        self.list.scroll_by(-delta_dip)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Store;
    use crate::demo::DemoStore;
    use crate::locale::SystemLocale;
    use crate::ui::a11y;
    use crate::ui::text::TextSystem;

    #[test]
    fn entry_tree_virtualizes_large_entry_sets_and_expands_three_levels() {
        let text = TextSystem::new().expect("dwrite");
        let demo = DemoStore::new(SystemLocale::en_us());
        let projs: HashMap<String, Project> = demo
            .list_projects()
            .unwrap()
            .into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();
        let now = demo.now_ms();
        let entries = demo.list_entries(now - 90 * 86_400_000, now).unwrap();
        assert!(entries.len() > 20);

        let mut et = EntryTreeState::new();
        let mut tree = Tree::new(style::column());
        let root = tree.root();
        et.build(
            &mut tree,
            root,
            &entries,
            &projs,
            now - 90 * 86_400_000,
            now,
            now,
            true,
            true,
            None,
            &Palette::LIGHT,
        );
        tree.layout(800.0, 500.0, &text);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));
        let group_count = et.row_count();
        assert!(group_count >= 1);

        // Expand first project group -> reveals task rows.
        let first_id = et.rendered_rows[0].1;
        assert!(et.handle_click(first_id).handled);

        let mut tree2 = Tree::new(style::column());
        let r2 = tree2.root();
        et.build(
            &mut tree2,
            r2,
            &entries,
            &projs,
            now - 90 * 86_400_000,
            now,
            now,
            true,
            true,
            None,
            &Palette::LIGHT,
        );
        tree2.layout(800.0, 500.0, &text);
        assert!(et.row_count() > group_count);
        assert!(
            et.rendered_len() <= 14,
            "virtual list must only build visible rows regardless of total row count"
        );
    }
}
