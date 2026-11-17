//! O(visible) virtualized list with proportional scrollbar, wheel clamping, and keyboard nav.

use std::ops::Range;

use taffy::prelude::*;

use super::{add_label, interactive_a11y, Interact};
use crate::ui::input::KeyCode;
use crate::ui::strings;
use crate::ui::text::{Align, Weight};
use crate::ui::theme::Palette;
use crate::ui::tree::{
    style, A11yInfo, A11yRole, DrawCmd, HitMode, Leaf, Node, PaintStyle, Rect, Tree, WidgetId,
};

/// Scroll and selection state for a virtualized list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VirtualListState {
    pub scroll_y: f32,
    pub total_count: usize,
    pub row_height: f32,
    pub viewport_height: f32,
    pub selected: Option<usize>,
}

impl VirtualListState {
    pub fn new(total_count: usize, row_height: f32, viewport_height: f32) -> Self {
        Self {
            scroll_y: 0.0,
            total_count,
            row_height: row_height.max(1.0),
            viewport_height: viewport_height.max(1.0),
            selected: None,
        }
    }

    pub fn max_scroll(&self) -> f32 {
        let total_h = (self.total_count as f32) * self.row_height;
        (total_h - self.viewport_height).max(0.0)
    }

    pub fn clamp_scroll(&mut self) {
        self.scroll_y = self.scroll_y.clamp(0.0, self.max_scroll());
    }

    /// Returns the `[start..end)` slice of visible indices (plus 1-row overscan on each side).
    pub fn visible_range(&self) -> Range<usize> {
        if self.total_count == 0 {
            return 0..0;
        }
        let first = (self.scroll_y / self.row_height).floor().max(0.0) as usize;
        let visible_rows = (self.viewport_height / self.row_height).ceil().max(1.0) as usize;
        let start = first.saturating_sub(1);
        let end = (first + visible_rows + 2).min(self.total_count);
        start..end
    }

    pub fn scroll_by(&mut self, delta_dip: f32) -> bool {
        let prev = self.scroll_y;
        self.scroll_y = (self.scroll_y + delta_dip).clamp(0.0, self.max_scroll());
        (self.scroll_y - prev).abs() > 0.01
    }

    pub fn ensure_visible(&mut self, index: usize) -> bool {
        if index >= self.total_count {
            return false;
        }
        let prev = self.scroll_y;
        let top = (index as f32) * self.row_height;
        let bottom = top + self.row_height;
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if bottom > self.scroll_y + self.viewport_height {
            self.scroll_y = bottom - self.viewport_height;
        }
        self.clamp_scroll();
        (self.scroll_y - prev).abs() > 0.01
    }

    pub fn handle_key(&mut self, key: KeyCode) -> bool {
        if self.total_count == 0 {
            return false;
        }
        let cur = self.selected.unwrap_or(0).min(self.total_count - 1);
        let page = ((self.viewport_height / self.row_height).floor() as usize).max(1);
        let next = match key {
            KeyCode::Down => (cur + 1).min(self.total_count - 1),
            KeyCode::Up => cur.saturating_sub(1),
            KeyCode::PageDown => (cur + page).min(self.total_count - 1),
            KeyCode::PageUp => cur.saturating_sub(page),
            KeyCode::Home => 0,
            KeyCode::End => self.total_count - 1,
            _ => return false,
        };
        let sel_changed = self.selected != Some(next);
        self.selected = Some(next);
        let scrolled = self.ensure_visible(next);
        sel_changed || scrolled
    }
}

pub struct VirtualListProps<'a> {
    pub aria_name: &'a str,
    pub state: VirtualListState,
    pub hover: Option<WidgetId>,
}

pub struct VirtualListBuild {
    pub viewport: Node,
    pub rendered_rows: Vec<(usize, WidgetId)>,
}

/// Builds only the visible window of rows (`O(visible)` nodes) plus a scrollbar thumb.
pub fn build(
    tree: &mut Tree,
    parent: Node,
    props: &VirtualListProps<'_>,
    palette: &Palette,
    mut item_aria_and_build: impl FnMut(&mut Tree, Node, usize, bool) -> String,
) -> VirtualListBuild {
    let mut state = props.state;
    state.clamp_scroll();
    let max_scroll = state.max_scroll();
    let show_bar = max_scroll > 0.5;
    let bar_reserve = if show_bar { 10.0_f32 } else { 0.0_f32 };

    let viewport = tree.add_container(
        parent,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            size: Size {
                width: percent(1.0_f32),
                height: length(state.viewport_height),
            },
            ..Default::default()
        },
    );
    tree.set_paint(
        viewport,
        PaintStyle {
            bg: Some(palette.surface),
            border_color: Some(palette.card_border),
            border_width: palette.border_width,
            radius: 8.0,
            clip: true,
        },
    );
    tree.set_a11y(
        viewport,
        A11yInfo {
            role: A11yRole::List,
            name: props.aria_name.into(),
            requires_name: true,
            ..Default::default()
        },
    );

    if state.total_count == 0 {
        let empty = tree.add_container(
            viewport,
            Style {
                flex_grow: 1.0,
                display: Display::Flex,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
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
        return VirtualListBuild {
            viewport,
            rendered_rows: Vec::new(),
        };
    }

    let range = state.visible_range();
    let mut rendered_rows = Vec::with_capacity(range.len());

    for idx in range {
        let top_dip = (idx as f32) * state.row_height - state.scroll_y;
        let is_selected = state.selected == Some(idx);
        let row = tree.add_container(
            viewport,
            Style {
                position: Position::Absolute,
                inset: taffy::Rect {
                    left: length(0.0_f32),
                    top: length(top_dip),
                    right: length(bar_reserve),
                    bottom: auto(),
                },
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                padding: style::pad(12.0, 0.0, 12.0, 0.0),
                size: Size {
                    width: auto(),
                    height: length(state.row_height),
                },
                ..Default::default()
            },
        );
        let rid = tree.id(row);
        let interact = Interact::from_state(Some(rid), props.hover, None, false);
        let bg = if is_selected {
            Some(palette.accent_bg)
        } else if interact.hovered {
            Some(palette.ctl_hover)
        } else {
            None
        };
        tree.set_paint(
            row,
            PaintStyle {
                bg,
                border_color: Some(palette.hairline),
                border_width: palette.border_width,
                radius: 0.0,
                clip: false,
            },
        );
        tree.set_hit(row, HitMode::Interactive);
        tree.set_focusable(row, true);
        let row_name = item_aria_and_build(tree, row, idx, is_selected);
        let mut info = interactive_a11y(A11yRole::ListItem, row_name, false);
        info.selected = Some(is_selected);
        tree.set_a11y(row, info);
        rendered_rows.push((idx, rid));
    }

    if show_bar {
        let vh = state.viewport_height;
        let total_h = (state.total_count as f32) * state.row_height;
        let thumb_h = ((vh / total_h) * vh).clamp(24.0, vh - 8.0);
        let thumb_y = 4.0 + (state.scroll_y / max_scroll) * (vh - 8.0 - thumb_h).max(0.0);
        let bar = tree.add_leaf(
            viewport,
            Style {
                position: Position::Absolute,
                inset: taffy::Rect {
                    left: auto(),
                    top: length(0.0_f32),
                    right: length(2.0_f32),
                    bottom: length(0.0_f32),
                },
                size: Size {
                    width: length(6.0_f32),
                    height: length(vh),
                },
                ..Default::default()
            },
            Leaf::Canvas(vec![DrawCmd::FillRect {
                rect: Rect {
                    x: 1.0,
                    y: thumb_y,
                    w: 4.0,
                    h: thumb_h,
                },
                color: palette.faint,
                radius: 2.0,
            }]),
        );
        tree.set_a11y(
            bar,
            A11yInfo {
                role: A11yRole::ScrollView,
                name: strings::SCROLL_BAR_LABEL.into(),
                requires_name: true,
                ..Default::default()
            },
        );
    }

    VirtualListBuild {
        viewport,
        rendered_rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::a11y;
    use crate::ui::text::TextSystem;

    #[test]
    fn virtual_list_renders_only_visible_slice_for_large_datasets_and_scrolls() {
        let text = TextSystem::new().expect("dwrite");
        let mut state = VirtualListState::new(100_000, 36.0, 360.0);
        assert!(state.scroll_by(1800.0));
        assert!(state.handle_key(KeyCode::PageDown));

        let mut tree = Tree::new(Style {
            size: style::size(480.0, 400.0),
            ..style::column()
        });
        let root = tree.root();
        let built = build(
            &mut tree,
            root,
            &VirtualListProps {
                aria_name: "Recorded sessions",
                state,
                hover: None,
            },
            &Palette::LIGHT,
            |t, parent, idx, selected| {
                let title = format!("Entry #{}", idx + 1);
                add_label(
                    t,
                    parent,
                    &title,
                    12.5,
                    if selected {
                        Weight::Medium
                    } else {
                        Weight::Regular
                    },
                    Align::Left,
                    Palette::LIGHT.text,
                );
                title
            },
        );

        tree.layout(480.0, 400.0, &text);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));
        // 360px / 36px = 10 visible + overscan <= 14 items even out of 100,000 rows!
        assert!(
            built.rendered_rows.len() <= 14,
            "rendered too many rows: {}",
            built.rendered_rows.len()
        );
        assert!(tree.all_nodes().len() < 40);
    }
}
