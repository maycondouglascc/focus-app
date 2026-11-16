//! Layout of the window chrome (the title bar): the title on the left, space
//! reserved on the right for the caption buttons that `render.rs` draws and
//! `frame.rs` hit-tests.

use taffy::prelude::*;

use super::frame::{BUTTON_DIP, TITLE_BAR_DIP};
use super::strings;
use super::text::{TextSystem, Weight};
use super::tree::{style, Leaf, Node, Rect, Tree};

pub const TITLE_SIZE: f32 = 12.0;
const TITLE_INSET: f32 = 12.0;

pub struct Chrome {
    tree: Tree,
    title: Node,
    laid_out_for: Option<f32>,
}

impl Chrome {
    pub fn new() -> Self {
        let mut tree = Tree::new(Style {
            size: Size {
                width: percent(1.0_f32),
                height: length(TITLE_BAR_DIP),
            },
            padding: style::pad(TITLE_INSET, 0.0, BUTTON_DIP * 3.0, 0.0),
            ..style::row()
        });
        let root = tree.root();
        let title = tree.add_leaf(
            root,
            Style {
                // May shrink below its text so a narrow window clips the title
                // instead of running under the caption buttons.
                min_size: Size {
                    width: length(0.0_f32),
                    height: auto(),
                },
                ..Default::default()
            },
            Leaf::Text {
                text: strings::APP_NAME.into(),
                size: TITLE_SIZE,
                weight: Weight::SemiBold,
            },
        );
        Self {
            tree,
            title,
            laid_out_for: None,
        }
    }

    /// Lays out for a window `w` DIPs wide; a no-op if nothing changed.
    pub fn layout(&mut self, w: f32, text: &TextSystem) {
        if self.laid_out_for == Some(w) {
            return;
        }
        self.tree.layout(w, TITLE_BAR_DIP, text);
        self.laid_out_for = Some(w);
    }

    pub fn title_rect(&self) -> Rect {
        self.tree.rect(self.title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_sits_left_and_is_centered_in_the_bar() {
        let text = TextSystem::new().unwrap();
        let mut c = Chrome::new();
        c.layout(1000.0, &text);
        let r = c.title_rect();
        assert_eq!(r.x, TITLE_INSET);
        assert!((r.y + r.h / 2.0 - TITLE_BAR_DIP / 2.0).abs() < 0.5);
    }

    #[test]
    fn narrow_windows_clip_the_title_before_the_buttons() {
        let text = TextSystem::new().unwrap();
        let mut c = Chrome::new();
        c.layout(BUTTON_DIP * 3.0 + TITLE_INSET + 10.0, &text);
        let r = c.title_rect();
        assert!(r.x + r.w <= BUTTON_DIP * 3.0 + TITLE_INSET + 10.0 - BUTTON_DIP * 3.0 + 0.01);
    }
}
