//! Modal dialog overlay (`scrim` + centered card + header with Close button)
//! paired with `FocusScope::Modal` focus trapping and `Esc` dismissal.

use taffy::prelude::*;

use super::button::{self, ButtonProps, ButtonSize, ButtonVariant};
use super::{add_label, glyph, Interact};
use crate::ui::focus::{FocusModel, FocusScope};
use crate::ui::strings;
use crate::ui::text::{Align, Weight};
use crate::ui::theme::Palette;
use crate::ui::tree::{style, A11yInfo, A11yRole, HitMode, Node, PaintStyle, Tree, WidgetId};

/// Handles to the built modal dialog nodes: `(scrim, card, body_container, close_button_id)`.
pub struct DialogNodes {
    pub scrim: Node,
    pub card: Node,
    pub body: Node,
    pub close_id: WidgetId,
}

/// Appends a full-viewport modal overlay onto `root` inside `tree`.
pub fn build(
    tree: &mut Tree,
    root: Node,
    title: &str,
    width_dip: f32,
    hover: Option<WidgetId>,
    palette: &Palette,
) -> DialogNodes {
    let scrim = tree.add_container(
        root,
        Style {
            position: Position::Absolute,
            inset: taffy::Rect {
                left: length(0.0_f32),
                top: length(0.0_f32),
                right: length(0.0_f32),
                bottom: length(0.0_f32),
            },
            display: Display::Flex,
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::Center),
            padding: style::pad(24.0, 24.0, 24.0, 24.0),
            ..Default::default()
        },
    );
    tree.set_paint(
        scrim,
        PaintStyle {
            bg: Some(palette.scrim),
            border_color: None,
            border_width: 0.0,
            radius: 0.0,
            clip: true,
        },
    );
    // Scrim absorbs background clicks without closing unless explicitly wired by the screen.
    tree.set_hit(scrim, HitMode::Opaque);

    let card = tree.add_container(
        scrim,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            size: Size {
                width: length(width_dip.clamp(280.0, 640.0)),
                height: auto(),
            },
            max_size: Size {
                width: percent(1.0_f32),
                height: percent(1.0_f32),
            },
            padding: style::pad(20.0, 18.0, 20.0, 20.0),
            gap: style::gap(14.0),
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
    tree.set_hit(card, HitMode::Opaque);
    tree.set_a11y(
        card,
        A11yInfo {
            role: A11yRole::Dialog,
            name: title.into(),
            modal: true,
            requires_name: true,
            ..Default::default()
        },
    );

    let header = tree.add_container(
        card,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::SpaceBetween),
            gap: style::gap(12.0),
            ..Default::default()
        },
    );
    add_label(
        tree,
        header,
        title,
        15.0,
        Weight::SemiBold,
        Align::Left,
        palette.text,
    );

    let mut close_props = ButtonProps::new("");
    close_props.aria_label = Some(strings::CLOSE_LABEL);
    close_props.icon = Some(glyph::CLOSE);
    close_props.variant = ButtonVariant::Ghost;
    close_props.size = ButtonSize::Icon;
    close_props.interact = Interact::from_state(None, hover, None, false);
    let (_close_node, close_id) = button::build(tree, header, &close_props, palette);

    let body = tree.add_container(
        card,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            gap: style::gap(12.0),
            ..Default::default()
        },
    );

    DialogNodes {
        scrim,
        card,
        body,
        close_id,
    }
}

/// Classifies tree nodes so `FocusModel` traps `Tab`/`Shift+Tab` inside `dialog_card`.
pub fn scope_classifier(tree: &Tree, node: Node, dialog_card: Node) -> FocusScope {
    let dialog_id = tree.id(dialog_card);
    let mut cur = Some(node);
    while let Some(n) = cur {
        if n == dialog_card {
            return FocusScope::Modal(dialog_id);
        }
        cur = tree.parent(n);
    }
    FocusScope::Main
}

/// Syncs `focus` into modal scope anchored at `dialog.card`.
pub fn activate_focus(focus: &mut FocusModel, tree: &Tree, dialog: &DialogNodes) {
    let card_id = tree.id(dialog.card);
    focus.sync_from_tree(tree, |t, n| scope_classifier(t, n, dialog.card));
    focus.push_modal(card_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::a11y;
    use crate::ui::focus::EscapeOutcome;
    use crate::ui::text::TextSystem;

    #[test]
    fn modal_dialog_absorbs_background_hits_and_traps_focus_until_esc() {
        let text = TextSystem::new().expect("dwrite");
        let mut tree = Tree::new(Style {
            size: style::size(800.0, 600.0),
            ..style::column()
        });
        let root = tree.root();
        let mut bg_btn_props = ButtonProps::new("Background action");
        bg_btn_props.variant = ButtonVariant::Secondary;
        let (_bg_node, bg_id) = button::build(&mut tree, root, &bg_btn_props, &Palette::LIGHT);

        let dlg = build(&mut tree, root, "Edit entry", 420.0, None, &Palette::LIGHT);
        let mut confirm_props = ButtonProps::new(strings::CONFIRM_LABEL);
        confirm_props.variant = ButtonVariant::Primary;
        let (_ok_node, ok_id) = button::build(&mut tree, dlg.body, &confirm_props, &Palette::LIGHT);

        tree.layout(800.0, 600.0, &text);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));

        // Scrim blocks hits to controls behind the dialog.
        assert_eq!(tree.hit_test(10.0, 10.0), None);
        let ok_rect = tree.rect(tree.node_by_id(ok_id).unwrap());
        assert_eq!(tree.hit_test(ok_rect.x + 8.0, ok_rect.y + 8.0), Some(ok_id));

        let mut focus = FocusModel::new();
        focus.sync_from_tree(&tree, |t, n| scope_classifier(t, n, dlg.card));
        focus.focus_keyboard(bg_id);
        activate_focus(&mut focus, &tree, &dlg);
        assert_eq!(focus.focused(), Some(dlg.close_id));

        let f1 = focus.tab(false).unwrap();
        let f2 = focus.tab(false).unwrap();
        let f3 = focus.tab(false).unwrap();
        assert_eq!((f1, f2, f3), (ok_id, dlg.close_id, ok_id));

        assert!(matches!(
            focus.on_escape(),
            EscapeOutcome::CloseModal { .. }
        ));
        assert_eq!(focus.focused(), Some(bg_id));
    }
}
