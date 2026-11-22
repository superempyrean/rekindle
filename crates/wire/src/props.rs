//! `view_props`: the host's retained per-`(node, PropId)` value store.
//!
//! RECOVERED (shape): the host keeps mutable visual state keyed by
//! `(view, PropId)` and the guest sends only what changed, as a tag-8 object
//! (`view_props.rs`, setter fn `0x1723148`, store fn `0x175f270`). The property
//! set on the wire is exactly [`Value::Object`].
//!
//! MODELLED: the in-memory representation here is a plain map. The host packs
//! each value into a 32-byte tagged union inside 64-byte records and carries a
//! separate ~13-variant value-type tag for interpolation. rekindle does not
//! need that packing to drive the mock, so this keeps the decoded [`Value`]
//! directly. The *keying* is faithful; the storage bytes are not reproduced.

use std::collections::BTreeMap;

use crate::tree::NodeId;
use crate::{PropId, Value};

/// A property set as it crosses the wire: the tag-8 object's contents.
pub type PropSet = Vec<(PropId, Value)>;

/// Modelled. The host store of per-node property values. The keying by
/// `(node, PropId)` is recovered. The storage is a plain map of decoded values
/// where the host packs 32-byte tagged unions into 64-byte records.
#[derive(Clone, Debug, Default)]
pub struct ViewProps {
    values: BTreeMap<(NodeId, PropId), Value>,
}

impl ViewProps {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Write one cell, as `view_props.set(view, PropId, value)` does.
    pub fn set(&mut self, node: NodeId, prop: PropId, value: Value) {
        self.values.insert((node, prop), value);
    }

    /// Apply a decoded tag-8 property set to a node: each `(PropId, Value)` is
    /// one cell write. This is the sparse-update path the guest uses.
    pub fn apply_set(&mut self, node: NodeId, props: PropSet) {
        for (prop, value) in props {
            self.set(node, prop, value);
        }
    }

    /// Read one cell.
    #[must_use]
    pub fn get(&self, node: NodeId, prop: PropId) -> Option<&Value> {
        self.values.get(&(node, prop))
    }

    /// Drop every cell belonging to a node (called when a node is removed).
    pub fn clear_node(&mut self, node: NodeId) {
        self.values.retain(|(n, _), _| *n != node);
    }

    /// Every cell of a node, in `PropId` order.
    pub fn node_props(&self, node: NodeId) -> impl Iterator<Item = (PropId, &Value)> {
        self.values
            .range((node, PropId(0))..=(node, PropId(u16::MAX)))
            .map(|((_, p), v)| (*p, v))
    }

    /// Total number of cells held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}
