//! Application screens mounted inside `AppShell` or standalone for tests.

pub mod activity;
pub mod activity_popover;
pub mod calendar;
pub mod calendar_layout;
pub mod entry_editor;
pub mod entry_tree;
pub mod project_report;
pub mod projects;
pub mod reports;
pub mod settings;
pub mod timer_bar;
pub mod week_grid;
pub mod welcome;

use std::cell::RefCell;
use std::rc::Rc;

use taffy::prelude::*;

use crate::ui::popup;
use crate::ui::screen::ScreenEnv;
use crate::ui::text::{Align, Weight, Wrap};
use crate::ui::theme::Palette;
use crate::ui::tree::{style, Leaf, Node, PaintStyle, Tree, WidgetId};
use crate::ui::widgets::add_label;
use crate::ui::widgets::select;
use crate::ui::window;

/// Reusable popup handle for a single `Select` trigger button inside a screen or modal.
#[derive(Default)]
pub struct PopupSelectSlot {
    pub trigger_id: Option<WidgetId>,
    result: Option<Rc<RefCell<Option<usize>>>>,
}

impl PopupSelectSlot {
    pub fn is_open(&self) -> bool {
        self.result.is_some()
    }

    pub fn open(&mut self, options: &[&str], selected: Option<usize>, palette: &Palette) -> bool {
        let Some(id) = self.trigger_id else {
            return false;
        };
        let (Some(hwnd), Some(rect)) = (window::hwnd(), window::widget_rect(id)) else {
            return false;
        };
        self.result = select::open_popup(hwnd, rect, options, selected, palette);
        self.result.is_some()
    }

    /// Returns `Some(Some(idx))` when user picked an option, `Some(None)` if dismissed,
    /// or `None` if still open / inactive.
    pub fn poll(&mut self) -> Option<Option<usize>> {
        let res = self.result.as_ref()?;
        let picked = *res.borrow();
        if picked.is_some() {
            self.result = None;
            return Some(picked);
        }
        if !popup::is_open() {
            self.result = None;
            return Some(None);
        }
        None
    }
}

/// Adjusts vertical scroll offset cleanly on wheel notches.
pub fn scroll_wheel_step(scroll_y: &mut f32, delta_dip: f32, max_scroll: f32) -> bool {
    let next = (*scroll_y - delta_dip).clamp(0.0, max_scroll.max(0.0));
    if (next - *scroll_y).abs() > 0.2 {
        *scroll_y = next;
        true
    } else {
        false
    }
}

/// Creates a standalone screen root `Tree` matching `ScreenEnv` viewport dimensions.
pub fn screen_root_tree(env: &ScreenEnv<'_>) -> Tree {
    let mut tree = Tree::new(Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Column,
        size: style::size(env.width_dip, env.height_dip),
        ..Default::default()
    });
    let root = tree.root();
    tree.set_paint(
        root,
        PaintStyle {
            bg: Some(env.palette.backdrop),
            border_color: None,
            border_width: 0.0,
            radius: 0.0,
            clip: true,
        },
    );
    tree
}

/// Builds a scrollable full-height viewport + inner padded centered column inside `parent`.
/// Returns `(viewport_node, inner_column_node)`.
pub fn build_scroll_page(
    tree: &mut Tree,
    parent: Node,
    max_width_dip: f32,
    padding_dip: f32,
    gap_dip: f32,
    scroll_y: f32,
) -> (Node, Node) {
    let viewport = tree.add_container(
        parent,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            align_items: Some(AlignItems::Center),
            flex_grow: 1.0,
            size: Size {
                width: percent(1.0_f32),
                height: percent(1.0_f32),
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
    tree.set_scroll(viewport, 0.0, scroll_y);

    let col = tree.add_container(
        viewport,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
            padding: style::pad(padding_dip, padding_dip, padding_dip, padding_dip),
            gap: style::gap(gap_dip),
            size: Size {
                width: percent(1.0_f32),
                height: auto(),
            },
            max_size: Size {
                width: length(max_width_dip),
                height: auto(),
            },
            ..Default::default()
        },
    );
    (viewport, col)
}

/// Builds a rounded surface card matching the Focus `.lo-card` design token.
pub fn build_card(
    tree: &mut Tree,
    parent: Node,
    padding_dip: f32,
    gap_dip: f32,
    palette: &Palette,
) -> Node {
    let card = tree.add_container(
        parent,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            flex_shrink: 0.0,
            padding: style::pad(padding_dip, padding_dip, padding_dip, padding_dip),
            gap: style::gap(gap_dip),
            size: Size {
                width: percent(1.0_f32),
                height: auto(),
            },
            ..Default::default()
        },
    );
    tree.set_paint(
        card,
        PaintStyle {
            bg: Some(palette.surface),
            border_color: Some(palette.card_border),
            border_width: palette.border_width,
            radius: 10.0,
            clip: true,
        },
    );
    card
}

/// Horizontal hairline separator inside cards.
pub fn build_hairline(tree: &mut Tree, parent: Node, palette: &Palette) {
    let line = tree.add_container(
        parent,
        Style {
            flex_shrink: 0.0,
            size: Size {
                width: percent(1.0_f32),
                height: length(palette.border_width.max(1.0)),
            },
            ..Default::default()
        },
    );
    tree.set_paint(
        line,
        PaintStyle {
            bg: Some(palette.hairline),
            border_color: None,
            border_width: 0.0,
            radius: 0.0,
            clip: false,
        },
    );
}

/// Builds a section header above a card.
pub fn build_section_title(tree: &mut Tree, parent: Node, title: &str, palette: &Palette) {
    let box_node = tree.add_container(
        parent,
        Style {
            flex_shrink: 0.0,
            padding: style::pad(2.0, 6.0, 0.0, 0.0),
            ..Default::default()
        },
    );
    add_label(
        tree,
        box_node,
        title,
        13.0,
        Weight::SemiBold,
        Align::Left,
        palette.text,
    );
}

/// Builds a uniform KPI summary tile (label + large tabular figure + optional subtitle).
pub fn build_stat_card(
    tree: &mut Tree,
    parent: Node,
    label: &str,
    value: &str,
    sub: Option<&str>,
    palette: &Palette,
) -> Node {
    let tile = tree.add_container(
        parent,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            padding: style::pad(14.0, 12.0, 14.0, 12.0),
            gap: style::gap(4.0),
            flex_grow: 1.0,
            flex_basis: length(0.0_f32),
            ..Default::default()
        },
    );
    tree.set_paint(
        tile,
        PaintStyle {
            bg: Some(palette.surface),
            border_color: Some(palette.card_border),
            border_width: palette.border_width,
            radius: 10.0,
            clip: false,
        },
    );
    add_label(
        tree,
        tile,
        label,
        11.5,
        Weight::Medium,
        Align::Left,
        palette.muted,
    );
    add_label(
        tree,
        tile,
        value,
        20.0,
        Weight::SemiBold,
        Align::Left,
        palette.text,
    );
    if let Some(sub_txt) = sub {
        tree.add_leaf(
            tile,
            Style::default(),
            Leaf::StyledText {
                text: sub_txt.into(),
                size: 11.5,
                weight: Weight::Regular,
                align: Align::Left,
                wrap: Wrap::Word,
                color: palette.faint,
            },
        );
    }
    tile
}

/// Builds a centered card empty state with icon, title, and muted description.
pub fn build_empty_state(
    tree: &mut Tree,
    parent: Node,
    icon: char,
    title: &str,
    subtitle: &str,
    palette: &Palette,
) -> Node {
    let card = build_card(tree, parent, 28.0, 8.0, palette);
    let inner = tree.add_container(
        card,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            align_items: Some(AlignItems::Center),
            gap: style::gap(6.0),
            size: Size {
                width: percent(1.0_f32),
                height: auto(),
            },
            ..Default::default()
        },
    );
    tree.add_leaf(
        inner,
        Style::default(),
        Leaf::Icon {
            glyph: icon,
            size: 24.0,
            color: palette.muted,
        },
    );
    add_label(
        tree,
        inner,
        title,
        14.0,
        Weight::SemiBold,
        Align::Center,
        palette.text,
    );
    tree.add_leaf(
        inner,
        Style::default(),
        Leaf::StyledText {
            text: subtitle.into(),
            size: 12.5,
            weight: Weight::Regular,
            align: Align::Center,
            wrap: Wrap::Word,
            color: palette.muted,
        },
    );
    inner
}
