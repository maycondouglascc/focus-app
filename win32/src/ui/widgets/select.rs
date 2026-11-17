//! Dropdown select (`ComboBox`) backed by a top-level `WS_POPUP` menu.

use std::cell::RefCell;
use std::rc::Rc;

use taffy::prelude::*;
use windows::Win32::Foundation::HWND;

use super::{add_label, client_rect_to_screen, glyph, interactive_a11y, Interact};
use crate::ui::focus::FocusModel;
use crate::ui::input::KeyCode;
use crate::ui::popup::{self, PopupAlign, PopupDelegate, PopupEvent, PopupOutcome};
use crate::ui::strings;
use crate::ui::text::{Align, Weight};
use crate::ui::theme::Palette;
use crate::ui::tree::{style, A11yInfo, A11yRole, HitMode, Leaf, Node, PaintStyle, Tree, WidgetId};

#[derive(Clone, Debug)]
pub struct SelectProps<'a> {
    pub aria_label: &'a str,
    pub options: &'a [&'a str],
    pub selected: Option<usize>,
    pub placeholder: &'a str,
    pub min_width: f32,
    pub open: bool,
    pub disabled: bool,
    pub interact: Interact,
}

/// Builds the closed/open select trigger control in `tree`.
pub fn build(
    tree: &mut Tree,
    parent: Node,
    props: &SelectProps<'_>,
    palette: &Palette,
) -> (Node, WidgetId) {
    let label_text = props
        .selected
        .and_then(|i| props.options.get(i).copied())
        .unwrap_or(if props.placeholder.is_empty() {
            strings::SELECT_PLACEHOLDER
        } else {
            props.placeholder
        });
    let has_value = props.selected.and_then(|i| props.options.get(i)).is_some();

    let node = tree.add_container(
        parent,
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::SpaceBetween),
            gap: style::gap(8.0),
            padding: style::pad(10.0, 0.0, 10.0, 0.0),
            min_size: Size {
                width: length(props.min_width.max(80.0)),
                height: length(32.0_f32),
            },
            size: Size {
                width: auto(),
                height: length(32.0_f32),
            },
            ..Default::default()
        },
    );

    let border_color = if props.disabled {
        palette.ctl_border
    } else if props.open {
        palette.accent
    } else {
        palette.ctl_border
    };
    let bg = if props.disabled {
        palette.disabled_bg
    } else if props.interact.pressed {
        palette.ctl_pressed
    } else if props.interact.hovered {
        palette.ctl_hover
    } else {
        palette.ctl_bg
    };

    tree.set_paint(
        node,
        PaintStyle {
            bg: Some(bg),
            border_color: Some(border_color),
            border_width: palette.border_width,
            radius: 6.0,
            clip: false,
        },
    );
    tree.set_hit(node, HitMode::Interactive);
    tree.set_focusable(node, true);
    tree.set_disabled(node, props.disabled);

    let mut info = interactive_a11y(A11yRole::ComboBox, props.aria_label, props.disabled);
    info.value = Some(label_text.into());
    info.expanded = Some(props.open);
    info.has_popup = true;
    tree.set_a11y(node, info);

    let fg = if props.disabled {
        palette.disabled_text
    } else if has_value {
        palette.text
    } else {
        palette.faint
    };
    add_label(
        tree,
        node,
        label_text,
        12.5,
        Weight::Regular,
        Align::Left,
        fg,
    );
    tree.add_leaf(
        node,
        Style {
            flex_shrink: 0.0,
            ..Default::default()
        },
        Leaf::Icon {
            glyph: if props.open {
                glyph::CHEVRON_UP
            } else {
                glyph::CHEVRON_DOWN
            },
            size: 10.0,
            color: if props.disabled {
                palette.disabled_text
            } else {
                palette.muted
            },
        },
    );

    (node, tree.id(node))
}

/// Builds the popup `ListBox` tree rendered inside `PopupWindow`.
pub fn build_menu_tree(
    options: &[&str],
    selected: Option<usize>,
    width_dip: f32,
    palette: &Palette,
) -> (Tree, Vec<WidgetId>) {
    let mut tree = Tree::new(Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Column,
        padding: style::pad(4.0, 4.0, 4.0, 4.0),
        gap: style::gap(2.0),
        size: Size {
            width: length(width_dip.max(120.0)),
            height: auto(),
        },
        ..Default::default()
    });
    let root = tree.root();
    tree.set_paint(
        root,
        PaintStyle {
            bg: Some(palette.surface),
            border_color: Some(palette.card_border),
            border_width: palette.border_width,
            radius: 8.0,
            clip: true,
        },
    );
    tree.set_a11y(
        root,
        A11yInfo {
            role: A11yRole::ListBox,
            name: strings::SELECT_MENU_LABEL.into(),
            requires_name: true,
            ..Default::default()
        },
    );

    let mut ids = Vec::with_capacity(options.len());
    for (idx, label) in options.iter().enumerate() {
        let is_selected = selected == Some(idx);
        let row = tree.add_container(
            root,
            Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                align_items: Some(AlignItems::Center),
                justify_content: Some(JustifyContent::SpaceBetween),
                padding: style::pad(8.0, 0.0, 8.0, 0.0),
                size: Size {
                    width: auto(),
                    height: length(30.0_f32),
                },
                ..Default::default()
            },
        );
        tree.set_paint(
            row,
            PaintStyle {
                bg: if is_selected {
                    Some(palette.accent_bg)
                } else {
                    None
                },
                border_color: None,
                border_width: 0.0,
                radius: 5.0,
                clip: false,
            },
        );
        tree.set_hit(row, HitMode::Interactive);
        tree.set_focusable(row, true);
        let mut info = interactive_a11y(A11yRole::ListBoxOption, *label, false);
        info.selected = Some(is_selected);
        tree.set_a11y(row, info);

        add_label(
            &mut tree,
            row,
            *label,
            12.5,
            if is_selected {
                Weight::Medium
            } else {
                Weight::Regular
            },
            Align::Left,
            palette.text,
        );
        if is_selected {
            tree.add_leaf(
                row,
                Style::default(),
                Leaf::Icon {
                    glyph: glyph::CHECK,
                    size: 11.0,
                    color: palette.accent,
                },
            );
        }
        ids.push(tree.id(row));
    }

    (tree, ids)
}

struct SelectPopupDelegate {
    item_ids: Vec<WidgetId>,
    cursor: usize,
    on_select: Rc<RefCell<Option<usize>>>,
}

impl PopupDelegate for SelectPopupDelegate {
    fn on_event(
        &mut self,
        event: PopupEvent,
        _tree: &mut Tree,
        focus: &mut FocusModel,
    ) -> PopupOutcome {
        match event {
            PopupEvent::Click(id) => {
                if let Some(idx) = self.item_ids.iter().position(|x| *x == id) {
                    *self.on_select.borrow_mut() = Some(idx);
                    return PopupOutcome {
                        handled: true,
                        dirty: false,
                        close: true,
                        notify_owner: true,
                        ..Default::default()
                    };
                }
                PopupOutcome::default()
            }
            PopupEvent::Key(ev) => match ev.key {
                KeyCode::Down if !self.item_ids.is_empty() => {
                    self.cursor = (self.cursor + 1) % self.item_ids.len();
                    focus.focus_keyboard(self.item_ids[self.cursor]);
                    PopupOutcome {
                        handled: true,
                        dirty: true,
                        close: false,
                        notify_owner: false,
                        ..Default::default()
                    }
                }
                KeyCode::Up if !self.item_ids.is_empty() => {
                    self.cursor = if self.cursor == 0 {
                        self.item_ids.len() - 1
                    } else {
                        self.cursor - 1
                    };
                    focus.focus_keyboard(self.item_ids[self.cursor]);
                    PopupOutcome {
                        handled: true,
                        dirty: true,
                        close: false,
                        notify_owner: false,
                        ..Default::default()
                    }
                }
                KeyCode::Enter | KeyCode::Space if !self.item_ids.is_empty() => {
                    *self.on_select.borrow_mut() = Some(self.cursor);
                    PopupOutcome {
                        handled: true,
                        dirty: false,
                        close: true,
                        notify_owner: true,
                        ..Default::default()
                    }
                }
                _ => PopupOutcome::default(),
            },
            PopupEvent::Closed | PopupEvent::Wheel(_) | PopupEvent::FocusChanged(_) => {
                PopupOutcome::default()
            }
        }
    }
}

/// Opens a popup dropdown anchored below `trigger_rect_dip` in `owner`.
/// Returns a handle storing the chosen option index once clicked or confirmed.
pub fn open_popup(
    owner: HWND,
    trigger_rect_dip: crate::ui::tree::Rect,
    options: &[&str],
    selected: Option<usize>,
    palette: &Palette,
) -> Option<Rc<RefCell<Option<usize>>>> {
    let width = trigger_rect_dip.w.max(140.0);
    let height = 8.0 + (options.len() as f32) * 32.0;
    let (tree, item_ids) = build_menu_tree(options, selected, width, palette);
    let chosen = Rc::new(RefCell::new(None));
    let delegate = SelectPopupDelegate {
        item_ids,
        cursor: selected.unwrap_or(0).min(options.len().saturating_sub(1)),
        on_select: Rc::clone(&chosen),
    };
    let anchor = client_rect_to_screen(owner, trigger_rect_dip);
    popup::open(
        Some(owner),
        Some(anchor),
        (width, height.min(440.0)),
        PopupAlign::BelowStart,
        tree,
        Box::new(delegate),
    )?;
    Some(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::a11y;
    use crate::ui::popup;
    use crate::ui::render::reset_shared;
    use crate::ui::text::TextSystem;
    use crate::ui::window;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_RETURN};
    use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_KEYDOWN};

    #[test]
    fn select_trigger_and_menu_tree_publish_combobox_and_listbox_a11y() {
        let text = TextSystem::new().expect("dwrite");
        let opts = ["Work", "Meetings", "Personal"];
        let mut tree = Tree::new(style::row());
        let root = tree.root();
        let (trigger, tid) = build(
            &mut tree,
            root,
            &SelectProps {
                aria_label: "Project",
                options: &opts,
                selected: Some(1),
                placeholder: strings::SELECT_PLACEHOLDER,
                min_width: 160.0,
                open: false,
                disabled: false,
                interact: Interact::default(),
            },
            &Palette::LIGHT,
        );
        tree.layout(300.0, 80.0, &text);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));
        let info = &tree.data(trigger).unwrap().a11y;
        assert_eq!(info.role, A11yRole::ComboBox);
        assert_eq!(info.value.as_deref(), Some("Meetings"));
        assert!(info.has_popup);

        let r = tree.rect(trigger);
        assert_eq!(tree.hit_test(r.x + 8.0, r.y + 8.0), Some(tid));

        let (mut menu, ids) = build_menu_tree(&opts, Some(1), 160.0, &Palette::LIGHT);
        menu.layout(160.0, 200.0, &text);
        assert_eq!(a11y::validate_tree_names(&menu), Ok(()));
        assert_eq!(ids.len(), 3);
        let opt1 = menu.node_by_id(ids[1]).unwrap();
        assert_eq!(menu.data(opt1).unwrap().a11y.selected, Some(true));
    }

    #[test]
    fn select_popup_selects_option_via_keyboard_and_closes() {
        crate::ui::init();
        assert!(window::open());
        window::pump();

        let opts = ["Work", "Meetings", "Personal"];
        let (menu, ids) = build_menu_tree(&opts, Some(0), 160.0, &Palette::LIGHT);
        let chosen = Rc::new(RefCell::new(None));
        let delegate = SelectPopupDelegate {
            item_ids: ids,
            cursor: 0,
            on_select: Rc::clone(&chosen),
        };
        let popup_hwnd = popup::open(
            None,
            None,
            (160.0, 110.0),
            PopupAlign::Center,
            menu,
            Box::new(delegate),
        )
        .expect("select popup");
        assert!(popup::is_open());

        // SAFETY: `popup_hwnd` is owned by this thread.
        unsafe {
            let _ = SendMessageW(
                popup_hwnd,
                WM_KEYDOWN,
                Some(WPARAM(VK_DOWN.0 as usize)),
                Some(LPARAM(0)),
            );
            let _ = SendMessageW(
                popup_hwnd,
                WM_KEYDOWN,
                Some(WPARAM(VK_RETURN.0 as usize)),
                Some(LPARAM(0)),
            );
        }
        window::pump();
        assert!(!popup::is_open());
        assert_eq!(*chosen.borrow(), Some(1));

        window::close();
        window::pump();
        reset_shared();
    }
}
