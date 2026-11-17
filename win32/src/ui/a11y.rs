//! Windows UI Automation accessibility via `accesskit` + `accesskit_windows`.
//!
//! Every widget in `Tree` publishes its semantic role, accessible `name`,
//! state (`disabled`, `toggled`, `selected`, `expanded`, `modal`), bounds in
//! client physical pixels, and supported actions (`Click`, `Focus`, `Expand`,
//! `Collapse`, `SetValue`). Background UIA action requests are posted back to
//! the owning UI thread via `WM_A11Y_ACTION`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use accesskit::{
    Action, ActionData, ActionHandler, ActionRequest, ActivationHandler, HasPopup, Node as AkNode,
    NodeId as AkNodeId, Rect as AkRect, Role, Toggled, TreeId, TreeInfo, TreeUpdate,
};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use super::tree::{A11yRole, HitMode, Leaf, Node, NodeData, Tree, WidgetId};

/// Window message posted by [`WinActionHandler`] to replay queued UIA actions on the UI thread.
pub const WM_A11Y_ACTION: u32 = WM_APP + 1;

/// Thread-safe action queue bridging UIA background threads to the window's UI thread.
#[derive(Clone, Default)]
pub struct A11yActionQueue {
    hwnd: isize,
    queue: Arc<Mutex<VecDeque<ActionRequest>>>,
}

impl A11yActionQueue {
    pub fn new(hwnd: HWND) -> Self {
        Self {
            hwnd: hwnd.0 as isize,
            queue: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub fn handler(&self) -> WinActionHandler {
        WinActionHandler {
            hwnd: self.hwnd,
            queue: Arc::clone(&self.queue),
        }
    }

    pub fn drain(&self) -> Vec<ActionRequest> {
        let mut g = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        g.drain(..).collect()
    }
}

pub struct WinActionHandler {
    hwnd: isize,
    queue: Arc<Mutex<VecDeque<ActionRequest>>>,
}

impl ActionHandler for WinActionHandler {
    fn do_action(&mut self, request: ActionRequest) {
        if let Ok(mut g) = self.queue.lock() {
            g.push_back(request);
        }
        let hwnd = HWND(self.hwnd as *mut core::ffi::c_void);
        if !hwnd.0.is_null() {
            // SAFETY: `PostMessageW` is thread-safe for queuing a wakeup message to `hwnd`.
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_A11Y_ACTION, WPARAM(0), LPARAM(0));
            }
        }
    }
}

pub struct StaticActivation(pub Option<TreeUpdate>);

impl ActivationHandler for StaticActivation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.0.take()
    }
}

pub fn ak_id(id: WidgetId) -> AkNodeId {
    AkNodeId(id.0)
}

#[allow(dead_code)] // used by F3 and widget action handlers
pub fn widget_id(id: AkNodeId) -> WidgetId {
    WidgetId(id.0)
}

/// Maps internal [`A11yRole`] to AccessKit's [`Role`].
pub fn map_role(role: A11yRole, is_root: bool) -> Role {
    match role {
        A11yRole::None => {
            if is_root {
                Role::Window
            } else {
                Role::GenericContainer
            }
        }
        A11yRole::Window => Role::Window,
        A11yRole::Group => Role::Group,
        A11yRole::Button => Role::Button,
        A11yRole::DefaultButton => Role::DefaultButton,
        A11yRole::Switch => Role::Switch,
        A11yRole::CheckBox => Role::CheckBox,
        A11yRole::RadioButton => Role::RadioButton,
        A11yRole::ComboBox => Role::ComboBox,
        A11yRole::ListBox => Role::ListBox,
        A11yRole::ListBoxOption => Role::ListBoxOption,
        A11yRole::TextInput => Role::TextInput,
        A11yRole::Dialog => Role::Dialog,
        A11yRole::Table => Role::Table,
        A11yRole::ColumnHeader => Role::ColumnHeader,
        A11yRole::Row => Role::Row,
        A11yRole::Cell => Role::Cell,
        A11yRole::List => Role::List,
        A11yRole::ListItem => Role::ListItem,
        A11yRole::Label => Role::Label,
        A11yRole::Figure => Role::Figure,
        A11yRole::ScrollView => Role::ScrollView,
        A11yRole::Link => Role::Link,
    }
}

/// Returns true if a node represents a widget/control that must expose an accessible name.
#[allow(dead_code)] // verified by automated tests and debug assertions
pub fn node_requires_name(data: &NodeData) -> bool {
    if data.a11y.requires_name || data.focusable || data.hit == HitMode::Interactive {
        return true;
    }
    matches!(
        data.a11y.role,
        A11yRole::Button
            | A11yRole::DefaultButton
            | A11yRole::Switch
            | A11yRole::CheckBox
            | A11yRole::RadioButton
            | A11yRole::ComboBox
            | A11yRole::ListBox
            | A11yRole::ListBoxOption
            | A11yRole::TextInput
            | A11yRole::Dialog
            | A11yRole::Table
            | A11yRole::ColumnHeader
            | A11yRole::Row
            | A11yRole::Cell
            | A11yRole::List
            | A11yRole::ListItem
            | A11yRole::Figure
            | A11yRole::Link
    )
}

/// Validates that every widget node in `tree` carries a non-empty accessible `name`.
#[allow(dead_code)] // verified by automated tests and debug assertions
pub fn validate_tree_names(tree: &Tree) -> Result<(), Vec<(WidgetId, A11yRole)>> {
    let mut missing = Vec::new();
    for node in tree.all_nodes() {
        let Some(data) = tree.data(node) else {
            continue;
        };
        if node_requires_name(data) && data.a11y.name.trim().is_empty() {
            missing.push((data.id, data.a11y.role));
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

/// Builds a complete [`TreeUpdate`] for `tree` (or a minimal window root if `tree` is `None`).
///
/// `scale` is `frame::scale(dpi)` so bounds match UIA client physical pixels.
pub fn build_tree_update(
    tree: Option<&Tree>,
    focused: Option<WidgetId>,
    scale: f32,
    client_px: (i32, i32),
    root_name: &str,
) -> TreeUpdate {
    let Some(tree) = tree else {
        let root_id = WidgetId(1);
        let mut root = AkNode::new(Role::Window);
        root.set_label(root_name);
        root.set_bounds(AkRect {
            x0: 0.0,
            y0: 0.0,
            x1: client_px.0.max(0) as f64,
            y1: client_px.1.max(0) as f64,
        });
        return TreeUpdate {
            nodes: vec![(ak_id(root_id), root)],
            tree: Some(TreeInfo::new(ak_id(root_id))),
            tree_id: TreeId::ROOT,
            focus: ak_id(root_id),
        };
    };

    let root = tree.root();
    let root_widget_id = tree.id(root);
    let mut nodes = Vec::new();
    push_node(tree, root, true, scale, root_name, &mut nodes);

    let focus_id = focused
        .filter(|id| {
            tree.node_by_id(*id)
                .is_some_and(|n| !is_subtree_hidden(tree, n))
        })
        .unwrap_or(root_widget_id);

    TreeUpdate {
        nodes,
        tree: Some(TreeInfo::new(ak_id(root_widget_id))),
        tree_id: TreeId::ROOT,
        focus: ak_id(focus_id),
    }
}

fn is_subtree_hidden(tree: &Tree, node: Node) -> bool {
    if tree.data(node).is_some_and(|d| d.hidden) {
        return true;
    }
    let mut cur = tree.parent(node);
    while let Some(p) = cur {
        if tree.data(p).is_some_and(|d| d.hidden) {
            return true;
        }
        cur = tree.parent(p);
    }
    false
}

fn push_node(
    tree: &Tree,
    node: Node,
    is_root: bool,
    scale: f32,
    root_name: &str,
    out: &mut Vec<(AkNodeId, AkNode)>,
) {
    let Some(data) = tree.data(node) else {
        return;
    };

    let role = map_role(data.a11y.role, is_root);
    let mut ak = AkNode::new(role);

    if !data.a11y.name.is_empty() {
        ak.set_label(data.a11y.name.as_str());
    } else if is_root && !root_name.is_empty() {
        ak.set_label(root_name);
    } else if matches!(data.a11y.role, A11yRole::Label) {
        match &data.leaf {
            Some(Leaf::Text { text, .. }) | Some(Leaf::StyledText { text, .. })
                if !text.is_empty() =>
            {
                ak.set_label(text.as_str());
            }
            _ => {}
        }
    }

    if let Some(desc) = &data.a11y.description {
        ak.set_description(desc.as_str());
    }
    if let Some(val) = &data.a11y.value {
        ak.set_value(val.as_str());
    }
    if let Some(toggled) = data.a11y.toggled {
        ak.set_toggled(Toggled::from(toggled));
    }
    if let Some(selected) = data.a11y.selected {
        ak.set_selected(selected);
    }
    if let Some(expanded) = data.a11y.expanded {
        ak.set_expanded(expanded);
    }
    if data.disabled || data.a11y.disabled {
        ak.set_disabled();
    }
    if data.hidden {
        ak.set_hidden();
    }
    if data.a11y.modal {
        ak.set_modal();
    }
    if data.a11y.has_popup {
        ak.set_has_popup(HasPopup::Listbox);
    }

    let r = tree.rect(node);
    ak.set_bounds(AkRect {
        x0: (r.x * scale) as f64,
        y0: (r.y * scale) as f64,
        x1: ((r.x + r.w) * scale) as f64,
        y1: ((r.y + r.h) * scale) as f64,
    });

    let disabled = data.disabled || data.a11y.disabled;
    if !disabled {
        if data.focusable {
            ak.add_action(Action::Focus);
        }
        if data.a11y.clickable || data.hit == HitMode::Interactive {
            ak.add_action(Action::Click);
        }
        if matches!(data.a11y.role, A11yRole::ComboBox) {
            ak.add_action(Action::Expand);
            ak.add_action(Action::Collapse);
        }
        if matches!(data.a11y.role, A11yRole::TextInput) {
            ak.add_action(Action::SetValue);
        }
    }

    let children = tree.children(node);
    if !children.is_empty() {
        let child_ids: Vec<AkNodeId> = children.iter().map(|c| ak_id(tree.id(*c))).collect();
        ak.set_children(child_ids);
    }

    out.push((ak_id(data.id), ak));

    for child in children {
        push_node(tree, child, false, scale, root_name, out);
    }
}

/// Extracts string text from an AccessKit [`ActionRequest`] when `action == Action::SetValue`.
#[allow(dead_code)] // used by TextField widget UIA integration
pub fn action_value(req: &ActionRequest) -> Option<&str> {
    match &req.data {
        Some(ActionData::Value(v)) => Some(v.as_ref()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::text::TextSystem;
    use crate::ui::tree::{style, A11yInfo};
    use taffy::Style;

    fn fixed_box(w: f32, h: f32) -> Style {
        Style {
            size: style::size(w, h),
            ..Default::default()
        }
    }

    #[test]
    fn validate_tree_names_passes_when_every_widget_has_a_name() {
        let mut tree = Tree::new(style::column());
        let btn = tree.add_leaf(tree.root(), fixed_box(100.0, 32.0), Leaf::Box);
        tree.set_hit(btn, HitMode::Interactive);
        tree.set_focusable(btn, true);
        tree.set_a11y(
            btn,
            A11yInfo {
                role: A11yRole::Button,
                name: "Start timer".into(),
                clickable: true,
                requires_name: true,
                ..Default::default()
            },
        );

        let toggle = tree.add_leaf(tree.root(), fixed_box(40.0, 20.0), Leaf::Box);
        tree.set_hit(toggle, HitMode::Interactive);
        tree.set_focusable(toggle, true);
        tree.set_a11y(
            toggle,
            A11yInfo {
                role: A11yRole::Switch,
                name: "Auto-pause on idle".into(),
                toggled: Some(true),
                clickable: true,
                requires_name: true,
                ..Default::default()
            },
        );

        assert_eq!(validate_tree_names(&tree), Ok(()));
    }

    #[test]
    fn validate_tree_names_fails_if_any_widget_lacks_a_name() {
        let mut tree = Tree::new(style::column());
        let btn = tree.add_leaf(tree.root(), fixed_box(80.0, 28.0), Leaf::Box);
        tree.set_hit(btn, HitMode::Interactive);
        tree.set_a11y(
            btn,
            A11yInfo {
                role: A11yRole::Button,
                name: "   ".into(),
                ..Default::default()
            },
        );

        let err = validate_tree_names(&tree).expect_err("unnamed button must fail validation");
        assert_eq!(err, vec![(tree.id(btn), A11yRole::Button)]);
    }

    #[test]
    fn build_tree_update_publishes_bounds_role_state_and_actions() {
        let text = TextSystem::new().expect("dwrite");
        let mut tree = Tree::new(style::column());
        let btn = tree.add_leaf(tree.root(), fixed_box(120.0, 32.0), Leaf::Box);
        tree.set_hit(btn, HitMode::Interactive);
        tree.set_focusable(btn, true);
        tree.set_a11y(
            btn,
            A11yInfo {
                role: A11yRole::Switch,
                name: "Launch on startup".into(),
                toggled: Some(true),
                clickable: true,
                requires_name: true,
                ..Default::default()
            },
        );
        tree.layout(400.0, 300.0, &text);

        let update = build_tree_update(Some(&tree), Some(tree.id(btn)), 1.5, (600, 450), "Focus");
        assert_eq!(update.focus, ak_id(tree.id(btn)));
        let (_, ak_btn) = update
            .nodes
            .iter()
            .find(|(id, _)| *id == ak_id(tree.id(btn)))
            .expect("switch node in update");
        assert_eq!(ak_btn.role(), Role::Switch);
        assert_eq!(ak_btn.label(), Some("Launch on startup"));
        assert_eq!(ak_btn.toggled(), Some(Toggled::True));
        assert!(ak_btn.supports_action(Action::Click));
        assert!(ak_btn.supports_action(Action::Focus));
        let bounds = ak_btn.bounds().expect("bounds");
        assert!((bounds.x1 - 180.0).abs() < 0.01);
        assert!((bounds.y1 - 48.0).abs() < 0.01);
    }
}
