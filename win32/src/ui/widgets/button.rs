//! Push buttons matching Focus variants (`Secondary`, `Primary`, `Ghost`, `Danger`)
//! and sizes (`Regular`, `Small`, `Icon`).

use taffy::prelude::*;

use super::{add_label, interactive_a11y, Interact};
use crate::ui::text::{Align, Weight};
use crate::ui::theme::{Color, Palette};
use crate::ui::tree::{style, A11yRole, HitMode, Leaf, Node, PaintStyle, Tree, WidgetId};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    #[default]
    Secondary,
    Primary,
    Ghost,
    Danger,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonSize {
    #[default]
    Regular,
    Small,
    Icon,
}

#[derive(Clone, Debug)]
pub struct ButtonProps<'a> {
    pub label: &'a str,
    pub aria_label: Option<&'a str>,
    pub icon: Option<char>,
    pub variant: ButtonVariant,
    pub size: ButtonSize,
    pub disabled: bool,
    pub interact: Interact,
}

impl<'a> ButtonProps<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            aria_label: None,
            icon: None,
            variant: ButtonVariant::Secondary,
            size: ButtonSize::Regular,
            disabled: false,
            interact: Interact::default(),
        }
    }
}

struct ButtonColors {
    bg: Option<Color>,
    border: Option<Color>,
    fg: Color,
}

fn resolve_colors(variant: ButtonVariant, interact: Interact, p: &Palette) -> ButtonColors {
    if interact.disabled {
        return ButtonColors {
            bg: Some(p.disabled_bg),
            border: match variant {
                ButtonVariant::Ghost => None,
                _ => Some(p.ctl_border),
            },
            fg: p.disabled_text,
        };
    }

    match variant {
        ButtonVariant::Primary => ButtonColors {
            bg: Some(if interact.pressed || interact.hovered {
                p.primary_hover
            } else {
                p.primary_bg
            }),
            border: None,
            fg: p.on_primary,
        },
        ButtonVariant::Danger => ButtonColors {
            bg: Some(if interact.pressed {
                p.danger_bg
            } else if interact.hovered {
                p.ctl_hover
            } else {
                p.ctl_bg
            }),
            border: Some(p.danger),
            fg: p.danger_text,
        },
        ButtonVariant::Ghost => ButtonColors {
            bg: if interact.pressed {
                Some(p.ctl_pressed)
            } else if interact.hovered {
                Some(p.ctl_hover)
            } else {
                None
            },
            border: None,
            fg: p.text,
        },
        ButtonVariant::Secondary => ButtonColors {
            bg: Some(if interact.pressed {
                p.ctl_pressed
            } else if interact.hovered {
                p.ctl_hover
            } else {
                p.ctl_bg
            }),
            border: Some(p.ctl_border),
            fg: p.text,
        },
    }
}

/// Adds a button node to `tree` under `parent` and returns `(node, WidgetId)`.
pub fn build(
    tree: &mut Tree,
    parent: Node,
    props: &ButtonProps<'_>,
    palette: &Palette,
) -> (Node, WidgetId) {
    let (h, pad_x, font_size, radius) = match props.size {
        ButtonSize::Regular => (32.0_f32, 14.0, 12.5, 6.0),
        ButtonSize::Small => (26.0_f32, 10.0, 12.0, 5.0),
        ButtonSize::Icon => (28.0_f32, 0.0, 12.5, 6.0),
    };

    let mut st = Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Row,
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        gap: style::gap(6.0),
        padding: style::pad(pad_x, 0.0, pad_x, 0.0),
        size: Size {
            width: if props.size == ButtonSize::Icon {
                length(28.0_f32)
            } else {
                auto()
            },
            height: length(h),
        },
        ..Default::default()
    };
    if props.size == ButtonSize::Icon {
        st.min_size = style::size(28.0, 28.0);
    }

    let colors = resolve_colors(props.variant, props.interact, palette);
    let node = tree.add_container(parent, st);
    tree.set_paint(
        node,
        PaintStyle {
            bg: colors.bg,
            border_color: colors.border,
            border_width: if colors.border.is_some() {
                palette.border_width
            } else {
                0.0
            },
            radius,
            clip: false,
        },
    );
    tree.set_hit(node, HitMode::Interactive);
    tree.set_focusable(node, true);
    tree.set_disabled(node, props.disabled);

    let name = props.aria_label.unwrap_or(props.label);
    let role = if props.variant == ButtonVariant::Primary {
        A11yRole::DefaultButton
    } else {
        A11yRole::Button
    };
    tree.set_a11y(node, interactive_a11y(role, name, props.disabled));

    if let Some(glyph) = props.icon {
        tree.add_leaf(
            node,
            Style::default(),
            Leaf::Icon {
                glyph,
                size: 13.0,
                color: colors.fg,
            },
        );
    }
    if props.size != ButtonSize::Icon && !props.label.is_empty() {
        add_label(
            tree,
            node,
            props.label,
            font_size,
            Weight::Medium,
            Align::Center,
            colors.fg,
        );
    }

    (node, tree.id(node))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::a11y;
    use crate::ui::text::TextSystem;

    #[test]
    fn button_sizes_hit_testing_and_a11y_names_work() {
        let text = TextSystem::new().expect("dwrite");
        let mut tree = Tree::new(style::row());
        let root = tree.root();
        let mut props = ButtonProps::new("Save changes");
        props.variant = ButtonVariant::Primary;
        let (btn, btn_id) = build(&mut tree, root, &props, &Palette::LIGHT);

        let mut icon_props = ButtonProps::new("");
        icon_props.aria_label = Some("Close dialog");
        icon_props.icon = Some(super::super::glyph::CLOSE);
        icon_props.size = ButtonSize::Icon;
        let (icon_btn, icon_id) = build(&mut tree, root, &icon_props, &Palette::LIGHT);

        tree.layout(400.0, 100.0, &text);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));

        let r = tree.rect(btn);
        assert_eq!(r.h, 32.0);
        assert!(r.w > 40.0);
        assert_eq!(tree.hit_test(r.x + 8.0, r.y + 8.0), Some(btn_id));

        let ir = tree.rect(icon_btn);
        assert_eq!((ir.w, ir.h), (28.0, 28.0));
        assert_eq!(tree.hit_test(ir.x + 4.0, ir.y + 4.0), Some(icon_id));
    }
}
