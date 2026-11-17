//! Boolean toggles (`Switch` and `Checkbox`) sharing checked state and
//! keyboard/pointer activation semantics.

use taffy::prelude::*;

use super::{add_label, glyph, interactive_a11y, Interact};
use crate::ui::text::{Align, Weight};
use crate::ui::theme::Palette;
use crate::ui::tree::{
    style, A11yRole, DrawCmd, HitMode, Leaf, Node, PaintStyle, Rect, Tree, WidgetId,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToggleKind {
    Switch,
    Checkbox,
}

#[derive(Clone, Debug)]
pub struct ToggleProps<'a> {
    pub kind: ToggleKind,
    pub label: &'a str,
    pub aria_label: Option<&'a str>,
    pub checked: bool,
    pub disabled: bool,
    pub interact: Interact,
}

/// Builds a switch (`36x20` pill track) or checkbox (`16x16` box) with an optional inline label.
pub fn build(
    tree: &mut Tree,
    parent: Node,
    props: &ToggleProps<'_>,
    palette: &Palette,
) -> (Node, WidgetId) {
    let row = tree.add_container(
        parent,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            align_items: Some(AlignItems::Center),
            gap: style::gap(8.0),
            min_size: Size {
                width: auto(),
                height: length(28.0_f32),
            },
            ..Default::default()
        },
    );
    tree.set_hit(row, HitMode::Interactive);
    tree.set_focusable(row, true);
    tree.set_disabled(row, props.disabled);

    let role = match props.kind {
        ToggleKind::Switch => A11yRole::Switch,
        ToggleKind::Checkbox => A11yRole::CheckBox,
    };
    let mut info = interactive_a11y(
        role,
        props.aria_label.unwrap_or(props.label),
        props.disabled,
    );
    info.toggled = Some(props.checked);
    tree.set_a11y(row, info);

    match props.kind {
        ToggleKind::Switch => {
            let track_color = if props.disabled {
                palette.disabled_bg
            } else if props.checked {
                palette.toggle_on
            } else if props.interact.hovered {
                palette.ctl_pressed
            } else {
                palette.toggle_off
            };
            let thumb_x = if props.checked { 26.0 } else { 10.0 };
            let thumb_color = if props.disabled {
                palette.disabled_text
            } else {
                palette.surface
            };
            tree.add_leaf(
                row,
                Style {
                    size: style::size(36.0, 20.0),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
                Leaf::Canvas(vec![
                    DrawCmd::FillRect {
                        rect: Rect {
                            x: 0.0,
                            y: 0.0,
                            w: 36.0,
                            h: 20.0,
                        },
                        color: track_color,
                        radius: 10.0,
                    },
                    DrawCmd::FillCircle {
                        cx: thumb_x,
                        cy: 10.0,
                        r: 7.0,
                        color: thumb_color,
                    },
                ]),
            );
        }
        ToggleKind::Checkbox => {
            let box_bg = if props.disabled {
                palette.disabled_bg
            } else if props.checked {
                palette.primary_bg
            } else if props.interact.hovered {
                palette.ctl_hover
            } else {
                palette.ctl_bg
            };
            let box_border = if props.checked && !props.disabled {
                palette.primary_bg
            } else {
                palette.ctl_border
            };
            let box_node = tree.add_container(
                row,
                Style {
                    display: Display::Flex,
                    align_items: Some(AlignItems::Center),
                    justify_content: Some(JustifyContent::Center),
                    size: style::size(16.0, 16.0),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            );
            tree.set_paint(
                box_node,
                PaintStyle {
                    bg: Some(box_bg),
                    border_color: Some(box_border),
                    border_width: palette.border_width,
                    radius: 4.0,
                    clip: false,
                },
            );
            if props.checked {
                tree.add_leaf(
                    box_node,
                    Style::default(),
                    Leaf::Icon {
                        glyph: glyph::CHECK,
                        size: 11.0,
                        color: if props.disabled {
                            palette.disabled_text
                        } else {
                            palette.on_primary
                        },
                    },
                );
            }
        }
    }

    if !props.label.is_empty() {
        let fg = if props.disabled {
            palette.disabled_text
        } else {
            palette.text
        };
        add_label(
            tree,
            row,
            props.label,
            12.5,
            Weight::Regular,
            Align::Left,
            fg,
        );
    }

    (row, tree.id(row))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::a11y;
    use crate::ui::text::TextSystem;

    #[test]
    fn switch_and_checkbox_publish_toggled_state_and_bounds() {
        let text = TextSystem::new().expect("dwrite");
        let mut tree = Tree::new(style::column());
        let root = tree.root();

        let (sw, sw_id) = build(
            &mut tree,
            root,
            &ToggleProps {
                kind: ToggleKind::Switch,
                label: "Idle detection",
                aria_label: None,
                checked: true,
                disabled: false,
                interact: Interact::default(),
            },
            &Palette::LIGHT,
        );
        let (cb, cb_id) = build(
            &mut tree,
            root,
            &ToggleProps {
                kind: ToggleKind::Checkbox,
                label: "Billable",
                aria_label: None,
                checked: false,
                disabled: false,
                interact: Interact::default(),
            },
            &Palette::LIGHT,
        );

        tree.layout(300.0, 120.0, &text);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));
        assert_eq!(tree.data(sw).unwrap().a11y.toggled, Some(true));
        assert_eq!(tree.data(cb).unwrap().a11y.toggled, Some(false));

        let swr = tree.rect(sw);
        let cbr = tree.rect(cb);
        assert_eq!(tree.hit_test(swr.x + 8.0, swr.y + 8.0), Some(sw_id));
        assert_eq!(tree.hit_test(cbr.x + 8.0, cbr.y + 8.0), Some(cb_id));
    }
}
