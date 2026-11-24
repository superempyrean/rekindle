//! Selectors: which nodes of a tree a rule applies to.

use std::collections::{HashMap, HashSet};

use rekindle_wire::a11y::ids as a11y;
use rekindle_wire::{ElementKind, NodeId, PropId, Tree, Value};
use serde::{Deserialize, Serialize};

/// The `text` cell (499), which [`Selector::Text`] reads.
pub const TEXT: PropId = PropId(499);

/// Which nodes a rule applies to, by what the tree holds. A selector names
/// cells and structure, never a client: the client's own vocabulary comes
/// in as the values a rule asks for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Selector {
    /// A node whose `prop` cell holds `value`.
    Cell {
        #[serde(with = "crate::json::prop")]
        prop: PropId,
        #[serde(with = "crate::json::value")]
        value: Value,
    },
    /// A node whose `prop` cell is a string that starts with `prefix`: a
    /// guest that puts an id after a fixed name (`message.avatar:<id>`).
    Prefix {
        #[serde(with = "crate::json::prop")]
        prop: PropId,
        prefix: String,
    },
    /// A node of this kind.
    Kind(#[serde(with = "crate::json::kind")] ElementKind),
    /// A node whose `accessibilityLabel` (11) is this string.
    Label(String),
    /// A node whose subtree's `text` (499) cells, joined in tree order,
    /// contain `contains`. With `from`, only the cells of the nodes it
    /// selects count.
    Text {
        contains: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<Box<Selector>>,
    },
    /// A node every selector matches.
    All(Vec<Selector>),
    /// A node any selector matches.
    Any(Vec<Selector>),
    /// A node whose parent the selector matches.
    ChildOf(Box<Selector>),
    /// A node one of whose children the selector matches.
    ParentOf(Box<Selector>),
    /// A node with an ancestor the selector matches, at any depth. Unlike
    /// a chain of `ChildOf`, it still matches when the guest wraps the node
    /// in one more `view`.
    Within(Box<Selector>),
}

impl Selector {
    /// A node whose `prop` cell holds the string `value`.
    pub fn cell(prop: PropId, value: &str) -> Self {
        Selector::Cell {
            prop,
            value: Value::Str(value.into()),
        }
    }

    pub fn child_of(self) -> Self {
        Selector::ChildOf(Box::new(self))
    }

    pub fn parent_of(self) -> Self {
        Selector::ParentOf(Box::new(self))
    }

    pub fn within(self) -> Self {
        Selector::Within(Box::new(self))
    }
}

/// A tree with each node's parent, for matching selectors.
pub(crate) struct View<'t> {
    tree: &'t Tree,
    parents: HashMap<NodeId, NodeId>,
}

impl<'t> View<'t> {
    pub(crate) fn new(tree: &'t Tree) -> Self {
        let mut parents = HashMap::new();
        for n in tree.nodes() {
            for c in &n.children {
                parents.entry(*c).or_insert(n.id);
            }
        }
        View { tree, parents }
    }

    pub(crate) fn parent(&self, node: NodeId) -> Option<NodeId> {
        self.parents.get(&node).copied()
    }

    pub(crate) fn children(&self, node: NodeId) -> &'t [NodeId] {
        self.tree.node(node).map_or(&[], |n| n.children.as_slice())
    }

    /// Every node `sel` matches, in id order.
    pub(crate) fn select(&self, sel: &Selector) -> Vec<NodeId> {
        self.tree
            .nodes()
            .map(|n| n.id)
            .filter(|id| self.matches(*id, sel))
            .collect()
    }

    /// The first node `sel` matches, in id order.
    pub(crate) fn first(&self, sel: &Selector) -> Option<NodeId> {
        self.tree
            .nodes()
            .map(|n| n.id)
            .find(|id| self.matches(*id, sel))
    }

    pub(crate) fn matches(&self, node: NodeId, sel: &Selector) -> bool {
        match sel {
            Selector::Cell { prop, value } => self.tree.props().get(node, *prop) == Some(value),
            Selector::Prefix { prop, prefix } => matches!(
                self.tree.props().get(node, *prop),
                Some(Value::Str(s)) if s.starts_with(prefix.as_str())
            ),
            Selector::Kind(kind) => self.tree.node(node).is_some_and(|n| n.kind == *kind),
            Selector::Label(label) => {
                matches!(self.tree.props().get(node, a11y::LABEL), Some(Value::Str(s)) if s == label)
            }
            Selector::Text { contains, from } => {
                let mut text = String::new();
                self.text(node, from.as_deref(), &mut HashSet::new(), &mut text);
                text.contains(contains.as_str())
            }
            Selector::All(all) => all.iter().all(|s| self.matches(node, s)),
            Selector::Any(any) => any.iter().any(|s| self.matches(node, s)),
            Selector::ChildOf(s) => self.parent(node).is_some_and(|p| self.matches(p, s)),
            Selector::ParentOf(s) => self.children(node).iter().any(|c| self.matches(*c, s)),
            Selector::Within(s) => {
                let mut seen = HashSet::new();
                let mut at = node;
                while let Some(p) = self.parent(at) {
                    if !seen.insert(p) {
                        return false;
                    }
                    if self.matches(p, s) {
                        return true;
                    }
                    at = p;
                }
                false
            }
        }
    }

    /// The `text` cells of `node` and the nodes under it, joined in tree
    /// order.
    pub(crate) fn text_of(&self, node: NodeId) -> String {
        let mut text = String::new();
        self.text(node, None, &mut HashSet::new(), &mut text);
        text
    }

    /// The `text` cells of `node` and the nodes under it, in tree order.
    fn text(
        &self,
        node: NodeId,
        from: Option<&Selector>,
        seen: &mut HashSet<NodeId>,
        out: &mut String,
    ) {
        if !seen.insert(node) {
            return;
        }
        if from.is_none_or(|f| self.matches(node, f))
            && let Some(Value::Str(s)) = self.tree.props().get(node, TEXT)
        {
            out.push_str(s);
        }
        for c in self.children(node) {
            self.text(*c, from, seen, out);
        }
    }

    /// `node` and every node under it.
    pub(crate) fn subtree(&self, node: NodeId, out: &mut std::collections::BTreeSet<NodeId>) {
        if out.insert(node) {
            for c in self.children(node) {
                self.subtree(*c, out);
            }
        }
    }
}
