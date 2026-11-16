//! Retained widget tree with flexbox layout (taffy). Sizes are DIPs. The tree
//! is built once and laid out again only when the window size, scale or
//! content changes, never per frame.

use taffy::prelude::*;

use super::text::{TextSystem, Weight};

/// What a leaf draws and how it is measured.
pub enum Leaf {
    Text {
        text: String,
        size: f32,
        weight: Weight,
    },
    /// Fixed-size or flexible box; its size comes from its style alone.
    #[allow(dead_code)] // used by the screens (F3); the layout tests cover it
    Box,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Node(NodeId);

pub struct Tree {
    taffy: TaffyTree<Leaf>,
    root: NodeId,
}

/// Style shorthands so screens read as layout, not as taffy plumbing.
#[allow(dead_code)] // most are first used by the F3 screens; the layout tests cover them
pub mod style {
    use taffy::prelude::*;

    pub fn row() -> Style {
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            align_items: Some(AlignItems::Center),
            ..Default::default()
        }
    }

    pub fn column() -> Style {
        Style {
            display: Display::Flex,
            flex_direction: FlexDirection::Column,
            ..Default::default()
        }
    }

    pub fn size(w: f32, h: f32) -> Size<Dimension> {
        Size {
            width: length(w),
            height: length(h),
        }
    }

    pub fn pad(l: f32, t: f32, r: f32, b: f32) -> taffy::Rect<LengthPercentage> {
        taffy::Rect {
            left: length(l),
            top: length(t),
            right: length(r),
            bottom: length(b),
        }
    }

    pub fn gap(g: f32) -> Size<LengthPercentage> {
        Size {
            width: length(g),
            height: length(g),
        }
    }

    pub fn grow() -> Style {
        Style {
            flex_grow: 1.0,
            ..Default::default()
        }
    }
}

impl Tree {
    pub fn new(root_style: Style) -> Self {
        let mut taffy = TaffyTree::new();
        // Pixel rounding would blur at fractional scales; layout stays in DIPs.
        taffy.disable_rounding();
        let root = taffy.new_leaf(root_style).expect("taffy root");
        Self { taffy, root }
    }

    pub fn root(&self) -> Node {
        Node(self.root)
    }

    #[allow(dead_code)] // used by the screens (F3); the layout tests cover it
    pub fn add_container(&mut self, parent: Node, style: Style) -> Node {
        let n = self.taffy.new_leaf(style).expect("taffy node");
        self.taffy.add_child(parent.0, n).expect("taffy child");
        Node(n)
    }

    pub fn add_leaf(&mut self, parent: Node, style: Style, leaf: Leaf) -> Node {
        let n = self
            .taffy
            .new_leaf_with_context(style, leaf)
            .expect("taffy leaf");
        self.taffy.add_child(parent.0, n).expect("taffy child");
        Node(n)
    }

    /// Lays the tree out for a `w` x `h` DIP viewport.
    pub fn layout(&mut self, w: f32, h: f32, text: &TextSystem) {
        let space = Size {
            width: AvailableSpace::Definite(w),
            height: AvailableSpace::Definite(h),
        };
        let _ = self.taffy.compute_layout_with_measure(
            self.root,
            space,
            |known, _avail, _id, ctx, _style| match ctx {
                Some(Leaf::Text {
                    text: t,
                    size,
                    weight,
                }) => {
                    let (mw, mh) = text.measure(t, *size, *weight).unwrap_or((0.0, 0.0));
                    Size {
                        width: known.width.unwrap_or(mw),
                        height: known.height.unwrap_or(mh),
                    }
                }
                _ => Size::ZERO,
            },
        );
    }

    /// Position and size of `node` in window coordinates.
    pub fn rect(&self, node: Node) -> Rect {
        let l = self.taffy.layout(node.0).expect("layout");
        let (mut x, mut y) = (l.location.x, l.location.y);
        let mut p = self.taffy.parent(node.0);
        while let Some(id) = p {
            let pl = self.taffy.layout(id).expect("layout");
            x += pl.location.x;
            y += pl.location.y;
            p = self.taffy.parent(id);
        }
        Rect {
            x,
            y,
            w: l.size.width,
            h: l.size.height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::style::*;
    use super::*;

    fn text() -> TextSystem {
        TextSystem::new().unwrap()
    }

    fn fixed(w: f32, h: f32) -> Style {
        Style {
            size: size(w, h),
            ..Default::default()
        }
    }

    #[test]
    fn row_places_children_left_to_right_with_gap_and_padding() {
        let mut t = Tree::new(Style {
            size: size(400.0, 100.0),
            padding: pad(10.0, 5.0, 10.0, 5.0),
            gap: gap(8.0),
            ..row()
        });
        let root = t.root();
        let a = t.add_leaf(root, fixed(50.0, 20.0), Leaf::Box);
        let b = t.add_leaf(root, fixed(70.0, 20.0), Leaf::Box);
        t.layout(400.0, 100.0, &text());
        assert_eq!(t.rect(a).x, 10.0);
        assert_eq!(t.rect(b).x, 10.0 + 50.0 + 8.0);
        // Centered on the cross axis inside the padded area.
        assert_eq!(t.rect(a).y, 5.0 + (90.0 - 20.0) / 2.0);
    }

    #[test]
    fn a_growing_child_takes_the_remaining_width() {
        let mut t = Tree::new(Style {
            size: size(500.0, 40.0),
            ..row()
        });
        let root = t.root();
        let left = t.add_leaf(root, fixed(100.0, 10.0), Leaf::Box);
        let mid = t.add_leaf(root, grow(), Leaf::Box);
        let right = t.add_leaf(root, fixed(138.0, 10.0), Leaf::Box);
        t.layout(500.0, 40.0, &text());
        assert_eq!(t.rect(left).w, 100.0);
        assert_eq!(t.rect(mid).w, 500.0 - 100.0 - 138.0);
        assert_eq!(t.rect(right).x, 500.0 - 138.0);
    }

    #[test]
    fn text_leaves_are_measured_and_stack_in_a_column() {
        let mut t = Tree::new(Style {
            size: size(300.0, 300.0),
            align_items: Some(AlignItems::Start),
            ..column()
        });
        let root = t.root();
        let leaf = |s: &str| Leaf::Text {
            text: s.into(),
            size: 14.0,
            weight: Weight::Regular,
        };
        let a = t.add_leaf(root, Style::default(), leaf("Projects"));
        let b = t.add_leaf(root, Style::default(), leaf("Reports for the whole week"));
        t.layout(300.0, 300.0, &text());
        let (ra, rb) = (t.rect(a), t.rect(b));
        assert!(ra.w > 20.0 && rb.w > ra.w);
        assert!(ra.h > 10.0);
        assert!((rb.y - (ra.y + ra.h)).abs() < 0.01);
    }

    #[test]
    fn rects_are_absolute_through_nested_containers() {
        let mut t = Tree::new(Style {
            size: size(600.0, 400.0),
            padding: pad(20.0, 30.0, 0.0, 0.0),
            ..column()
        });
        let root = t.root();
        let panel = t.add_container(
            root,
            Style {
                padding: pad(7.0, 9.0, 0.0, 0.0),
                size: size(200.0, 100.0),
                ..column()
            },
        );
        let inner = t.add_leaf(panel, fixed(10.0, 10.0), Leaf::Box);
        t.layout(600.0, 400.0, &text());
        let r = t.rect(inner);
        assert_eq!((r.x, r.y), (27.0, 39.0));
    }

    #[test]
    fn layout_follows_the_viewport_without_rebuilding() {
        let mut t = Tree::new(Style {
            size: Size {
                width: percent(1.0_f32),
                height: length(40.0_f32),
            },
            ..row()
        });
        let root = t.root();
        let g = t.add_leaf(root, grow(), Leaf::Box);
        t.layout(800.0, 40.0, &text());
        assert_eq!(t.rect(g).w, 800.0);
        t.layout(500.0, 40.0, &text());
        assert_eq!(t.rect(g).w, 500.0);
    }
}
