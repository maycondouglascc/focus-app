//! Pill-shaped segmented option switcher (e.g. Day / Week, Today / Reports).

use taffy::prelude::*;

use super::{add_label, interactive_a11y, Interact};
use crate::ui::input::KeyCode;
use crate::ui::text::{Align, Weight};
use crate::ui::theme::Palette;
use crate::ui::tree::{style, A11yInfo, A11yRole, HitMode, Node, PaintStyle, Tree, WidgetId};

#[derive(Clone, Debug)]
pub struct SegmentedProps<'a> {
    pub group_name: &'a str,
    pub options: &'a [&'a str],
    pub selected: usize,
    pub hover: Option<WidgetId>,
    pub disabled: bool,
}

/// Builds a segmented control on `palette.seg_track` and returns `(group_node, option_ids)`.
pub fn build(
    tree: &mut Tree,
    parent: Node,
    props: &SegmentedProps<'_>,
    palette: &Palette,
) -> (Node, Vec<WidgetId>) {
    build_keyed(tree, parent, props, None, palette)
}

/// Builds a segmented control with optional stable widget keys (`"<prefix>.<i>"`).
pub fn build_keyed(
    tree: &mut Tree,
    parent: Node,
    props: &SegmentedProps<'_>,
    key_prefix: Option<&str>,
    palette: &Palette,
) -> (Node, Vec<WidgetId>) {
    let track = tree.add_container(
        parent,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            align_items: Some(AlignItems::Center),
            gap: style::gap(2.0),
            padding: style::pad(3.0, 3.0, 3.0, 3.0),
            size: Size {
                width: auto(),
                height: length(32.0_f32),
            },
            ..Default::default()
        },
    );
    tree.set_paint(
        track,
        PaintStyle {
            bg: Some(palette.seg_track),
            border_color: Some(palette.hairline),
            border_width: palette.border_width,
            radius: 7.0,
            clip: false,
        },
    );
    tree.set_a11y(
        track,
        A11yInfo {
            role: A11yRole::Group,
            name: props.group_name.into(),
            requires_name: true,
            ..Default::default()
        },
    );

    let mut ids = Vec::with_capacity(props.options.len());
    for (idx, label) in props.options.iter().enumerate() {
        let is_selected = idx == props.selected;
        let item = tree.add_container(
            track,
            Style {
                display: Display::Flex,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::Center),
                padding: style::pad(10.0, 0.0, 10.0, 0.0),
                size: Size {
                    width: auto(),
                    height: length(26.0_f32),
                },
                ..Default::default()
            },
        );
        let item_id = match key_prefix {
            Some(prefix) => tree.set_key(item, &format!("{prefix}.{idx}")),
            None => tree.id(item),
        };
        let interact = Interact::from_state(Some(item_id), props.hover, None, props.disabled);
        let bg = if is_selected {
            Some(palette.surface)
        } else if interact.hovered {
            Some(palette.ctl_hover)
        } else {
            None
        };
        let fg = if props.disabled {
            palette.disabled_text
        } else if is_selected {
            palette.text
        } else {
            palette.muted
        };
        tree.set_paint(
            item,
            PaintStyle {
                bg,
                border_color: if is_selected {
                    Some(palette.card_border)
                } else {
                    None
                },
                border_width: if is_selected {
                    palette.border_width
                } else {
                    0.0
                },
                radius: 5.0,
                clip: false,
            },
        );
        tree.set_hit(item, HitMode::Interactive);
        tree.set_focusable(item, true);
        tree.set_disabled(item, props.disabled);

        let mut a11y = interactive_a11y(A11yRole::RadioButton, *label, props.disabled);
        a11y.selected = Some(is_selected);
        tree.set_a11y(item, a11y);

        add_label(
            tree,
            item,
            *label,
            12.0,
            if is_selected {
                Weight::SemiBold
            } else {
                Weight::Medium
            },
            Align::Center,
            fg,
        );
        ids.push(item_id);
    }

    (track, ids)
}

/// Applies Left/Right/Home/End key navigation inside a segmented control.
pub fn handle_key(key: KeyCode, current: usize, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    match key {
        KeyCode::Left | KeyCode::Up => Some(if current == 0 { count - 1 } else { current - 1 }),
        KeyCode::Right | KeyCode::Down => Some((current + 1) % count),
        KeyCode::Home => Some(0),
        KeyCode::End => Some(count - 1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::a11y;
    use crate::ui::text::TextSystem;

    #[test]
    fn segmented_selection_navigation_and_a11y_work() {
        let text = TextSystem::new().expect("dwrite");
        let mut tree = Tree::new(style::row());
        let root = tree.root();
        let (_group, ids) = build(
            &mut tree,
            root,
            &SegmentedProps {
                group_name: "Period range",
                options: &["Day", "Week", "Month"],
                selected: 1,
                hover: None,
                disabled: false,
            },
            &Palette::LIGHT,
        );
        tree.layout(400.0, 60.0, &text);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));
        assert_eq!(ids.len(), 3);

        let n1 = tree.node_by_id(ids[1]).unwrap();
        let n0 = tree.node_by_id(ids[0]).unwrap();
        assert_eq!(tree.data(n1).unwrap().a11y.selected, Some(true));
        assert_eq!(tree.data(n0).unwrap().a11y.selected, Some(false));

        assert_eq!(handle_key(KeyCode::Left, 0, 3), Some(2));
        assert_eq!(handle_key(KeyCode::Right, 2, 3), Some(0));
    }
}
