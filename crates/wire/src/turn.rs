//! Modelled. The node / turn framing above the value codec, which a
//! recovered guest module or a desktop build's encoder would replace.
//!
//! The APK ships no guest module, so how a turn groups node create / remove /
//! move and property-set ops above the value layer is NOT recovered. What IS
//! recovered constrains the model, and this module stays inside those
//! constraints:
//!
//! - element kinds are interned name strings ([`ElementKind`]), a fixed set;
//! - `ops.rs` apply reconciles two `&[u32]` child-id lists with a keyed diff
//!   ([`diff_children`]);
//! - property sets cross as tag-8 objects and land in `view_props` keyed by
//!   `(node, PropId)` ([`ViewProps`]).
//!
//! Everything else here — the op-code numbers, the per-op framing bytes, the
//! turn header — is a plausible stand-in so the mock can emit one op buffer per
//! turn that this crate decodes and applies. When a real guest module is
//! recovered, this framing is what gets replaced; the value codec, `PropId` table,
//! `view_props` store and sibling diff below it do not.

use std::collections::BTreeMap;

use crate::tree::{ChildOp, Node, NodeId, diff_children};
use crate::value::{Cursor, DecodeError};
use crate::{ElementKind, PropId, Value, ViewProps};

/// Modelled. One mutation against the retained tree, in this crate's framing
/// (see the module docs).
#[derive(Clone, Debug, PartialEq)]
pub enum NodeOp {
    /// Create a node of a given kind. Op-code 0.
    Create { node: NodeId, kind: ElementKind },
    /// Write a node's property set (a tag-8 object). Op-code 1.
    SetProps {
        node: NodeId,
        props: Vec<(PropId, Value)>,
    },
    /// Replace a node's keyed child list. Op-code 2.
    SetChildren { node: NodeId, children: Vec<NodeId> },
    /// Remove a node. Op-code 3.
    Remove { node: NodeId },
}

/// Modelled. One update cycle of the guest's views: the op buffer handed to
/// the host, in this crate's framing (see the module docs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Turn {
    pub ops: Vec<NodeOp>,
}

impl Turn {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, op: NodeOp) {
        self.ops.push(op);
    }

    /// Serialize the turn to one op buffer.
    ///
    /// Framing (MODELLED): `u32` op-count, then each op as a 1-byte op-code and
    /// its operands. `Create` writes `u32` node + a `u32`-len name string.
    /// `SetProps` writes `u32` node + a tag-8 [`Value::Object`] through the real
    /// value codec. `SetChildren` writes `u32` node + a `u32`-len list of `u32`
    /// child ids (the `&[u32]` shape `ops.rs` consumes). `Remove` writes `u32`
    /// node.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.ops.len() as u32).to_le_bytes());
        for op in &self.ops {
            match op {
                NodeOp::Create { node, kind } => {
                    out.push(0);
                    out.extend_from_slice(&node.to_le_bytes());
                    let name = kind.name();
                    out.extend_from_slice(&(name.len() as u32).to_le_bytes());
                    out.extend_from_slice(name.as_bytes());
                }
                NodeOp::SetProps { node, props } => {
                    out.push(1);
                    out.extend_from_slice(&node.to_le_bytes());
                    // The property set is the recovered tag-8 object, byte-for-byte.
                    Value::Object(props.clone()).encode(&mut out);
                }
                NodeOp::SetChildren { node, children } => {
                    out.push(2);
                    out.extend_from_slice(&node.to_le_bytes());
                    out.extend_from_slice(&(children.len() as u32).to_le_bytes());
                    for child in children {
                        out.extend_from_slice(&child.to_le_bytes());
                    }
                }
                NodeOp::Remove { node } => {
                    out.push(3);
                    out.extend_from_slice(&node.to_le_bytes());
                }
            }
        }
        out
    }

    /// Decode a turn from an op buffer. Errors (not panics) on malformed input,
    /// like the value codec.
    pub fn from_bytes(buf: &[u8]) -> Result<Turn, DecodeError> {
        let mut cur = Cursor::new(buf);
        let count = cur.u32()? as usize;
        if count > cur.remaining() {
            return Err(DecodeError::LengthOverflow);
        }
        let mut ops = Vec::with_capacity(count);
        for _ in 0..count {
            let opcode = cur.u8()?;
            let op = match opcode {
                0 => {
                    let node = cur.u32()?;
                    let name = cur.string()?;
                    let kind = ElementKind::from_name(&name).ok_or(DecodeError::BadFraming)?;
                    NodeOp::Create { node, kind }
                }
                1 => {
                    let node = cur.u32()?;
                    let value = cur.value()?;
                    let Value::Object(props) = value else {
                        return Err(DecodeError::BadFraming);
                    };
                    NodeOp::SetProps { node, props }
                }
                2 => {
                    let node = cur.u32()?;
                    let len = cur.u32()? as usize;
                    if len > cur.remaining() / 4 {
                        return Err(DecodeError::LengthOverflow);
                    }
                    let mut children = Vec::with_capacity(len);
                    for _ in 0..len {
                        children.push(cur.u32()?);
                    }
                    NodeOp::SetChildren { node, children }
                }
                3 => NodeOp::Remove { node: cur.u32()? },
                other => return Err(DecodeError::BadTag(other)),
            };
            ops.push(op);
        }
        Ok(Turn { ops })
    }
}

/// The host's retained element tree plus its `view_props` store. Structure
/// (kinds, children) lives in [`Node`]s; visual state lives in [`ViewProps`],
/// keyed by `(node, PropId)` — the same split the host keeps.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    nodes: BTreeMap<NodeId, Node>,
    props: ViewProps,
}

/// What a turn did to each node's children, so a test or a drive can confirm the
/// sibling diff was minimal.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurnReport {
    pub child_ops: Vec<(NodeId, Vec<ChildOp>)>,
}

impl Tree {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn props(&self) -> &ViewProps {
        &self.props
    }

    /// The store, for the host's own writes: what its motion engine shows
    /// each frame, with no turn from the guest.
    pub fn props_mut(&mut self) -> &mut ViewProps {
        &mut self.props
    }

    #[must_use]
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Every node, in id order.
    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Apply a decoded turn, reporting the keyed child diffs it ran. New child
    /// lists are reconciled against the retained ones with [`diff_children`], so
    /// a node kept across turns keeps its identity, as the host's
    /// `element_pool` keeps it.
    pub fn apply(&mut self, turn: &Turn) -> TurnReport {
        let mut report = TurnReport::default();
        for op in &turn.ops {
            match op {
                NodeOp::Create { node, kind } => {
                    self.nodes
                        .entry(*node)
                        .or_insert_with(|| Node::new(*node, *kind))
                        .kind = *kind;
                }
                NodeOp::SetProps { node, props } => {
                    self.props.apply_set(*node, props.clone());
                }
                NodeOp::SetChildren { node, children } => {
                    let old = self
                        .nodes
                        .get(node)
                        .map(|n| n.children.clone())
                        .unwrap_or_default();
                    let ops = diff_children(&old, children);
                    if let Some(n) = self.nodes.get_mut(node) {
                        n.children.clone_from(children);
                    }
                    report.child_ops.push((*node, ops));
                }
                NodeOp::Remove { node } => {
                    self.nodes.remove(node);
                    self.props.clear_node(*node);
                }
            }
        }
        report
    }

    /// Apply a turn straight from its wire bytes: decode, then apply.
    pub fn apply_bytes(&mut self, buf: &[u8]) -> Result<TurnReport, DecodeError> {
        Ok(self.apply(&Turn::from_bytes(buf)?))
    }
}
