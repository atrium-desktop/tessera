//! In-memory semantic UI tree repository for shell chrome.

use std::collections::HashMap;
use tessera_primitives::accessibility::{AccessibleNode, AccessibleRole, SemanticTreeUpdate};

/// In-memory repository of accessible nodes exposed by the compositor shell.
#[derive(Debug, Clone, Default)]
pub struct ShellSemanticTree {
    root_id: u64,
    nodes: HashMap<u64, AccessibleNode>,
}

impl ShellSemanticTree {
    /// Create an empty semantic tree.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply an incremental tree update emitted by shell chrome components.
    pub fn apply_update(&mut self, update: SemanticTreeUpdate) {
        if update.root_id != 0 {
            self.root_id = update.root_id;
        }

        for id in update.removed_ids {
            self.nodes.remove(&id);
        }

        for node in update.nodes {
            self.nodes.insert(node.id, node);
        }
    }

    /// Get the root node ID.
    #[must_use]
    pub fn root_id(&self) -> u64 {
        self.root_id
    }

    /// Retrieve an accessible node by identifier.
    #[must_use]
    pub fn get_node(&self, id: u64) -> Option<&AccessibleNode> {
        self.nodes.get(&id)
    }

    /// Return total node count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Check if tree is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Check whether a node can execute an action (action barrier check).
    ///
    /// Disabled elements reject actions. Elements with sensitive roles require
    /// explicit validation.
    #[must_use]
    pub fn can_execute_action(&self, id: u64) -> bool {
        let Some(node) = self.get_node(id) else {
            return false;
        };

        if node.state.disabled {
            return false;
        }

        // Sensitive password fields reject blind programmatic activation
        if node.role == AccessibleRole::PasswordText {
            return false;
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_primitives::accessibility::AccessibleState;
    use tessera_primitives::geometry::Rect;

    #[test]
    fn tree_applies_incremental_updates() {
        let mut tree = ShellSemanticTree::new();
        let mut node1 = AccessibleNode::new(1, AccessibleRole::Window, Rect::new(0, 0, 800, 600));
        node1.name = Some("Shell Root".into());
        node1.children = vec![2];

        let mut node2 = AccessibleNode::new(2, AccessibleRole::PushButton, Rect::new(10, 10, 100, 30));
        node2.name = Some("Launchpad".into());

        tree.apply_update(SemanticTreeUpdate {
            root_id: 1,
            nodes: vec![node1, node2],
            removed_ids: vec![],
        });

        assert_eq!(tree.len(), 2);
        assert_eq!(tree.root_id(), 1);
        assert_eq!(tree.get_node(2).and_then(|n| n.safe_name()), Some("Launchpad"));
        assert!(tree.can_execute_action(2));
    }

    #[test]
    fn disabled_or_sensitive_nodes_reject_action() {
        let mut tree = ShellSemanticTree::new();
        let mut disabled_btn = AccessibleNode::new(1, AccessibleRole::PushButton, Rect::default());
        disabled_btn.state = AccessibleState {
            disabled: true,
            ..Default::default()
        };

        let password_field = AccessibleNode::new(2, AccessibleRole::PasswordText, Rect::default());

        tree.apply_update(SemanticTreeUpdate {
            root_id: 1,
            nodes: vec![disabled_btn, password_field],
            removed_ids: vec![],
        });

        assert!(!tree.can_execute_action(1), "disabled button must reject action");
        assert!(!tree.can_execute_action(2), "password field must reject blind action execution");
    }
}
