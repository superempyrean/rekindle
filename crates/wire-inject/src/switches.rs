//! [`Switches`]: on and off controls in rekindle's own mounts, whose state
//! the host keeps.

use std::collections::{BTreeMap, BTreeSet};

use rekindle_wire::{NodeId, PropId};
use serde::{Deserialize, Serialize};

use crate::{Cell, Recolor, Work, WorkError};

/// Switches in a [`Work`]'s mounts. Each is named by the `element` of the
/// [`Handler::Plugin`](crate::Handler::Plugin) on the node a press flips it
/// with, and says what the mounts' cells and the work's recolours are while
/// it is on and while it is off. The host keeps which are on, flips one on
/// a press ([`Engine::take_presses`](crate::Engine::take_presses)) and sets
/// the work [`Switches::apply`] makes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Switches {
    pub switches: Vec<Switch>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Switch {
    pub element: String,
    /// The state until the user flips it.
    #[serde(default)]
    pub on: bool,
    #[serde(default)]
    pub when_on: SwitchState,
    #[serde(default)]
    pub when_off: SwitchState,
}

/// What a [`Switch`] adds to the work in one of its states.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchState {
    /// Cells of mount nodes. Each replaces the node's cell of the same
    /// `PropId`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<NodeCells>,
    /// Recolours appended to the work's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recolors: Vec<Recolor>,
}

/// Cells for one mount node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeCells {
    pub node: NodeId,
    pub cells: Vec<Cell>,
}

impl Switches {
    pub fn from_json(json: &str) -> Result<Switches, WorkError> {
        serde_json::from_str(json).map_err(|e| WorkError(format!("switches: {e}")))
    }

    pub fn get(&self, element: &str) -> Option<&Switch> {
        self.switches.iter().find(|s| s.element == element)
    }

    /// `base` with every switch in the state `on` gives it, or its own `on`
    /// when `on` doesn't name it. A cell only the other state writes is left
    /// off the node, so a mount that keeps its shape clears it in the next
    /// turn.
    pub fn apply(&self, base: &Work, on: &BTreeMap<String, bool>) -> Work {
        let mut work = base.clone();
        for s in &self.switches {
            let (state, other) = if on.get(&s.element).copied().unwrap_or(s.on) {
                (&s.when_on, &s.when_off)
            } else {
                (&s.when_off, &s.when_on)
            };
            let written: BTreeSet<(NodeId, PropId)> = state
                .cells
                .iter()
                .chain(&other.cells)
                .flat_map(|n| n.cells.iter().map(|c| (n.node, c.prop)))
                .collect();
            for node in work.mounts.iter_mut().flat_map(|m| &mut m.nodes) {
                node.cells.retain(|c| !written.contains(&(node.id, c.prop)));
                for n in state.cells.iter().filter(|n| n.node == node.id) {
                    node.cells.extend(n.cells.iter().cloned());
                }
            }
            work.recolors.extend(state.recolors.iter().copied());
        }
        work
    }
}
