//! Retained widget tree with flexbox layout (taffy), paint metadata,
//! clipping/scrolling, hit-testing, and accessibility attributes. Sizes are
//! DIPs. The tree is built or updated when state changes and laid out again
//! only when the viewport, text scale, or content changes, never per frame.

use std::collections::HashMap;

use taffy::prelude::*;

use super::text::{Align, TextFont, TextSystem, Weight, Wrap};
use super::theme::Color;

/// Stable node identifier within a window's tree.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct WidgetId(pub u64);

/// Drawing primitives emitted in a leaf's local `(0, 0, w, h)` DIP coordinate space.
#[allow(dead_code)] // variants used progressively by widgets and chart primitives
#[derive(Clone, Debug)]
pub enum DrawCmd {
    FillRect {
        rect: Rect,
        color: Color,
        radius: f32,
    },
    StrokeRect {
        rect: Rect,
        color: Color,
        width: f32,
        radius: f32,
    },
    FillCircle {
        cx: f32,
        cy: f32,
        r: f32,
        color: Color,
    },
    StrokeArc {
        cx: f32,
        cy: f32,
        r: f32,
        start_deg: f32,
        sweep_deg: f32,
        width: f32,
        color: Color,
        round_cap: bool,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        width: f32,
        color: Color,
    },
    Text {
        rect: Rect,
        text: String,
        size: f32,
        weight: Weight,
        align: Align,
        wrap: Wrap,
        color: Color,
    },
    Icon {
        rect: Rect,
        glyph: char,
        size: f32,
        color: Color,
    },
}

/// What a leaf draws and how it is measured.
#[allow(dead_code)] // variants consumed by widgets and screens
pub enum Leaf {
    Text {
        text: String,
        size: f32,
        weight: Weight,
    },
    StyledText {
        text: String,
        size: f32,
        weight: Weight,
        align: Align,
        wrap: Wrap,
        color: Color,
    },
    Icon {
        glyph: char,
        size: f32,
        color: Color,
    },
    Canvas(Vec<DrawCmd>),
    /// Placeholder rectangle for a child `RICHEDIT50W` window positioned by the tree.
    RichTextSlot {
        key: String,
        placeholder: String,
        value: String,
        multiline: bool,
        sync_rev: u64,
    },
    /// Fixed-size or flexible box; its size comes from its style alone.
    Box,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RichTextSlotSpec {
    pub id: WidgetId,
    pub key: String,
    pub placeholder: String,
    pub value: String,
    pub multiline: bool,
    pub sync_rev: u64,
    pub disabled: bool,
    pub rect: Rect,
    pub hidden: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PaintStyle {
    pub bg: Option<Color>,
    pub border_color: Option<Color>,
    pub border_width: f32,
    pub radius: f32,
    pub clip: bool,
}

#[allow(dead_code)] // variants used by input and widgets
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HitMode {
    #[default]
    PassThrough,
    Interactive,
    /// Absorbs pointer hits without triggering an action (cards, modal scrims, disabled controls).
    Opaque,
}

/// Semantic role mapped to AccessKit in `a11y.rs`.
#[allow(dead_code)] // roles used across the full F2 widget set
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum A11yRole {
    #[default]
    None,
    Window,
    Group,
    Button,
    DefaultButton,
    Switch,
    CheckBox,
    RadioButton,
    ComboBox,
    ListBox,
    ListBoxOption,
    TextInput,
    Dialog,
    Table,
    ColumnHeader,
    Row,
    Cell,
    List,
    ListItem,
    Label,
    Figure,
    ScrollView,
    Link,
}

/// Accessibility properties attached directly to a node.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct A11yInfo {
    pub role: A11yRole,
    pub name: String,
    pub description: Option<String>,
    pub value: Option<String>,
    pub toggled: Option<bool>,
    pub selected: Option<bool>,
    pub expanded: Option<bool>,
    pub disabled: bool,
    pub modal: bool,
    pub has_popup: bool,
    pub clickable: bool,
    /// True for any widget node that must carry a non-empty accessible `name`.
    pub requires_name: bool,
}

pub struct NodeData {
    pub id: WidgetId,
    pub leaf: Option<Leaf>,
    pub paint: PaintStyle,
    pub hit: HitMode,
    pub focusable: bool,
    pub disabled: bool,
    pub hidden: bool,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub a11y: A11yInfo,
}

impl NodeData {
    fn new(id: WidgetId, leaf: Option<Leaf>) -> Self {
        Self {
            id,
            leaf,
            paint: PaintStyle::default(),
            hit: HitMode::PassThrough,
            focusable: false,
            disabled: false,
            hidden: false,
            scroll_x: 0.0,
            scroll_y: 0.0,
            a11y: A11yInfo::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    pub fn intersect(&self, other: Rect) -> Option<Rect> {
        let l = self.x.max(other.x);
        let t = self.y.max(other.y);
        let r = (self.x + self.w).min(other.x + other.w);
        let b = (self.y + self.h).min(other.y + other.h);
        if r > l && b > t {
            Some(Rect {
                x: l,
                y: t,
                w: r - l,
                h: b - t,
            })
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Node(NodeId);

pub struct Tree {
    taffy: TaffyTree<NodeData>,
    root: NodeId,
    next_id: u64,
    by_id: HashMap<WidgetId, NodeId>,
}

/// Style shorthands so screens read as layout, not as taffy plumbing.
#[allow(dead_code)] // most are first used by widgets and F3 screens; layout tests cover them
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

#[allow(dead_code)] // methods used across input, focus, a11y, widgets, and render
impl Tree {
    pub fn new(root_style: Style) -> Self {
        let mut taffy = TaffyTree::new();
        // Pixel rounding would blur at fractional scales; layout stays in DIPs.
        taffy.disable_rounding();
        let root_id = WidgetId(1);
        let root = taffy
            .new_leaf_with_context(root_style, NodeData::new(root_id, None))
            .expect("taffy root");
        let mut by_id = HashMap::new();
        by_id.insert(root_id, root);
        Self {
            taffy,
            root,
            next_id: 2,
            by_id,
        }
    }

    fn alloc_id(&mut self) -> WidgetId {
        let id = WidgetId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn root(&self) -> Node {
        Node(self.root)
    }

    pub fn id(&self, node: Node) -> WidgetId {
        self.taffy
            .get_node_context(node.0)
            .expect("node context")
            .id
    }

    pub fn node_by_id(&self, id: WidgetId) -> Option<Node> {
        self.by_id.get(&id).copied().map(Node)
    }

    pub fn add_container(&mut self, parent: Node, style: Style) -> Node {
        let id = self.alloc_id();
        let n = self
            .taffy
            .new_leaf_with_context(style, NodeData::new(id, None))
            .expect("taffy node");
        self.taffy.add_child(parent.0, n).expect("taffy child");
        self.by_id.insert(id, n);
        Node(n)
    }

    pub fn add_leaf(&mut self, parent: Node, style: Style, leaf: Leaf) -> Node {
        let id = self.alloc_id();
        let n = self
            .taffy
            .new_leaf_with_context(style, NodeData::new(id, Some(leaf)))
            .expect("taffy leaf");
        self.taffy.add_child(parent.0, n).expect("taffy child");
        self.by_id.insert(id, n);
        Node(n)
    }

    pub fn data(&self, node: Node) -> Option<&NodeData> {
        self.taffy.get_node_context(node.0)
    }

    pub fn data_mut(&mut self, node: Node) -> Option<&mut NodeData> {
        self.taffy.get_node_context_mut(node.0)
    }

    pub fn set_paint(&mut self, node: Node, paint: PaintStyle) {
        if let Some(d) = self.data_mut(node) {
            d.paint = paint;
        }
    }

    pub fn set_hit(&mut self, node: Node, hit: HitMode) {
        if let Some(d) = self.data_mut(node) {
            d.hit = hit;
        }
    }

    pub fn set_focusable(&mut self, node: Node, focusable: bool) {
        if let Some(d) = self.data_mut(node) {
            d.focusable = focusable;
        }
    }

    pub fn set_disabled(&mut self, node: Node, disabled: bool) {
        if let Some(d) = self.data_mut(node) {
            d.disabled = disabled;
            d.a11y.disabled = disabled;
        }
    }

    pub fn set_hidden(&mut self, node: Node, hidden: bool) {
        if let Some(d) = self.data_mut(node) {
            d.hidden = hidden;
        }
    }

    pub fn set_scroll(&mut self, node: Node, x: f32, y: f32) {
        if let Some(d) = self.data_mut(node) {
            d.scroll_x = x.max(0.0);
            d.scroll_y = y.max(0.0);
        }
    }

    pub fn replace_canvas(&mut self, node: Node, cmds: Vec<DrawCmd>) {
        if let Some(d) = self.data_mut(node) {
            d.leaf = Some(Leaf::Canvas(cmds));
        }
    }

    pub fn set_a11y(&mut self, node: Node, a11y: A11yInfo) {
        if let Some(d) = self.data_mut(node) {
            d.a11y = a11y;
        }
    }

    pub fn children(&self, node: Node) -> Vec<Node> {
        self.taffy
            .children(node.0)
            .map(|c| c.into_iter().map(Node).collect())
            .unwrap_or_default()
    }

    pub fn parent(&self, node: Node) -> Option<Node> {
        self.taffy.parent(node.0).map(Node)
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
            |known, avail, _id, ctx, _style| {
                let Some(data) = ctx else {
                    return Size::ZERO;
                };
                if data.hidden {
                    return Size::ZERO;
                }
                let Some(leaf) = data.leaf.as_ref() else {
                    return Size::ZERO;
                };
                match leaf {
                    Leaf::Text {
                        text: t,
                        size,
                        weight,
                    } => {
                        let (mw, mh) = text.measure(t, *size, *weight).unwrap_or((0.0, 0.0));
                        Size {
                            width: known.width.unwrap_or(mw),
                            height: known.height.unwrap_or(mh),
                        }
                    }
                    Leaf::StyledText {
                        text: t,
                        size,
                        weight,
                        align,
                        wrap,
                        ..
                    } => {
                        let max_w = match (*wrap, known.width, avail.width) {
                            (Wrap::Word, Some(w), _) => w,
                            (Wrap::Word, None, AvailableSpace::Definite(w)) => w,
                            _ => 65_536.0,
                        };
                        // Intrinsic width of a non-wrapped line is alignment-independent;
                        // measuring with `Align::Left` avoids DirectWrite center/right offset blowup.
                        let measure_align = if *wrap == Wrap::NoWrap {
                            Align::Left
                        } else {
                            *align
                        };
                        let (mw, mh) = text
                            .measure_styled(
                                TextFont::Ui,
                                t,
                                *size,
                                *weight,
                                measure_align,
                                *wrap,
                                max_w,
                            )
                            .unwrap_or((0.0, 0.0));
                        Size {
                            width: known.width.unwrap_or(mw),
                            height: known.height.unwrap_or(mh),
                        }
                    }
                    Leaf::Icon { size, .. } => {
                        let scaled = size * text.text_scale();
                        Size {
                            width: known.width.unwrap_or(scaled),
                            height: known.height.unwrap_or(scaled),
                        }
                    }
                    Leaf::Canvas(_) | Leaf::RichTextSlot { .. } | Leaf::Box => Size {
                        width: known.width.unwrap_or(0.0),
                        height: known.height.unwrap_or(0.0),
                    },
                }
            },
        );
    }

    /// Position and size of `node` in window DIP coordinates, including ancestor scroll offsets.
    pub fn rect(&self, node: Node) -> Rect {
        let l = self.taffy.layout(node.0).expect("layout");
        let (mut x, mut y) = (l.location.x, l.location.y);
        let mut p = self.taffy.parent(node.0);
        while let Some(id) = p {
            let pl = self.taffy.layout(id).expect("layout");
            let (sx, sy) = self
                .taffy
                .get_node_context(id)
                .map(|d| (d.scroll_x, d.scroll_y))
                .unwrap_or((0.0, 0.0));
            x += pl.location.x - sx;
            y += pl.location.y - sy;
            p = self.taffy.parent(id);
        }
        Rect {
            x,
            y,
            w: l.size.width,
            h: l.size.height,
        }
    }

    /// Hit-tests `x, y` (window DIP coordinates) respecting reverse paint order,
    /// clipping containers, scroll offsets, and `HitMode`.
    pub fn hit_test(&self, x: f32, y: f32) -> Option<WidgetId> {
        match self.hit_walk(self.root, x, y, 0.0, 0.0, None) {
            HitOutcome::Hit(id) => Some(id),
            HitOutcome::Blocked | HitOutcome::Miss => None,
        }
    }

    fn hit_walk(
        &self,
        id: NodeId,
        px: f32,
        py: f32,
        ox: f32,
        oy: f32,
        clip: Option<Rect>,
    ) -> HitOutcome {
        let Some(data) = self.taffy.get_node_context(id) else {
            return HitOutcome::Miss;
        };
        if data.hidden {
            return HitOutcome::Miss;
        }
        let Ok(l) = self.taffy.layout(id) else {
            return HitOutcome::Miss;
        };
        let rect = Rect {
            x: ox + l.location.x,
            y: oy + l.location.y,
            w: l.size.width,
            h: l.size.height,
        };
        let effective_clip = match (clip, data.paint.clip) {
            (Some(c), true) => match c.intersect(rect) {
                Some(inter) => Some(inter),
                None => return HitOutcome::Miss,
            },
            (Some(c), false) => Some(c),
            (None, true) => Some(rect),
            (None, false) => None,
        };
        if let Some(c) = effective_clip {
            if !c.contains(px, py) {
                return HitOutcome::Miss;
            }
        }
        if let Ok(children) = self.taffy.children(id) {
            let child_ox = rect.x - data.scroll_x;
            let child_oy = rect.y - data.scroll_y;
            for child in children.into_iter().rev() {
                match self.hit_walk(child, px, py, child_ox, child_oy, effective_clip) {
                    HitOutcome::Miss => {}
                    other => return other,
                }
            }
        }
        if !rect.contains(px, py) {
            return HitOutcome::Miss;
        }
        match data.hit {
            HitMode::PassThrough => HitOutcome::Miss,
            HitMode::Interactive => {
                if data.disabled {
                    HitOutcome::Blocked
                } else {
                    HitOutcome::Hit(data.id)
                }
            }
            HitMode::Opaque => HitOutcome::Blocked,
        }
    }

    /// Walks all visible nodes in paint order for Direct2D rendering.
    pub fn visit_paint<F: FnMut(&NodeData, Rect, PaintStep)>(&self, f: &mut F) {
        self.paint_walk(self.root, 0.0, 0.0, f);
    }

    fn paint_walk<F: FnMut(&NodeData, Rect, PaintStep)>(
        &self,
        id: NodeId,
        ox: f32,
        oy: f32,
        f: &mut F,
    ) {
        let Some(data) = self.taffy.get_node_context(id) else {
            return;
        };
        if data.hidden {
            return;
        }
        let Ok(l) = self.taffy.layout(id) else {
            return;
        };
        let rect = Rect {
            x: ox + l.location.x,
            y: oy + l.location.y,
            w: l.size.width,
            h: l.size.height,
        };
        f(data, rect, PaintStep::Enter);
        if let Ok(children) = self.taffy.children(id) {
            let child_ox = rect.x - data.scroll_x;
            let child_oy = rect.y - data.scroll_y;
            for child in children {
                self.paint_walk(child, child_ox, child_oy, f);
            }
        }
        f(data, rect, PaintStep::Leave);
    }

    /// Collects all nodes in pre-order (used by focus ordering and accessibility).
    pub fn all_nodes(&self) -> Vec<Node> {
        let mut out = Vec::new();
        self.collect_nodes(self.root, &mut out);
        out
    }

    fn collect_nodes(&self, id: NodeId, out: &mut Vec<Node>) {
        out.push(Node(id));
        if let Ok(children) = self.taffy.children(id) {
            for c in children {
                self.collect_nodes(c, out);
            }
        }
    }

    /// Returns specs and rects for every `RichTextSlot` leaf.
    pub fn rich_text_slots(&self) -> Vec<RichTextSlotSpec> {
        let mut out = Vec::new();
        self.collect_slots(self.root, 0.0, 0.0, false, None, &mut out);
        out
    }

    fn collect_slots(
        &self,
        id: NodeId,
        ox: f32,
        oy: f32,
        ancestor_hidden: bool,
        clip: Option<Rect>,
        out: &mut Vec<RichTextSlotSpec>,
    ) {
        let Some(data) = self.taffy.get_node_context(id) else {
            return;
        };
        let hidden = ancestor_hidden || data.hidden;
        let Ok(l) = self.taffy.layout(id) else {
            return;
        };
        let rect = Rect {
            x: ox + l.location.x,
            y: oy + l.location.y,
            w: l.size.width,
            h: l.size.height,
        };
        if let Some(Leaf::RichTextSlot {
            key,
            placeholder,
            value,
            multiline,
            sync_rev,
        }) = &data.leaf
        {
            let clipped_out = clip.is_some_and(|c| rect.intersect(c).is_none());
            out.push(RichTextSlotSpec {
                id: data.id,
                key: key.clone(),
                placeholder: placeholder.clone(),
                value: value.clone(),
                multiline: *multiline,
                sync_rev: *sync_rev,
                disabled: data.disabled,
                rect,
                hidden: hidden || clipped_out,
            });
        }
        let next_clip = match (clip, data.paint.clip) {
            (Some(c), true) => c.intersect(rect),
            (Some(c), false) => Some(c),
            (None, true) => Some(rect),
            (None, false) => None,
        };
        if let Ok(children) = self.taffy.children(id) {
            let child_ox = rect.x - data.scroll_x;
            let child_oy = rect.y - data.scroll_y;
            for c in children {
                self.collect_slots(c, child_ox, child_oy, hidden, next_clip, out);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PaintStep {
    Enter,
    Leave,
}

enum HitOutcome {
    Miss,
    Hit(WidgetId),
    Blocked,
}

#[cfg(test)]
mod tests {
    use super::super::frame::px;
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

    #[test]
    fn hit_test_respects_z_order_clipping_scroll_and_blocking() {
        let mut t = Tree::new(Style {
            size: size(400.0, 200.0),
            ..column()
        });
        let root = t.root();
        let scroll = t.add_container(
            root,
            Style {
                size: size(200.0, 50.0),
                ..column()
            },
        );
        t.set_paint(
            scroll,
            PaintStyle {
                clip: true,
                ..Default::default()
            },
        );
        let first = t.add_leaf(
            scroll,
            Style {
                flex_shrink: 0.0,
                ..fixed(180.0, 40.0)
            },
            Leaf::Box,
        );
        t.set_hit(first, HitMode::Interactive);
        let second = t.add_leaf(
            scroll,
            Style {
                flex_shrink: 0.0,
                ..fixed(180.0, 40.0)
            },
            Leaf::Box,
        );
        t.set_hit(second, HitMode::Interactive);

        t.layout(400.0, 200.0, &text());
        // Before scroll: `first` is at y=[0,40), `second` partially visible at y=[40,50).
        assert_eq!(t.hit_test(10.0, 20.0), Some(t.id(first)));
        assert_eq!(t.hit_test(10.0, 45.0), Some(t.id(second)));
        // Below clip viewport (y=60) is clipped out even though `second` spans 40..80.
        assert_eq!(t.hit_test(10.0, 60.0), None);

        // Scroll down 40 DIPs: `second` moves into y=[0,40).
        t.set_scroll(scroll, 0.0, 40.0);
        assert_eq!(t.hit_test(10.0, 20.0), Some(t.id(second)));

        // Add an opaque modal scrim on top of everything.
        let scrim = t.add_leaf(
            root,
            Style {
                position: Position::Absolute,
                size: size(400.0, 200.0),
                ..Default::default()
            },
            Leaf::Box,
        );
        t.set_hit(scrim, HitMode::Opaque);
        t.layout(400.0, 200.0, &text());
        assert_eq!(t.hit_test(10.0, 20.0), None);
    }

    #[test]
    fn dpi_scales_dip_layout_cleanly_at_100_150_200_percent() {
        let mut t = Tree::new(Style {
            size: Size {
                width: percent(1.0_f32),
                height: percent(1.0_f32),
            },
            padding: pad(24.0, 16.0, 24.0, 16.0),
            gap: gap(12.0),
            ..row()
        });
        let root = t.root();
        let btn = t.add_leaf(root, fixed(120.0, 36.0), Leaf::Box);
        let fill = t.add_leaf(
            root,
            Style {
                size: Size {
                    width: auto(),
                    height: length(36.0_f32),
                },
                ..grow()
            },
            Leaf::Box,
        );

        let tx = text();
        t.layout(1100.0, 760.0, &tx);
        let rb = t.rect(btn);
        let rf = t.rect(fill);
        assert_eq!((rb.x, rb.w, rb.h), (24.0, 120.0, 36.0));
        assert_eq!(rf.x, 24.0 + 120.0 + 12.0);
        assert_eq!(rf.w, 1100.0 - 48.0 - 120.0 - 12.0);

        // Verify physical pixel conversion across 100% (96), 150% (144), and 200% (192).
        for (dpi, mult) in [(96_u32, 1.0_f32), (144, 1.5), (192, 2.0)] {
            assert_eq!(px(rb.x, dpi), (24.0 * mult).round() as i32);
            assert_eq!(px(rb.w, dpi), (120.0 * mult).round() as i32);
            assert_eq!(px(rb.h, dpi), (36.0 * mult).round() as i32);
            assert_eq!(px(rf.w, dpi), (rf.w * mult).round() as i32);
        }
    }
}
