//! The layout of a tab's panes: a tree of splits.

use gpui::Axis;

/// A pane, or a split into several equally sized areas.
/// `Axis::Horizontal` puts the areas side by side, `Axis::Vertical` stacks them.
#[derive(Debug, Clone, PartialEq)]
pub enum Node<T> {
    Pane(T),
    Split { axis: Axis, children: Vec<Node<T>> },
}

impl<T: PartialEq + Clone> Node<T> {
    /// The panes, in reading order.
    pub fn panes(&self) -> Vec<T> {
        let mut panes = Vec::new();
        self.collect(&mut panes);
        panes
    }

    fn collect(&self, panes: &mut Vec<T>) {
        match self {
            Node::Pane(pane) => panes.push(pane.clone()),
            Node::Split { children, .. } => children.iter().for_each(|child| child.collect(panes)),
        }
    }

    /// Places `new` right after `target`, along `axis`. Returns `false` if
    /// `target` is not in the tree.
    pub fn split(&mut self, target: &T, axis: Axis, new: T) -> bool {
        match self {
            Node::Pane(pane) if pane == target => {
                let existing = Node::Pane(pane.clone());
                *self = Node::Split { axis, children: vec![existing, Node::Pane(new)] };
                true
            }
            Node::Pane(_) => false,
            Node::Split { axis: split_axis, children } => {
                // Splitting along the parent split's axis adds a sibling area
                // instead of nesting one more split.
                let position = children.iter().position(|child| *child == Node::Pane(target.clone()));
                if let Some(index) = position
                    && *split_axis == axis
                {
                    children.insert(index + 1, Node::Pane(new));
                    return true;
                }
                for child in children {
                    if child.split(target, axis, new.clone()) {
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Removes `target`. Returns `false` if it is absent, or if it is the root
    /// (the tab then has no pane left: closing it is up to the caller).
    pub fn remove(&mut self, target: &T) -> bool {
        let Node::Split { children, .. } = self else {
            return false;
        };
        let position = children.iter().position(|child| *child == Node::Pane(target.clone()));
        let removed = match position {
            Some(index) => {
                children.remove(index);
                true
            }
            None => children.iter_mut().any(|child| child.remove(target)),
        };
        // A split with a single area no longer serves any purpose.
        if children.len() == 1 {
            *self = children.remove(0);
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Axis::{Horizontal, Vertical};

    #[test]
    fn splitting_along_the_parent_axis_adds_a_sibling() {
        let mut root = Node::Pane(1);
        assert!(root.split(&1, Horizontal, 2));
        assert!(root.split(&1, Horizontal, 3));
        assert_eq!(root.panes(), vec![1, 3, 2]);
        assert!(matches!(&root, Node::Split { children, .. } if children.len() == 3));
    }

    #[test]
    fn splitting_across_nests_a_new_split() {
        let mut root = Node::Pane(1);
        root.split(&1, Horizontal, 2);
        assert!(root.split(&2, Vertical, 3));
        assert_eq!(
            root,
            Node::Split {
                axis: Horizontal,
                children: vec![
                    Node::Pane(1),
                    Node::Split { axis: Vertical, children: vec![Node::Pane(2), Node::Pane(3)] },
                ],
            }
        );
        assert!(!root.split(&9, Vertical, 4));
    }

    #[test]
    fn removing_collapses_single_child_splits() {
        let mut root = Node::Pane(1);
        root.split(&1, Horizontal, 2);
        root.split(&2, Vertical, 3);
        assert!(root.remove(&3));
        assert_eq!(
            root,
            Node::Split { axis: Horizontal, children: vec![Node::Pane(1), Node::Pane(2)] }
        );
        assert!(root.remove(&1));
        assert_eq!(root, Node::Pane(2));
        assert!(!root.remove(&2), "the root does not remove itself");
    }
}
