//! Selectors: which nodes of a tree a rule applies to.

use std::cell::RefCell;
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
    /// A node the selector doesn't match: the children of a node but one
    /// (`all: [child_of X, not Y]`).
    Not(Box<Selector>),
    /// A node whose parent the selector matches.
    ChildOf(Box<Selector>),
    /// A node one of whose children the selector matches.
    ParentOf(Box<Selector>),
    /// A node with an ancestor the selector matches, at any depth. Unlike
    /// a chain of `ChildOf`, it still matches when the guest wraps the node
    /// in one more `view`.
    Within(Box<Selector>),
    /// A node with a descendant the selector matches, at any depth: the
    /// sheet that holds a given element.
    Has(Box<Selector>),
    /// The `n`th node `of` matches, counted from 0 in tree order: the root
    /// first, then each node's children in their order, depth first. A
    /// guest that draws one element twice gives both copies the same cells
    /// and the same subtree, and only their place tells them apart.
    Nth { of: Box<Selector>, n: usize },
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

    /// The `n`th node this selector matches, in tree order.
    pub fn nth(self, n: usize) -> Self {
        Selector::Nth {
            of: Box::new(self),
            n,
        }
    }
}

/// A tree with each node's parent, for matching selectors.
pub(crate) struct View<'t> {
    tree: &'t Tree,
    parents: HashMap<NodeId, NodeId>,
    /// Every node in tree order, made the first time a [`Selector::Nth`]
    /// needs it.
    order: RefCell<Option<Vec<NodeId>>>,
    /// The node each [`Selector::Nth`] matched, by the address of its `of`.
    /// The selector is kept with it, so a selector made at an address
    /// another one used isn't taken for it.
    nth: RefCell<HashMap<*const Selector, NthFound>>,
}

/// What a [`Selector::Nth`] matched: its `of`, its `n` and the node.
type NthFound = (Selector, usize, Option<NodeId>);

impl<'t> View<'t> {
    pub(crate) fn new(tree: &'t Tree) -> Self {
        let mut parents = HashMap::new();
        for n in tree.nodes() {
            for c in &n.children {
                parents.entry(*c).or_insert(n.id);
            }
        }
        View {
            tree,
            parents,
            order: RefCell::new(None),
            nth: RefCell::new(HashMap::new()),
        }
    }

    /// Every node, the root first, then each node's children in their
    /// order, depth first. Nodes under no parent follow the root's subtree
    /// in id order, each with its own subtree.
    fn tree_order(&self) -> Vec<NodeId> {
        let mut out = Vec::with_capacity(self.tree.len());
        let mut seen = HashSet::new();
        let tops = self.tree.root().into_iter().chain(
            self.tree
                .nodes()
                .map(|n| n.id)
                .filter(|id| !self.parents.contains_key(id)),
        );
        for top in tops {
            let mut stack = vec![top];
            while let Some(n) = stack.pop() {
                if !seen.insert(n) {
                    continue;
                }
                out.push(n);
                stack.extend(self.children(n).iter().rev());
            }
        }
        out
    }

    /// The `n`th node `of` matches, in tree order.
    fn nth(&self, of: &Selector, n: usize) -> Option<NodeId> {
        let key = std::ptr::from_ref(of);
        if let Some((sel, at, found)) = self.nth.borrow().get(&key)
            && sel == of
            && *at == n
        {
            return *found;
        }
        let found = self.select_in_order(of).get(n).copied();
        self.nth.borrow_mut().insert(key, (of.clone(), n, found));
        found
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

    /// Every node `sel` matches, in tree order, as [`Selector::Nth`]
    /// counts them.
    pub(crate) fn select_in_order(&self, sel: &Selector) -> Vec<NodeId> {
        if self.order.borrow().is_none() {
            *self.order.borrow_mut() = Some(self.tree_order());
        }
        let order = self.order.borrow();
        order
            .iter()
            .flatten()
            .copied()
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
            Selector::Not(s) => !self.matches(node, s),
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
            Selector::Has(s) => {
                let mut seen = HashSet::new();
                let mut stack = self.children(node).to_vec();
                while let Some(n) = stack.pop() {
                    if !seen.insert(n) {
                        continue;
                    }
                    if self.matches(n, s) {
                        return true;
                    }
                    stack.extend(self.children(n).iter().copied());
                }
                false
            }
            Selector::Nth { of, n } => self.nth(of, *n) == Some(node),
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
