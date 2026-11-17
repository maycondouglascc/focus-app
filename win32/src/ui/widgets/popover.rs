//! Anchored transient `WS_POPUP` panel built on `PopupWindow`.

use taffy::prelude::*;
use windows::Win32::Foundation::HWND;

use super::client_rect_to_screen;
use crate::ui::popup::{self, PopupAlign, PopupDelegate};
use crate::ui::theme::Palette;
use crate::ui::tree::{style, A11yInfo, A11yRole, PaintStyle, Rect, Tree};

/// Creates a styled `Tree` root ready to hold popover content in a `WS_POPUP` window.
pub fn build_tree(name: &str, width_dip: f32, palette: &Palette) -> Tree {
    let mut tree = Tree::new(Style {
        display: Display::Flex,
        flex_direction: FlexDirection::Column,
        padding: style::pad(12.0, 12.0, 12.0, 12.0),
        gap: style::gap(8.0),
        size: Size {
            width: length(width_dip.clamp(160.0, 480.0)),
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
            role: A11yRole::Dialog,
            name: name.into(),
            requires_name: true,
            ..Default::default()
        },
    );
    tree
}

/// Opens an anchored `WS_POPUP` popover relative to `anchor_rect_dip` in `owner`.
pub fn open(
    owner: HWND,
    anchor_rect_dip: Rect,
    size_dip: (f32, f32),
    align: PopupAlign,
    tree: Tree,
    delegate: Box<dyn PopupDelegate>,
) -> Option<HWND> {
    let anchor = client_rect_to_screen(owner, anchor_rect_dip);
    popup::open(Some(owner), Some(anchor), size_dip, align, tree, delegate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::a11y;
    use crate::ui::focus::FocusModel;
    use crate::ui::popup::{PopupEvent, PopupOutcome};
    use crate::ui::render::reset_shared;
    use crate::ui::text::TextSystem;
    use crate::ui::widgets::button::{self, ButtonProps};
    use crate::ui::window;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
    use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_KEYDOWN};

    struct TestDelegate;
    impl PopupDelegate for TestDelegate {
        fn on_event(
            &mut self,
            _event: PopupEvent,
            _tree: &mut Tree,
            _focus: &mut FocusModel,
        ) -> PopupOutcome {
            PopupOutcome::default()
        }
    }

    #[test]
    fn anchored_popover_opens_validates_a11y_and_closes_on_esc() {
        crate::ui::init();
        assert!(window::open());
        window::pump();

        let text = TextSystem::new().expect("dwrite");
        let mut tree = build_tree("Activity details", 240.0, &Palette::LIGHT);
        let root = tree.root();
        let props = ButtonProps::new("Copy summary");
        let _ = button::build(&mut tree, root, &props, &Palette::LIGHT);
        tree.layout(240.0, 140.0, &text);
        assert_eq!(a11y::validate_tree_names(&tree), Ok(()));

        let owner = window::hwnd().expect("owner hwnd");
        let pop = open(
            owner,
            Rect {
                x: 40.0,
                y: 40.0,
                w: 120.0,
                h: 32.0,
            },
            (240.0, 140.0),
            PopupAlign::BelowStart,
            tree,
            Box::new(TestDelegate),
        )
        .expect("open popover");
        assert!(popup::is_open());

        // SAFETY: `pop` is our active popup HWND owned by this thread.
        unsafe {
            let _ = SendMessageW(
                pop,
                WM_KEYDOWN,
                Some(WPARAM(VK_ESCAPE.0 as usize)),
                Some(LPARAM(0)),
            );
        }
        window::pump();
        assert!(!popup::is_open());

        window::close();
        window::pump();
        reset_shared();
    }
}
