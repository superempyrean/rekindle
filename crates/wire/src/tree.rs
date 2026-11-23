//! The retained element tree and the keyed sibling diff (`ops.rs` apply shape).
//!
//! RECOVERED (shape): `0x174d124` is not a byte decoder. It takes two
//! already-decoded `&[u32]` child-id lists (old children, new children) and
//! diffs them, with `-1` as the absent sentinel. The turn writer runs it to
//! choose between a Children and a `ChildEdits` op (`crate::turn`), keeping a
//! longest increasing subsequence of the old positions in place. A node
//! carries its kind as a string (the reconcile path compares a 4-byte field
//! against `"list"` = `0x7473696c`), not as a numeric tag.
//!
//! MODELLED: the platform's `element_pool` recycling, the structural
//! hash-skip and the transition records are not reproduced. [`diff_children`]
//! reports the same diff as insert, move and remove ops, which is the
//! vocabulary a keyed sibling diff uses.

use crate::ElementKind;

/// A retained node's id. The wire carries these as `u32`; `-1` (all ones) is the
/// host's absent sentinel, so a real id never uses that value.
pub type NodeId = u32;

/// The absent sentinel the host uses in the id-list diff.
pub const ABSENT: u32 = u32::MAX;

/// One sibling-diff operation against the retained tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChildOp {
    /// A node present in the new list but not the old: mount it at `index`.
    Insert { id: NodeId, index: usize },
    /// A node present in both but out of order: move it to `index`.
    Move { id: NodeId, index: usize },
    /// A node present in the old list but not the new: unmount it.
    Remove { id: NodeId },
}

/// Diff two keyed child-id lists into the minimal insert / move / remove
/// set. The guest's diff at `0x174d124` keeps a longest increasing
/// subsequence of old positions in place too. The insert, move and remove
/// ops are rekindle's way of reporting it.
///
/// Old and new are `&[u32]` id lists. Removals come
/// first (old ids absent from new), then the new list is walked; ids absent from
/// old are inserts, and ids present in both but outside the longest increasing
/// run of retained positions are moves. Keeping the longest increasing
/// subsequence in place is what makes the move set minimal.
#[must_use]
pub fn diff_children(old: &[NodeId], new: &[NodeId]) -> Vec<ChildOp> {
    use std::collections::HashMap;

    let old_index: HashMap<NodeId, usize> =
        old.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let new_set: std::collections::HashSet<NodeId> = new.iter().copied().collect();

    let mut ops = Vec::new();

    // Removals: old ids no longer present (sentinel -1 in the new mapping).
    for id in old {
        if !new_set.contains(id) {
            ops.push(ChildOp::Remove { id: *id });
        }
    }

    // Source position of each new child in the old list, or ABSENT if new.
    let sources: Vec<u32> = new
        .iter()
        .map(|id| old_index.get(id).map_or(ABSENT, |i| *i as u32))
        .collect();

    // Longest increasing subsequence over the retained (non-ABSENT) sources:
    // those keep their place, everything else is an insert or a move.
    let keep = lis_positions(&sources);

    for (pos, (id, src)) in new.iter().zip(&sources).enumerate() {
        if *src == ABSENT {
            ops.push(ChildOp::Insert {
                id: *id,
                index: pos,
            });
        } else if !keep.contains(&pos) {
            ops.push(ChildOp::Move {
                id: *id,
                index: pos,
            });
        }
    }

    ops
}

/// Positions (indices into `sources`) that form a longest increasing
/// subsequence of the non-[`ABSENT`] values. These are the children that do not
/// have to move.
fn lis_positions(sources: &[u32]) -> std::collections::HashSet<usize> {
    // Classic O(n log n) patience LIS, strict-increasing, recording predecessors
    // so the actual positions can be reconstructed.
    let mut tails: Vec<usize> = Vec::new(); // index (into sources) of each pile's top
    let mut prev: Vec<Option<usize>> = vec![None; sources.len()];

    for (i, &src) in sources.iter().enumerate() {
        if src == ABSENT {
            continue;
        }
        // First pile whose top value >= src (strict increase).
        let mut lo = 0;
        let mut hi = tails.len();
        while lo < hi {
            let mid = usize::midpoint(lo, hi);
            if sources[tails[mid]] < src {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        prev[i] = if lo == 0 { None } else { Some(tails[lo - 1]) };
        if lo == tails.len() {
            tails.push(i);
        } else {
            tails[lo] = i;
        }
    }

    let mut keep = std::collections::HashSet::new();
    let mut cur = tails.last().copied();
    while let Some(i) = cur {
        keep.insert(i);
        cur = prev[i];
    }
    keep
}

/// A retained element: its kind (an interned name string) and keyed children.
/// Property values live in [`crate::ViewProps`], keyed by `(node, PropId)`, not
/// here — mirroring the host's split of structure from visual state.
#[derive(Clone, Debug)]
pub struct Node {
    pub id: NodeId,
    pub kind: ElementKind,
    pub children: Vec<NodeId>,
}

impl Node {
    #[must_use]
    pub fn new(id: NodeId, kind: ElementKind) -> Self {
        Node {
            id,
            kind,
            children: Vec::new(),
        }
    }
}
