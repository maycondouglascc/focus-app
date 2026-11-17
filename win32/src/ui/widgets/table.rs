//! Data table with sortable headers, hairline separators, row hover/selection,
//! and complete Table/Row/ColumnHeader/Cell accessibility metadata.

use taffy::prelude::*;

use super::{add_label, glyph, interactive_a11y, Interact};
use crate::ui::strings;
use crate::ui::text::{Align, Weight};
use crate::ui::theme::Palette;
use crate::ui::tree::{style, A11yInfo, A11yRole, HitMode, Leaf, Node, PaintStyle, Tree, WidgetId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

impl SortDir {
    pub fn toggled(self) -> Self {
        match self {
            Self::Asc => Self::Desc,
            Self::Desc => Self::Asc,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortState {
    pub col: usize,
    pub dir: SortDir,
}

#[derive(Clone, Debug)]
pub struct TableColumn<'a> {
    pub label: &'a str,
    pub width: Option<f32>,
    pub align: Align,
    pub sortable: bool,
}

#[derive(Clone, Debug)]
pub struct TableRow<'a> {
    pub aria_name: &'a str,
    pub cells: &'a [&'a str],
    pub selected: bool,
}

pub struct TableBuild {
    pub root: Node,
    pub header_ids: Vec<Option<WidgetId>>,
    pub row_ids: Vec<WidgetId>,
}

pub struct TableProps<'a> {
    pub aria_name: &'a str,
    pub columns: &'a [TableColumn<'a>],
    pub rows: &'a [TableRow<'a>],
    pub sort: Option<SortState>,
    pub hover: Option<WidgetId>,
}

fn cell_style(col: &TableColumn<'_>, align: Align) -> Style {
    Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Row,
        align_items: Some(AlignItems::Center),
        justify_content: Some(match align {
            Align::Right => JustifyContent::FlexEnd,
            Align::Center => JustifyContent::Center,
            Align::Left => JustifyContent::FlexStart,
        }),
        gap: style::gap(4.0),
        flex_grow: if col.width.is_some() { 0.0 } else { 1.0 },
        flex_shrink: if col.width.is_some() { 0.0 } else { 1.0 },
        size: Size {
            width: match col.width {
                Some(w) => length(w),
                None => auto(),
            },
            height: auto(),
        },
        ..Default::default()
    }
}

/// Builds a full table (`props.columns` + `props.rows`) under `parent` in `tree`.
pub fn build(
    tree: &mut Tree,
    parent: Node,
    props: &TableProps<'_>,
    palette: &Palette,
) -> TableBuild {
    let columns = props.columns;
    let rows = props.rows;
    let aria_name = props.aria_name;
    let sort = props.sort;
    let hover = props.hover;
    let table = tree.add_container(
        parent,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            size: Size {
                width: percent(1.0_f32),
                height: auto(),
            },
            ..Default::default()
        },
    );
    tree.set_paint(
        table,
        PaintStyle {
            bg: Some(palette.surface),
            border_color: Some(palette.card_border),
            border_width: palette.border_width,
            radius: 8.0,
            clip: true,
        },
    );
    tree.set_a11y(
        table,
        A11yInfo {
            role: A11yRole::Table,
            name: aria_name.into(),
            requires_name: true,
            ..Default::default()
        },
    );

    // Header row
    let hdr_row = tree.add_container(
        table,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            align_items: Some(AlignItems::Center),
            gap: style::gap(12.0),
            padding: style::pad(14.0, 0.0, 14.0, 0.0),
            size: Size {
                width: percent(1.0_f32),
                height: length(34.0_f32),
            },
            ..Default::default()
        },
    );
    tree.set_paint(
        hdr_row,
        PaintStyle {
            bg: Some(palette.ctl_hover),
            border_color: Some(palette.hairline),
            border_width: palette.border_width,
            radius: 0.0,
            clip: false,
        },
    );
    tree.set_a11y(
        hdr_row,
        A11yInfo {
            role: A11yRole::Row,
            name: aria_name.into(),
            requires_name: true,
            ..Default::default()
        },
    );

    let mut header_ids = Vec::with_capacity(columns.len());
    for (idx, col) in columns.iter().enumerate() {
        let hcell = tree.add_container(hdr_row, cell_style(col, col.align));
        let sorted = sort.filter(|s| s.col == idx);
        let a11y_label = match sorted {
            Some(s) => format!(
                "{}, {}",
                col.label,
                if s.dir == SortDir::Asc {
                    strings::SORT_ASC_SUFFIX
                } else {
                    strings::SORT_DESC_SUFFIX
                }
            ),
            None => col.label.into(),
        };
        if col.sortable {
            let hid = tree.id(hcell);
            let interact = Interact::from_state(Some(hid), hover, None, false);
            tree.set_hit(hcell, HitMode::Interactive);
            tree.set_focusable(hcell, true);
            tree.set_a11y(
                hcell,
                interactive_a11y(A11yRole::ColumnHeader, a11y_label, false),
            );
            add_label(
                tree,
                hcell,
                col.label,
                11.5,
                Weight::SemiBold,
                col.align,
                if interact.hovered {
                    palette.text
                } else {
                    palette.muted
                },
            );
            if let Some(s) = sorted {
                tree.add_leaf(
                    hcell,
                    Style::default(),
                    Leaf::Icon {
                        glyph: match s.dir {
                            SortDir::Asc => glyph::CHEVRON_UP,
                            SortDir::Desc => glyph::CHEVRON_DOWN,
                        },
                        size: 9.0,
                        color: palette.text,
                    },
                );
            }
            header_ids.push(Some(hid));
        } else {
            tree.set_a11y(
                hcell,
                A11yInfo {
                    role: A11yRole::ColumnHeader,
                    name: a11y_label,
                    requires_name: true,
                    ..Default::default()
                },
            );
            add_label(
                tree,
                hcell,
                col.label,
                11.5,
                Weight::SemiBold,
                col.align,
                palette.muted,
            );
            header_ids.push(None);
        }
    }

    if rows.is_empty() {
        let empty = tree.add_container(
            table,
            Style {
                display: Display::Flex,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                padding: style::pad(16.0, 20.0, 16.0, 20.0),
                ..Default::default()
            },
        );
        add_label(
            tree,
            empty,
            strings::LIST_EMPTY,
            12.5,
            Weight::Regular,
            Align::Center,
            palette.muted,
        );
        return TableBuild {
            root: table,
            header_ids,
            row_ids: Vec::new(),
        };
    }

    let mut row_ids = Vec::with_capacity(rows.len());
    for row in rows {
        let rnode = tree.add_container(
            table,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                gap: style::gap(12.0),
                padding: style::pad(14.0, 0.0, 14.0, 0.0),
                size: Size {
                    width: percent(1.0_f32),
                    height: length(36.0_f32),
                },
                ..Default::default()
            },
        );
        let rid = tree.id(rnode);
        let interact = Interact::from_state(Some(rid), hover, None, false);
        let bg = if row.selected {
            Some(palette.accent_bg)
        } else if interact.hovered {
            Some(palette.ctl_hover)
        } else {
            None
        };
        tree.set_paint(
            rnode,
            PaintStyle {
                bg,
                border_color: Some(palette.hairline),
                border_width: palette.border_width,
                radius: 0.0,
                clip: false,
            },
        );
        tree.set_hit(rnode, HitMode::Interactive);
        tree.set_focusable(rnode, true);
        let mut rinfo = interactive_a11y(A11yRole::Row, row.aria_name, false);
        rinfo.selected = Some(row.selected);
        tree.set_a11y(rnode, rinfo);

        for (cidx, col) in columns.iter().enumerate() {
            let txt = row.cells.get(cidx).copied().unwrap_or("-");
            let cell = tree.add_container(rnode, cell_style(col, col.align));
            tree.set_a11y(
                cell,
                A11yInfo {
                    role: A11yRole::Cell,
                    name: txt.into(),
                    requires_name: true,
                    ..Default::default()
                },
            );
            add_label(
                tree,
                cell,
                txt,
                12.5,
                if cidx == 0 {
                    Weight::Medium
                } else {
                    Weight::Regular
                },
                col.align,
                palette.text,
            );
        }
        row_ids.push(rid);
    }

    TableBuild {
        root: table,
        header_ids,
        row_ids,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::a11y;
    use crate::ui::text::TextSystem;

    #[test]
    fn table_lays_out_columns_headers_rows_and_validates_a11y() {
        let text = TextSystem::new().expect("dwrite");
        let cols = [
            TableColumn {
                label: "Category",
                width: None,
                align: Align::Left,
                sortable: true,
            },
            TableColumn {
                label: "Duration",
                width: Some(90.0),
                align: Align::Right,
                sortable: true,
            },
        ];
        let rows = [
            TableRow {
                aria_name: "Category A, 1h 20m",
                cells: &["Category A", "1h 20m"],
                selected: true,
            },
            TableRow {
                aria_name: "Category B, 35m",
                cells: &["Category B", "35m"],
                selected: false,
            },
        ];

        let mut tree = Tree::new(Style {
            size: style::size(500.0, 300.0),
            ..style::column()
        });
        let root = tree.root();
        let built = build(
            &mut tree,
            root,
            &TableProps {
                aria_name: "Time breakdown",
                columns: &cols,
                rows: &rows,
                sort: Some(SortState {
                    col: 1,
                    dir: SortDir::Desc,
                }),
                hover: None,
            },
            &Palette::LIGHT,
        );
        tree.layout(500.0, 300.0, &text);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));
        assert_eq!(built.header_ids.len(), 2);
        assert_eq!(built.row_ids.len(), 2);

        let r0 = tree.rect(tree.node_by_id(built.row_ids[0]).unwrap());
        let r1 = tree.rect(tree.node_by_id(built.row_ids[1]).unwrap());
        assert!(r1.y >= r0.y + r0.h - 0.1);
        assert_eq!(SortDir::Desc.toggled(), SortDir::Asc);
    }
}
