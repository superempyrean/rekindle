//! [`Work`]: what the engine does to a client's turns and events.

use std::collections::BTreeSet;
use std::fmt;

use rekindle_wire::{ElementKind, NodeId, PropId, Value};
use serde::{Deserialize, Serialize};

use crate::{RESERVED, Selector};

/// What the engine does to a client's turns and events. Every list may be
/// empty, and a `Work` with nothing in it leaves the wire alone.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Work {
    /// Subtrees rekindle creates and keeps mounted under a node of the
    /// client's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<Mount>,
    /// Cells written on the nodes a selector finds in the host's tree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<Write>,
    /// What happens to the guest's nodes a selector finds in the guest's
    /// tree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<RowRule>,
    /// What happens to presses on the guest's nodes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presses: Vec<PressRule>,
    /// `accessibilityLabel`s for nodes the guest left without one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<LabelRule>,
    /// Colours the guest writes, replaced wherever it writes them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recolors: Vec<Recolor>,
}

/// One cell: a `PropId` and its value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cell {
    #[serde(with = "crate::json::prop")]
    pub prop: PropId,
    #[serde(with = "crate::json::value")]
    pub value: Value,
}

impl Cell {
    pub fn new(prop: PropId, value: Value) -> Self {
        Cell { prop, value }
    }
}

/// A subtree rekindle creates once, the first time `under` finds a node in
/// the host's tree, and appends to that node's children. When the guest
/// rewrites the children, the root is spliced into the guest's own list, so
/// the host's keyed diff keeps it. A mount is known by its `root`: new work
/// that keeps the root and the subtree's shape only changes its cells.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mount {
    pub under: Selector,
    /// The node appended under `under`. One of `nodes`.
    pub root: NodeId,
    /// Every node of the subtree. Ids are [`RESERVED`] and up.
    pub nodes: Vec<MountNode>,
}

/// One node of a [`Mount`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MountNode {
    pub id: NodeId,
    #[serde(with = "crate::json::kind")]
    pub kind: ElementKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<Cell>,
    /// Nodes of the same mount, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<NodeId>,
    /// What a press on this node does. A press on any of rekindle's nodes
    /// is consumed, with or without a handler, so the guest never receives
    /// an event for a node it didn't create.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_press: Option<Handler>,
}

/// What rekindle does with a press on one of its own nodes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Handler {
    /// Count the presses, reply with the count as an `i32`, and write it
    /// into the `prop` cell of `node`, a node of the same mount: `one`
    /// after one press, `many` with `{n}` replaced by the count otherwise.
    /// The cell starts at zero presses.
    Count {
        node: NodeId,
        #[serde(with = "crate::json::prop")]
        prop: PropId,
        one: String,
        many: String,
    },
    /// Hand the press to a plugin. The engine consumes it with no reply and
    /// keeps it for [`Engine::take_presses`](crate::Engine::take_presses),
    /// so the host delivers it on its own thread, as it delivers a click on
    /// any other of the plugin's controls. `surface` says what the control
    /// is to the host (a slot such as `chat-bar.button`), and `element` is
    /// the plugin's id for it.
    Plugin {
        plugin: String,
        surface: String,
        element: String,
    },
}

/// A press on a node with a [`Handler::Plugin`], waiting for the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginPress {
    pub plugin: String,
    pub surface: String,
    pub element: String,
}

impl Handler {
    /// What a [`Handler::Count`] writes after `n` presses.
    pub fn count_text(one: &str, many: &str, n: u32) -> String {
        if n == 1 {
            one.to_string()
        } else {
            many.replace("{n}", &n.to_string())
        }
    }
}

/// Cells written on every node `select` finds in the host's tree, in a
/// turn after the guest's own ops. A cell is written again only when the
/// tree holds a different value, which happens when the guest writes its
/// own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Write {
    pub select: Selector,
    pub cells: Vec<Cell>,
}

/// What happens to the guest's nodes `select` finds. Nodes are found in the
/// guest's tree, so a node rekindle dropped or rewrote is still found by
/// what the guest said.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RowRule {
    pub select: Selector,
    pub action: RowAction,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RowAction {
    /// The node and everything under it never reach the host's tree. If it
    /// stops matching, it is rebuilt from the guest's cells. If it starts
    /// matching after the host mounted it, it is removed.
    Drop,
    /// Rewrite `from` to `to` in the node's `prop` string cell, inside the
    /// guest's own op.
    Replace {
        #[serde(with = "crate::json::prop")]
        prop: PropId,
        from: String,
        to: String,
    },
    /// Write cells on the node, as a [`Write`] does.
    Cells(Vec<Cell>),
}

/// What happens to a press on a guest node `select` finds. The first rule
/// that matches and applies decides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PressRule {
    pub select: Selector,
    pub action: PressAction,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum PressAction {
    /// The guest never receives the press.
    Drop,
    /// The guest receives the press on the first node this selector finds
    /// instead. If it finds none, the next rule applies.
    Retarget(Selector),
}

/// An `accessibilityLabel` for each node `select` finds that the guest
/// gave none: `prefix` and the string in the node's `from` cell. A node
/// without that cell gets none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabelRule {
    pub select: Selector,
    #[serde(default)]
    pub prefix: String,
    #[serde(with = "crate::json::prop")]
    pub from: PropId,
}

/// Every colour cell of the guest's that holds `from` gets `to` in the
/// host's tree. A cell is one of [`COLOR_PROPS`](crate::COLOR_PROPS) holding
/// an integer colour, `0xAARRGGBB`, and it matches when its red, green and
/// blue are `from`. The cell's alpha is kept, multiplied by `to`'s. When two recolours
/// have the same `from`, the later one applies. A cell a `cells`, `labels`
/// or row rule writes keeps the rule's value.
///
/// Cells are matched in the guest's tree, so a cell the engine recoloured is
/// still found by the colour the guest wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recolor {
    /// `0xRRGGBB`, spelled as six hex digits.
    #[serde(with = "crate::json::rgb")]
    pub from: u32,
    /// `0xRRGGBBAA`, spelled as eight hex digits.
    #[serde(with = "crate::json::rgba")]
    pub to: u32,
}

/// A [`Work`] that can't be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkError(pub String);

impl fmt::Display for WorkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WorkError {}

impl Mount {
    /// Whether `other` is this mount with at most its cells and handlers
    /// changed: the same node, root, ids, kinds and children.
    pub(crate) fn same_shape(&self, other: &Mount) -> bool {
        self.under == other.under
            && self.root == other.root
            && self.nodes.len() == other.nodes.len()
            && self
                .nodes
                .iter()
                .zip(&other.nodes)
                .all(|(a, b)| a.id == b.id && a.kind == b.kind && a.children == b.children)
    }
}

impl Work {
    /// Read a `Work` from JSON. See [`json`](crate::json) for how cells are
    /// spelled.
    pub fn from_json(json: &str) -> Result<Work, WorkError> {
        let work: Work = serde_json::from_str(json).map_err(|e| WorkError(format!("work: {e}")))?;
        work.check()?;
        Ok(work)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a Work serializes")
    }

    pub fn is_empty(&self) -> bool {
        self.mounts.is_empty()
            && self.cells.is_empty()
            && self.rows.is_empty()
            && self.presses.is_empty()
            && self.labels.is_empty()
            && self.recolors.is_empty()
    }

    /// Check that the mounts' ids are rekindle's, unique, and refer to each
    /// other.
    pub fn check(&self) -> Result<(), WorkError> {
        let err = |m: String| Err(WorkError(m));
        let mut ids = BTreeSet::new();
        for n in self.mounts.iter().flat_map(|m| &m.nodes) {
            if n.id < RESERVED {
                return err(format!(
                    "mount node {} is below the reserved range ({RESERVED:#x})",
                    n.id
                ));
            }
            if !ids.insert(n.id) {
                return err(format!("mount node {} appears twice", n.id));
            }
        }
        for m in &self.mounts {
            let own: BTreeSet<NodeId> = m.nodes.iter().map(|n| n.id).collect();
            if !own.contains(&m.root) {
                return err(format!("mount root {} is not one of its nodes", m.root));
            }
            for n in &m.nodes {
                if let Some(c) = n.children.iter().find(|c| !own.contains(c)) {
                    return err(format!(
                        "mount node {} lists {c}, which is not in its mount",
                        n.id
                    ));
                }
                if let Some(Handler::Count { node, .. }) = &n.on_press
                    && !own.contains(node)
                {
                    return err(format!(
                        "the handler on {} writes {node}, which is not in its mount",
                        n.id
                    ));
                }
            }
        }
        Ok(())
    }
}
