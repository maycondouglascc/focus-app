//! Native Focus UI widgets built directly on `Tree`, `Palette`, `FocusModel`,
//! `PopupWindow`, and `AccessKit`. Strictly scoped to the controls needed by F3
//! screens (see `win32/docs/decisions/widgets.md`).

#[allow(dead_code)] // widget modules wired progressively across F2/F3
pub mod button;
#[allow(dead_code)]
pub mod charts;
#[allow(dead_code)]
pub mod dialog;
#[allow(dead_code)]
pub mod popover;
#[allow(dead_code)]
pub mod segmented;
#[allow(dead_code)]
pub mod select;
#[allow(dead_code)]
pub mod table;
#[allow(dead_code)]
pub mod text_field;
#[allow(dead_code)]
pub mod toggle;
#[allow(dead_code)]
pub mod virtual_list;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::HiDpi::GetDpiForWindow;

use super::frame;
use super::popup::ScreenRect;
use super::text::{Align, Weight, Wrap};
use super::theme::Color;
use super::tree::{A11yInfo, A11yRole, Leaf, Node, Rect, Tree, WidgetId};

/// Segoe MDL2 Assets / Fluent Icons codepoints used by chrome and controls.
#[allow(dead_code)]
pub mod glyph {
    pub const CHEVRON_DOWN: char = '\u{E70D}';
    pub const CHEVRON_UP: char = '\u{E70E}';
    pub const CHEVRON_LEFT: char = '\u{E76B}';
    pub const CHEVRON_RIGHT: char = '\u{E76C}';
    pub const CHECK: char = '\u{E73E}';
    pub const CLOSE: char = '\u{E8BB}';
    pub const PLAY: char = '\u{E768}';
    pub const STOP: char = '\u{E71A}';
    pub const ADD: char = '\u{E710}';
    pub const CALENDAR: char = '\u{E787}';
    pub const ACTIVITY: char = '\u{E9D2}';
    pub const REPORTS: char = '\u{E9D9}';
    pub const PROJECTS: char = '\u{E8B7}';
    pub const SETTINGS: char = '\u{E713}';
    pub const EDIT: char = '\u{E70F}';
    pub const DELETE: char = '\u{E74D}';
    pub const DOWNLOAD: char = '\u{E896}';
    pub const WARNING: char = '\u{E7BA}';
    pub const CLEAR: char = '\u{E894}';
}

/// Visual interaction flags computed from pointer state for one control instance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Interact {
    pub hovered: bool,
    pub pressed: bool,
    pub disabled: bool,
}

impl Interact {
    pub fn from_state(
        id: Option<WidgetId>,
        hover: Option<WidgetId>,
        pressed: Option<WidgetId>,
        disabled: bool,
    ) -> Self {
        Self {
            hovered: !disabled && id.is_some() && id == hover,
            pressed: !disabled && id.is_some() && id == pressed,
            disabled,
        }
    }
}

/// Shared accessible-info initializer guaranteeing `requires_name = true`.
pub fn interactive_a11y(role: A11yRole, name: impl Into<String>, disabled: bool) -> A11yInfo {
    A11yInfo {
        role,
        name: name.into(),
        disabled,
        clickable: true,
        requires_name: true,
        ..Default::default()
    }
}

/// Convenience helper for a non-wrapping single-line styled label inside a control.
pub fn add_label(
    tree: &mut Tree,
    parent: Node,
    text: impl Into<String>,
    size: f32,
    weight: Weight,
    align: Align,
    color: Color,
) -> Node {
    tree.add_leaf(
        parent,
        taffy::Style::default(),
        Leaf::StyledText {
            text: text.into(),
            size,
            weight,
            align,
            wrap: Wrap::NoWrap,
            color,
        },
    )
}

/// Converts a client DIP rectangle inside `hwnd` into virtual-screen physical pixels.
pub fn client_rect_to_screen(hwnd: HWND, rect_dip: Rect) -> ScreenRect {
    // SAFETY: `GetDpiForWindow` and `ClientToScreen` are safe for any valid HWND.
    let scale = unsafe { frame::scale(GetDpiForWindow(hwnd)) };
    let mut tl = POINT {
        x: (rect_dip.x * scale).round() as i32,
        y: (rect_dip.y * scale).round() as i32,
    };
    let mut br = POINT {
        x: ((rect_dip.x + rect_dip.w) * scale).round() as i32,
        y: ((rect_dip.y + rect_dip.h) * scale).round() as i32,
    };
    // SAFETY: `tl` and `br` are local mutable `POINT` values; failure leaves original coords.
    unsafe {
        let _ = ClientToScreen(hwnd, &mut tl);
        let _ = ClientToScreen(hwnd, &mut br);
    }
    ScreenRect {
        left: tl.x,
        top: tl.y,
        right: br.x,
        bottom: br.y,
    }
}
