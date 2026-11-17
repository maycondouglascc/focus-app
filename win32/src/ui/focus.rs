//! Pure focus model: Tab / Shift+Tab ordering, modal/popup scope trapping,
//! visible focus ring state, and Esc dismissal/drag cancellation.

use super::tree::{Node, Rect, Tree, WidgetId};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)] // Popup/Modal scopes exercised by tests now and widgets in F2
pub enum FocusScope {
    Main,
    Modal(WidgetId),
    Popup(WidgetId),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FocusEntry {
    pub id: WidgetId,
    pub scope: FocusScope,
    pub disabled: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)] // variants dispatched by screens and popups
pub enum EscapeOutcome {
    None,
    CancelDrag,
    ClosePopup {
        popup: WidgetId,
        restore_focus: Option<WidgetId>,
    },
    CloseModal {
        modal: WidgetId,
        restore_focus: Option<WidgetId>,
    },
}

pub struct FocusModel {
    entries: Vec<FocusEntry>,
    /// Stack of active scopes with the focus target to restore when popped.
    scopes: Vec<(FocusScope, Option<WidgetId>)>,
    focused: Option<WidgetId>,
    focus_visible: bool,
    dragging: bool,
}

impl Default for FocusModel {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)] // full focus API consumed across input, popups, dialogs, and widgets
impl FocusModel {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            scopes: vec![(FocusScope::Main, None)],
            focused: None,
            focus_visible: false,
            dragging: false,
        }
    }

    pub fn focused(&self) -> Option<WidgetId> {
        self.focused
    }

    pub fn is_focus_visible(&self) -> bool {
        self.focus_visible && self.focused.is_some()
    }

    pub fn active_scope(&self) -> FocusScope {
        self.scopes
            .last()
            .map(|(s, _)| *s)
            .unwrap_or(FocusScope::Main)
    }

    pub fn set_dragging(&mut self, dragging: bool) {
        self.dragging = dragging;
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// Rebuilds the tab-order table directly from explicit entries.
    pub fn set_entries(&mut self, entries: Vec<FocusEntry>) {
        self.entries = entries;
        self.ensure_valid_focus();
    }

    /// Rebuilds the tab-order table from a `Tree` in pre-order.
    pub fn sync_from_tree(
        &mut self,
        tree: &Tree,
        mut scope_of: impl FnMut(&Tree, Node) -> FocusScope,
    ) {
        let mut entries = Vec::new();
        self.collect_tree(tree, tree.root(), &mut scope_of, &mut entries);
        self.set_entries(entries);
    }

    fn collect_tree(
        &self,
        tree: &Tree,
        node: Node,
        scope_of: &mut impl FnMut(&Tree, Node) -> FocusScope,
        out: &mut Vec<FocusEntry>,
    ) {
        let Some(data) = tree.data(node) else {
            return;
        };
        if data.hidden {
            return;
        }
        if data.focusable {
            out.push(FocusEntry {
                id: data.id,
                scope: scope_of(tree, node),
                disabled: data.disabled,
            });
        }
        for child in tree.children(node) {
            self.collect_tree(tree, child, scope_of, out);
        }
    }

    fn scope_enabled_ids(&self, scope: FocusScope) -> Vec<WidgetId> {
        self.entries
            .iter()
            .filter(|e| e.scope == scope && !e.disabled)
            .map(|e| e.id)
            .collect()
    }

    fn is_valid_in_active(&self, id: WidgetId) -> bool {
        let active = self.active_scope();
        self.entries
            .iter()
            .any(|e| e.id == id && e.scope == active && !e.disabled)
    }

    fn ensure_valid_focus(&mut self) {
        if let Some(id) = self.focused {
            if !self.is_valid_in_active(id) {
                self.focused = None;
                self.focus_visible = false;
            }
        }
    }

    /// Enters a modal dialog scope, trapping Tab navigation until popped.
    pub fn push_modal(&mut self, modal: WidgetId) {
        let prev = self.focused;
        self.scopes.push((FocusScope::Modal(modal), prev));
        self.focused = self
            .scope_enabled_ids(FocusScope::Modal(modal))
            .into_iter()
            .next();
    }

    /// Enters a popup scope (select menu or popover), saving focus to restore on close.
    pub fn push_popup(&mut self, popup: WidgetId) {
        let prev = self.focused;
        self.scopes.push((FocusScope::Popup(popup), prev));
        self.focused = self
            .scope_enabled_ids(FocusScope::Popup(popup))
            .into_iter()
            .next();
    }

    /// Pops the topmost matching scope and restores its saved focus target if still enabled.
    pub fn pop_scope(&mut self, scope: FocusScope) -> Option<WidgetId> {
        if let Some(pos) = self.scopes.iter().rposition(|(s, _)| *s == scope) {
            let (_, restore) = self.scopes.remove(pos);
            if self.scopes.is_empty() {
                self.scopes.push((FocusScope::Main, None));
            }
            self.focused = restore.filter(|id| self.is_valid_in_active(*id));
            return self.focused;
        }
        None
    }

    /// Moves focus forward (`backward == false`) or backward (`backward == true`)
    /// within the active scope, wrapping around. Enables the visible focus ring.
    pub fn tab(&mut self, backward: bool) -> Option<WidgetId> {
        let candidates = self.scope_enabled_ids(self.active_scope());
        if candidates.is_empty() {
            self.focused = None;
            return None;
        }
        let next = match self
            .focused
            .and_then(|cur| candidates.iter().position(|&id| id == cur))
        {
            Some(idx) => {
                if backward {
                    candidates[(idx + candidates.len() - 1) % candidates.len()]
                } else {
                    candidates[(idx + 1) % candidates.len()]
                }
            }
            None => {
                if backward {
                    *candidates.last().unwrap()
                } else {
                    candidates[0]
                }
            }
        };
        self.focused = Some(next);
        self.focus_visible = true;
        Some(next)
    }

    /// Focuses `id` from a pointer click (hides keyboard focus ring).
    pub fn focus_pointer(&mut self, id: WidgetId) -> bool {
        if !self.is_valid_in_active(id) {
            return false;
        }
        let changed = self.focused != Some(id) || self.focus_visible;
        self.focused = Some(id);
        self.focus_visible = false;
        changed
    }

    /// Focuses `id` from keyboard navigation (shows focus ring).
    pub fn focus_keyboard(&mut self, id: WidgetId) -> bool {
        if !self.is_valid_in_active(id) {
            return false;
        }
        let changed = self.focused != Some(id) || !self.focus_visible;
        self.focused = Some(id);
        self.focus_visible = true;
        changed
    }

    pub fn clear_focus(&mut self) -> bool {
        let changed = self.focused.is_some() || self.focus_visible;
        self.focused = None;
        self.focus_visible = false;
        changed
    }

    /// Handles `Esc`: cancels active drag first, else closes topmost popup or modal.
    pub fn on_escape(&mut self) -> EscapeOutcome {
        if self.dragging {
            self.dragging = false;
            return EscapeOutcome::CancelDrag;
        }
        match self.active_scope() {
            FocusScope::Main => EscapeOutcome::None,
            scope @ FocusScope::Popup(popup) => {
                let restore_focus = self.pop_scope(scope);
                EscapeOutcome::ClosePopup {
                    popup,
                    restore_focus,
                }
            }
            scope @ FocusScope::Modal(modal) => {
                let restore_focus = self.pop_scope(scope);
                EscapeOutcome::CloseModal {
                    modal,
                    restore_focus,
                }
            }
        }
    }

    /// Returns the window DIP rect of the focused node when the focus ring should be drawn.
    pub fn focus_ring_rect(&self, tree: &Tree) -> Option<Rect> {
        if !self.focus_visible {
            return None;
        }
        let id = self.focused?;
        let node = tree.node_by_id(id)?;
        let data = tree.data(node)?;
        if data.hidden || data.disabled {
            return None;
        }
        Some(tree.rect(node))
    }
}

#[cfg(test)]
mod tests {
    use taffy::prelude::*;

    use super::super::text::TextSystem;
    use super::super::tree::{style, Leaf};
    use super::*;

    #[test]
    fn tab_and_shift_tab_wrap_and_skip_disabled_entries() {
        let mut fm = FocusModel::new();
        let (a, b, c) = (WidgetId(10), WidgetId(11), WidgetId(12));
        fm.set_entries(vec![
            FocusEntry {
                id: a,
                scope: FocusScope::Main,
                disabled: false,
            },
            FocusEntry {
                id: b,
                scope: FocusScope::Main,
                disabled: true,
            },
            FocusEntry {
                id: c,
                scope: FocusScope::Main,
                disabled: false,
            },
        ]);

        assert_eq!(fm.tab(false), Some(a));
        assert!(fm.is_focus_visible());
        assert_eq!(fm.tab(false), Some(c));
        assert_eq!(fm.tab(false), Some(a));
        assert_eq!(fm.tab(true), Some(c));
        assert_eq!(fm.tab(true), Some(a));
    }

    #[test]
    fn pointer_focus_hides_ring_until_keyboard_navigation() {
        let mut t = Tree::new(Style {
            size: style::size(300.0, 100.0),
            ..style::row()
        });
        let root = t.root();
        let btn = t.add_leaf(
            root,
            Style {
                size: style::size(80.0, 36.0),
                ..Default::default()
            },
            Leaf::Box,
        );
        t.set_focusable(btn, true);
        let text = TextSystem::new().unwrap();
        t.layout(300.0, 100.0, &text);

        let mut fm = FocusModel::new();
        fm.sync_from_tree(&t, |_, _| FocusScope::Main);
        let btn_id = t.id(btn);

        assert!(fm.focus_pointer(btn_id));
        assert_eq!(fm.focused(), Some(btn_id));
        assert_eq!(fm.focus_ring_rect(&t), None);

        assert!(fm.focus_keyboard(btn_id));
        assert_eq!(fm.focus_ring_rect(&t), Some(t.rect(btn)));
    }

    #[test]
    fn modal_and_popup_trap_tab_and_esc_closes_topmost_restoring_focus() {
        let mut fm = FocusModel::new();
        let (main_btn, modal_id, modal_a, modal_b, popup_id, popup_opt) = (
            WidgetId(1),
            WidgetId(20),
            WidgetId(21),
            WidgetId(22),
            WidgetId(30),
            WidgetId(31),
        );
        fm.set_entries(vec![
            FocusEntry {
                id: main_btn,
                scope: FocusScope::Main,
                disabled: false,
            },
            FocusEntry {
                id: modal_a,
                scope: FocusScope::Modal(modal_id),
                disabled: false,
            },
            FocusEntry {
                id: modal_b,
                scope: FocusScope::Modal(modal_id),
                disabled: false,
            },
            FocusEntry {
                id: popup_opt,
                scope: FocusScope::Popup(popup_id),
                disabled: false,
            },
        ]);

        assert!(fm.focus_keyboard(main_btn));
        fm.push_modal(modal_id);
        assert_eq!(fm.focused(), Some(modal_a));
        assert_eq!(fm.tab(false), Some(modal_b));
        assert_eq!(fm.tab(false), Some(modal_a));

        // Open a popup inside the modal and test Esc closes popup first, then modal.
        fm.push_popup(popup_id);
        assert_eq!(fm.focused(), Some(popup_opt));
        assert_eq!(fm.tab(false), Some(popup_opt));

        // Active drag takes precedence over closing popup on Esc.
        fm.set_dragging(true);
        assert_eq!(fm.on_escape(), EscapeOutcome::CancelDrag);

        assert_eq!(
            fm.on_escape(),
            EscapeOutcome::ClosePopup {
                popup: popup_id,
                restore_focus: Some(modal_a),
            }
        );
        assert_eq!(fm.focused(), Some(modal_a));

        assert_eq!(
            fm.on_escape(),
            EscapeOutcome::CloseModal {
                modal: modal_id,
                restore_focus: Some(main_btn),
            }
        );
        assert_eq!(fm.focused(), Some(main_btn));
        assert_eq!(fm.on_escape(), EscapeOutcome::None);
    }

    #[test]
    fn empty_focus_list_is_safe() {
        let mut fm = FocusModel::new();
        assert_eq!(fm.tab(false), None);
        assert_eq!(fm.tab(true), None);
        assert!(!fm.focus_pointer(WidgetId(99)));
    }
}
