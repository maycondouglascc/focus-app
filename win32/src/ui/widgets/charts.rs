//! Direct2D data-visualization primitives matching Focus's chart suite:
//! `StackedBars`, `Heatmap`, `Donut`, `ProportionBar`, `Spark`, and `ShareRow`.
//! Every color comes strictly from `Palette` / `PROJECT_COLORS` tokens.

use taffy::prelude::*;

use super::add_label;
use crate::ui::text::{Align, Weight};
use crate::ui::theme::{mix, Color, Palette};
use crate::ui::tree::{
    style, A11yInfo, A11yRole, DrawCmd, Leaf, Node, PaintStyle, Rect, Tree, WidgetId,
};

fn figure_a11y(name: &str) -> A11yInfo {
    A11yInfo {
        role: A11yRole::Figure,
        name: name.into(),
        requires_name: true,
        ..Default::default()
    }
}

/// Stacked bar column chart (`bars`: list of per-column `(color, value)` slices).
pub fn stacked_bars(
    tree: &mut Tree,
    parent: Node,
    aria_name: &str,
    bars: &[&[(Color, f32)]],
    width: f32,
    height: f32,
    palette: &Palette,
) -> (Node, WidgetId) {
    let max_total = bars
        .iter()
        .map(|col| col.iter().map(|(_, v)| v.max(0.0)).sum::<f32>())
        .fold(0.0_f32, f32::max);

    let mut cmds = Vec::new();
    // Horizontal subtle reference lines.
    for step in [0.0_f32, 0.5, 1.0] {
        let y = (height - 4.0) * (1.0 - step) + 2.0;
        cmds.push(DrawCmd::Line {
            x1: 0.0,
            y1: y,
            x2: width,
            y2: y,
            width: 1.0,
            color: palette.chart_grid,
        });
    }

    let n = bars.len().max(1) as f32;
    let slot_w = width / n;
    let bar_w = (slot_w * 0.58).clamp(4.0, 28.0);
    let inner_h = (height - 6.0).max(1.0);

    if max_total > 0.0 {
        for (i, segs) in bars.iter().enumerate() {
            let x = (i as f32) * slot_w + (slot_w - bar_w) * 0.5;
            let mut y_cursor = height - 2.0;
            for (color, val) in *segs {
                let h = (val.max(0.0) / max_total) * inner_h;
                if h <= 0.4 {
                    continue;
                }
                y_cursor -= h;
                cmds.push(DrawCmd::FillRect {
                    rect: Rect {
                        x,
                        y: y_cursor,
                        w: bar_w,
                        h,
                    },
                    color: *color,
                    radius: 2.0,
                });
            }
        }
    }

    let node = tree.add_leaf(
        parent,
        Style {
            size: style::size(width, height),
            ..Default::default()
        },
        Leaf::Canvas(cmds),
    );
    tree.set_a11y(node, figure_a11y(aria_name));
    (node, tree.id(node))
}

/// Calendar / weekly intensity heatmap (`intensities`: rows × cols in `0.0..=1.0`).
pub fn heatmap(
    tree: &mut Tree,
    parent: Node,
    aria_name: &str,
    rows: &[&[f32]],
    accent: Color,
    cell_and_gap: (f32, f32),
    palette: &Palette,
) -> (Node, WidgetId) {
    let (cell, gap) = cell_and_gap;
    let row_count = rows.len();
    let col_count = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let w = (col_count as f32) * cell + (col_count.saturating_sub(1) as f32) * gap;
    let h = (row_count as f32) * cell + (row_count.saturating_sub(1) as f32) * gap;

    let mut cmds = Vec::with_capacity(row_count * col_count);
    for (r_idx, row) in rows.iter().enumerate() {
        for (c_idx, &v) in row.iter().enumerate() {
            let clamped = v.clamp(0.0, 1.0);
            let color = if clamped <= 0.01 {
                palette.chart_empty_cell
            } else {
                mix(accent, palette.surface, 0.25 + clamped * 0.75)
            };
            cmds.push(DrawCmd::FillRect {
                rect: Rect {
                    x: (c_idx as f32) * (cell + gap),
                    y: (r_idx as f32) * (cell + gap),
                    w: cell,
                    h: cell,
                },
                color,
                radius: 2.5,
            });
        }
    }

    let node = tree.add_leaf(
        parent,
        Style {
            size: style::size(w.max(cell), h.max(cell)),
            ..Default::default()
        },
        Leaf::Canvas(cmds),
    );
    tree.set_a11y(node, figure_a11y(aria_name));
    (node, tree.id(node))
}

/// Segmented donut ring using `DrawCmd::StrokeArc` (`segments`: `(color, value)`).
pub fn donut(
    tree: &mut Tree,
    parent: Node,
    aria_name: &str,
    segments: &[(Color, f32)],
    diameter: f32,
    stroke: f32,
    palette: &Palette,
) -> (Node, WidgetId) {
    let total: f32 = segments.iter().map(|(_, v)| v.max(0.0)).sum();
    let c = diameter * 0.5;
    let r = (c - stroke * 0.5 - 1.0).max(4.0);
    let mut cmds = Vec::new();

    if total <= 0.0 {
        cmds.push(DrawCmd::StrokeArc {
            cx: c,
            cy: c,
            r,
            start_deg: 0.0,
            sweep_deg: 359.5,
            width: stroke,
            color: palette.chart_empty_cell,
            round_cap: false,
        });
    } else {
        let mut start = -90.0_f32;
        for (color, val) in segments {
            let sweep = (val.max(0.0) / total) * 360.0;
            if sweep < 1.0 {
                continue;
            }
            cmds.push(DrawCmd::StrokeArc {
                cx: c,
                cy: c,
                r,
                start_deg: start,
                sweep_deg: (sweep - 1.2).clamp(1.0, 359.5),
                width: stroke,
                color: *color,
                round_cap: true,
            });
            start += sweep;
        }
    }

    let node = tree.add_leaf(
        parent,
        Style {
            size: style::size(diameter, diameter),
            ..Default::default()
        },
        Leaf::Canvas(cmds),
    );
    tree.set_a11y(node, figure_a11y(aria_name));
    (node, tree.id(node))
}

/// Horizontal multi-segment proportion bar.
pub fn proportion_bar(
    tree: &mut Tree,
    parent: Node,
    aria_name: &str,
    segments: &[(Color, f32)],
    width: f32,
    height: f32,
    palette: &Palette,
) -> (Node, WidgetId) {
    let total: f32 = segments.iter().map(|(_, v)| v.max(0.0)).sum();
    let mut cmds = Vec::new();
    let radius = height * 0.5;

    if total <= 0.0 {
        cmds.push(DrawCmd::FillRect {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w: width,
                h: height,
            },
            color: palette.chart_empty_cell,
            radius,
        });
    } else {
        let mut x = 0.0_f32;
        for (color, val) in segments {
            let seg_w = (val.max(0.0) / total) * width;
            if seg_w < 1.0 {
                continue;
            }
            cmds.push(DrawCmd::FillRect {
                rect: Rect {
                    x,
                    y: 0.0,
                    w: (seg_w - 1.0).max(1.0),
                    h: height,
                },
                color: *color,
                radius: 2.0,
            });
            x += seg_w;
        }
    }

    let node = tree.add_leaf(
        parent,
        Style {
            size: style::size(width, height),
            ..Default::default()
        },
        Leaf::Canvas(cmds),
    );
    tree.set_a11y(node, figure_a11y(aria_name));
    (node, tree.id(node))
}

/// Compact inline sparkline trend primitive.
pub fn spark(
    tree: &mut Tree,
    parent: Node,
    aria_name: &str,
    values: &[f32],
    size: (f32, f32),
    color: Color,
    palette: &Palette,
) -> (Node, WidgetId) {
    let (width, height) = size;
    let mut cmds = Vec::new();
    if values.len() < 2 {
        cmds.push(DrawCmd::Line {
            x1: 0.0,
            y1: height * 0.5,
            x2: width,
            y2: height * 0.5,
            width: 1.5,
            color: palette.chart_grid,
        });
    } else {
        let min = values.iter().copied().fold(f32::INFINITY, f32::min);
        let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let span = (max - min).max(1e-4);
        let step_x = width / (values.len() - 1) as f32;
        let pad_y = 3.0;
        let inner_h = (height - pad_y * 2.0).max(1.0);
        let pt = |i: usize, v: f32| {
            (
                (i as f32) * step_x,
                pad_y + inner_h * (1.0 - (v - min) / span),
            )
        };
        for idx in 1..values.len() {
            let (x1, y1) = pt(idx - 1, values[idx - 1]);
            let (x2, y2) = pt(idx, values[idx]);
            cmds.push(DrawCmd::Line {
                x1,
                y1,
                x2,
                y2,
                width: 1.6,
                color,
            });
        }
        let (lx, ly) = pt(values.len() - 1, *values.last().unwrap());
        cmds.push(DrawCmd::FillCircle {
            cx: lx,
            cy: ly,
            r: 2.5,
            color,
        });
    }

    let node = tree.add_leaf(
        parent,
        Style {
            size: style::size(width, height),
            ..Default::default()
        },
        Leaf::Canvas(cmds),
    );
    tree.set_a11y(node, figure_a11y(aria_name));
    (node, tree.id(node))
}

/// Summary breakdown row: colored dot + label + proportion track + readout text.
pub fn share_row(
    tree: &mut Tree,
    parent: Node,
    label: &str,
    readout: &str,
    ratio: f32,
    color: Color,
    palette: &Palette,
) -> (Node, WidgetId) {
    let row = tree.add_container(
        parent,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            align_items: Some(AlignItems::Center),
            gap: style::gap(8.0),
            size: Size {
                width: percent(1.0_f32),
                height: length(24.0_f32),
            },
            ..Default::default()
        },
    );
    let dot = tree.add_container(
        row,
        Style {
            size: style::size(8.0, 8.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
    );
    tree.set_paint(
        dot,
        PaintStyle {
            bg: Some(color),
            border_color: None,
            border_width: 0.0,
            radius: 4.0,
            clip: false,
        },
    );

    let name_box = tree.add_container(
        row,
        Style {
            size: Size {
                width: length(120.0_f32),
                height: auto(),
            },
            flex_shrink: 0.0,
            ..Default::default()
        },
    );
    add_label(
        tree,
        name_box,
        label,
        12.0,
        Weight::Medium,
        Align::Left,
        palette.text,
    );

    let pct = ratio.clamp(0.0, 1.0);
    let track = tree.add_container(
        row,
        Style {
            flex_grow: 1.0,
            size: Size {
                width: auto(),
                height: length(6.0_f32),
            },
            ..Default::default()
        },
    );
    tree.set_paint(
        track,
        PaintStyle {
            bg: Some(palette.chart_empty_cell),
            border_color: None,
            border_width: 0.0,
            radius: 3.0,
            clip: true,
        },
    );
    let fill = tree.add_container(
        track,
        Style {
            size: Size {
                width: percent(pct),
                height: percent(1.0_f32),
            },
            ..Default::default()
        },
    );
    tree.set_paint(
        fill,
        PaintStyle {
            bg: Some(color),
            border_color: None,
            border_width: 0.0,
            radius: 3.0,
            clip: false,
        },
    );

    let val_box = tree.add_container(
        row,
        Style {
            size: Size {
                width: length(64.0_f32),
                height: auto(),
            },
            justify_content: Some(JustifyContent::FlexEnd),
            flex_shrink: 0.0,
            ..Default::default()
        },
    );
    add_label(
        tree,
        val_box,
        readout,
        12.0,
        Weight::Regular,
        Align::Right,
        palette.muted,
    );

    tree.set_a11y(row, figure_a11y(&format!("{label}, {readout}")));
    (row, tree.id(row))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::a11y;
    use crate::ui::text::TextSystem;
    use crate::ui::theme::PROJECT_COLORS;

    #[test]
    fn all_chart_primitives_build_geometry_and_pass_a11y_validation() {
        let text = TextSystem::new().expect("dwrite");
        let p = &Palette::LIGHT;
        let mut tree = Tree::new(Style {
            size: style::size(600.0, 500.0),
            gap: style::gap(12.0),
            ..style::column()
        });
        let root = tree.root();

        let col0 = [(PROJECT_COLORS[0], 30.0), (PROJECT_COLORS[1], 15.0)];
        let col1 = [(PROJECT_COLORS[0], 20.0), (PROJECT_COLORS[2], 25.0)];
        let (sb, _) = stacked_bars(
            &mut tree,
            root,
            "Daily hours",
            &[&col0, &col1],
            240.0,
            80.0,
            p,
        );
        let row0 = [0.0, 0.4, 0.9];
        let row1 = [0.2, 0.7, 1.0];
        let (hm, _) = heatmap(
            &mut tree,
            root,
            "Weekly density",
            &[&row0, &row1],
            PROJECT_COLORS[0],
            (14.0, 3.0),
            p,
        );
        let (dn, _) = donut(&mut tree, root, "Share ring", &col0, 72.0, 10.0, p);
        let (pb, _) = proportion_bar(&mut tree, root, "Split bar", &col1, 200.0, 8.0, p);
        let (sp, _) = spark(
            &mut tree,
            root,
            "Weekly trend",
            &[2.0, 4.5, 3.0, 6.0],
            (100.0, 28.0),
            p.accent,
            p,
        );
        let _ = share_row(
            &mut tree,
            root,
            "Project A",
            "2h 15m",
            0.65,
            PROJECT_COLORS[0],
            p,
        );

        tree.layout(600.0, 500.0, &text);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));

        for node in [sb, hm, dn, pb, sp] {
            let r = tree.rect(node);
            assert!(r.w > 0.0 && r.h > 0.0);
            match &tree.data(node).unwrap().leaf {
                Some(Leaf::Canvas(cmds)) => assert!(!cmds.is_empty()),
                _ => panic!("chart node must carry DrawCmds"),
            }
        }
    }
}
