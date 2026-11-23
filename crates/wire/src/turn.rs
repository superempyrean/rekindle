//! A turn: the op buffer the guest's runtime writes once per update cycle.
//!
//! Read out of the turn writer in Phoenix's Android host
//! (`liblibdiscord_wasm_host.so`, build 356207), and emulated byte for byte.
//! The first op written to an empty buffer writes the version byte `0x01`
//! before itself (`0x17262a4`). Ops follow to the end of the buffer, with no
//! op count and no length per op. A turn with no ops is an empty buffer.
//!
//! | op | name | operands |
//! |---:|---|---|
//! | 1 | Create | `u32` node, `u16` kind length, kind, `u16` n, n × (`u16` `PropId`, value) |
//! | 2 | Update | `u32` node, `u16` n, n × (`u16` `PropId`, value) |
//! | 3 | Children | `u32` parent, `u32` n, n × `u32` child |
//! | 4 | Delete | `u32` n, n × `u32` node |
//! | 5 | Root | `u32` node |
//! | 6 | Focus | `u8` 0, or `u8` 1 and `u32` node |
//! | 7 | `ScrollTo` | `u32` node, value |
//! | 8 | `RequestFrame` | none |
//! | 9 | Motion | `u32` n, n × `u32` word |
//! | 10 | `ChildEdits` | `u32` parent, `u32` n, n × (`u32` keep, `u32` del, `u32` m, m × `u32` id) |
//!
//! A property list in Create and Update has a `u16` count, where a tag-8
//! object has a `u32` one. An Update sends a removed property as its `PropId`
//! and Null, and the writer takes an Update with no entries back out of the
//! buffer. Create writes `text` (499) or `spans` (468) first and `on` (320),
//! the names of the events the node listens for, last ([`create_props`]).
//! Delete lists every node of the removed subtree.
//!
//! The guest sends `ChildEdits` instead of Children when its diff of the old
//! and new lists is smaller ([`child_edits`]). The platform applies the edits
//! to the parent's previous list: per edit it copies `keep` children, skips
//! `del` and appends the `m` ids, and after the last edit it copies the rest
//! of the previous list ([`apply_edits`]).
//!
//! The values of a turn share one words table (tags 11 and 12), which starts
//! empty for every buffer. A tag-12 value names a list by its index among the
//! tag-11 and tag-12 values before it in the same buffer, so dropping, adding
//! or moving ops can break the references after them.
//! [`Turn::own_words`] replaces each tag 12 with the list it stands for,
//! after which the ops can be rewritten freely.

use std::collections::{BTreeMap, BTreeSet};

use crate::tree::{ChildOp, Node, NodeId, diff_children};
use crate::value::{Cursor, DecodeError};
use crate::{ElementKind, PropId, PropSet, Value, ViewProps};

/// The byte a turn's buffer starts with.
pub const VERSION: u8 = 1;

/// `text` (499), which Create writes first.
pub const TEXT: PropId = PropId(499);
/// `spans` (468), an array of span objects, in place of `text`.
pub const SPANS: PropId = PropId(468);
/// `on` (320), an array of the event names a node listens for, which Create
/// writes last.
pub const ON: PropId = PropId(320);

/// The fewest children a list needs before the guest considers `ChildEdits`.
pub const EDITS_MIN: usize = 4;

/// One edit of a `ChildEdits` op.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildEdit {
    /// Children of the previous list copied over.
    pub keep: u32,
    /// Children of the previous list skipped after them.
    pub del: u32,
    /// Ids appended after that.
    pub ids: Vec<NodeId>,
}

/// One op of a turn.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeOp {
    /// Op 1. A new node of `kind` with its properties.
    Create {
        node: NodeId,
        kind: ElementKind,
        props: PropSet,
    },
    /// Op 2. Properties written on a node. Null removes one.
    Update { node: NodeId, props: PropSet },
    /// Op 3. A node's whole child list.
    Children { node: NodeId, children: Vec<NodeId> },
    /// Op 4. Nodes removed, every node of each removed subtree.
    Delete { nodes: Vec<NodeId> },
    /// Op 5. The window's root node.
    Root { node: NodeId },
    /// Op 6. The focused node, or none.
    Focus { node: Option<NodeId> },
    /// Op 7. Scroll a node to a position.
    ScrollTo { node: NodeId, value: Value },
    /// Op 8. Ask the platform for another frame.
    RequestFrame,
    /// Op 9. Words for the platform's motion engine.
    Motion { words: Vec<u32> },
    /// Op 10. A node's child list as edits of its previous one.
    ChildEdits { node: NodeId, edits: Vec<ChildEdit> },
}

/// One turn's ops.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Turn {
    pub ops: Vec<NodeOp>,
}

/// `props` in the order Create writes them: `text` or `spans` first, `on`
/// last, the rest as given.
#[must_use]
pub fn create_props(mut props: PropSet) -> PropSet {
    let rank = |p: PropId| match p {
        TEXT | SPANS => 0,
        ON => 2,
        _ => 1,
    };
    props.sort_by_key(|(p, _)| rank(*p));
    props
}

/// The edits that take `old` to `new`, if the guest would send them in
/// place of the whole list: `new` holds at least [`EDITS_MIN`] children,
/// and three words per edit plus the ids inserted come to fewer words than
/// `new`. The children kept in place are a longest increasing subsequence
/// of their old positions. A last edit that only keeps is left out, since
/// the rest of the previous list is kept.
#[must_use]
pub fn child_edits(old: &[NodeId], new: &[NodeId]) -> Option<Vec<ChildEdit>> {
    if new.len() < EDITS_MIN {
        return None;
    }
    let kept = kept_in_place(old, new);
    let mut edits = Vec::new();
    let (mut o, mut n) = (0, 0);
    loop {
        let mut keep = 0;
        while o < old.len() && n < new.len() && old[o] == new[n] && kept.contains(&old[o]) {
            o += 1;
            n += 1;
            keep += 1;
        }
        let mut del = 0;
        while o < old.len() && !kept.contains(&old[o]) {
            o += 1;
            del += 1;
        }
        let mut ids = Vec::new();
        while n < new.len() && !kept.contains(&new[n]) {
            ids.push(new[n]);
            n += 1;
        }
        if del == 0 && ids.is_empty() {
            break;
        }
        edits.push(ChildEdit { keep, del, ids });
    }
    let words = 3 * edits.len() + edits.iter().map(|e| e.ids.len()).sum::<usize>();
    (words < new.len()).then_some(edits)
}

/// The children of `new` that keep their place: a longest increasing
/// subsequence of their positions in `old`.
fn kept_in_place(old: &[NodeId], new: &[NodeId]) -> BTreeSet<NodeId> {
    let keep_ops = diff_children(old, new);
    let moved: BTreeSet<NodeId> = keep_ops
        .iter()
        .filter_map(|op| match op {
            ChildOp::Move { id, .. } | ChildOp::Insert { id, .. } => Some(*id),
            ChildOp::Remove { .. } => None,
        })
        .collect();
    let old: BTreeSet<NodeId> = old.iter().copied().collect();
    new.iter()
        .copied()
        .filter(|id| old.contains(id) && !moved.contains(id))
        .collect()
}

/// `old` with `edits` applied, or `None` if an edit runs past its end.
#[must_use]
pub fn apply_edits(old: &[NodeId], edits: &[ChildEdit]) -> Option<Vec<NodeId>> {
    let mut out = Vec::with_capacity(old.len());
    let mut at = 0usize;
    for e in edits {
        let keep = at.checked_add(e.keep as usize)?;
        out.extend_from_slice(old.get(at..keep)?);
        at = keep.checked_add(e.del as usize)?;
        if at > old.len() {
            return None;
        }
        out.extend_from_slice(&e.ids);
    }
    out.extend_from_slice(&old[at..]);
    Some(out)
}

fn put_u16(out: &mut Vec<u8>, v: usize) {
    out.extend_from_slice(&(u16::try_from(v).unwrap_or(u16::MAX)).to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_ids(out: &mut Vec<u8>, ids: &[NodeId]) {
    put_u32(out, ids.len() as u32);
    for id in ids {
        put_u32(out, *id);
    }
}

/// A `u16`-counted property list. The count saturates at `0xFFFF`, as the
/// writer's does, and only that many entries are written.
fn put_props(out: &mut Vec<u8>, props: &PropSet) {
    let n = props.len().min(usize::from(u16::MAX));
    put_u16(out, n);
    for (id, value) in &props[..n] {
        out.extend_from_slice(&id.0.to_le_bytes());
        value.encode(out);
    }
}

/// `s` cut to at most `max` bytes at a char boundary.
fn cut(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

impl Turn {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, op: NodeOp) {
        self.ops.push(op);
    }

    /// The op's code on the wire.
    #[must_use]
    pub fn code(op: &NodeOp) -> u8 {
        match op {
            NodeOp::Create { .. } => 1,
            NodeOp::Update { .. } => 2,
            NodeOp::Children { .. } => 3,
            NodeOp::Delete { .. } => 4,
            NodeOp::Root { .. } => 5,
            NodeOp::Focus { .. } => 6,
            NodeOp::ScrollTo { .. } => 7,
            NodeOp::RequestFrame => 8,
            NodeOp::Motion { .. } => 9,
            NodeOp::ChildEdits { .. } => 10,
        }
    }

    /// The turn's buffer: the version byte, then each op. Empty when the
    /// turn has no ops. A property list is written as given, so a Create
    /// built by hand should go through [`create_props`] first.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.ops.is_empty() {
            return out;
        }
        out.push(VERSION);
        for op in &self.ops {
            out.push(Turn::code(op));
            match op {
                NodeOp::Create { node, kind, props } => {
                    put_u32(&mut out, *node);
                    let name = cut(kind.name(), usize::from(u16::MAX));
                    put_u16(&mut out, name.len());
                    out.extend_from_slice(name.as_bytes());
                    put_props(&mut out, props);
                }
                NodeOp::Update { node, props } => {
                    put_u32(&mut out, *node);
                    put_props(&mut out, props);
                }
                NodeOp::Children { node, children } => {
                    put_u32(&mut out, *node);
                    put_ids(&mut out, children);
                }
                NodeOp::Delete { nodes } => put_ids(&mut out, nodes),
                NodeOp::Root { node } => put_u32(&mut out, *node),
                NodeOp::Focus { node } => match node {
                    None => out.push(0),
                    Some(n) => {
                        out.push(1);
                        put_u32(&mut out, *n);
                    }
                },
                NodeOp::ScrollTo { node, value } => {
                    put_u32(&mut out, *node);
                    value.encode(&mut out);
                }
                NodeOp::RequestFrame => {}
                NodeOp::Motion { words } => put_ids(&mut out, words),
                NodeOp::ChildEdits { node, edits } => {
                    put_u32(&mut out, *node);
                    put_u32(&mut out, edits.len() as u32);
                    for e in edits {
                        put_u32(&mut out, e.keep);
                        put_u32(&mut out, e.del);
                        put_ids(&mut out, &e.ids);
                    }
                }
            }
        }
        out
    }

    /// Replace each tag-12 value in the turn's ops with the tag-11 list it
    /// stands for, against the turn's own words table. Afterwards no value
    /// refers to another, so ops can be dropped, added or reordered. A
    /// reference to a list the turn doesn't hold is an error.
    pub fn own_words(&mut self) -> Result<(), DecodeError> {
        let mut table = Vec::new();
        for op in &mut self.ops {
            match op {
                NodeOp::Create { props, .. } | NodeOp::Update { props, .. } => {
                    for (_, v) in props {
                        v.own_words(&mut table)?;
                    }
                }
                NodeOp::ScrollTo { value, .. } => value.own_words(&mut table)?,
                _ => {}
            }
        }
        Ok(())
    }

    /// Decode a turn's buffer. An empty buffer is a turn with no ops.
    /// Malformed input is an error, not a panic.
    pub fn from_bytes(buf: &[u8]) -> Result<Turn, DecodeError> {
        let mut ops = Vec::new();
        if buf.is_empty() {
            return Ok(Turn { ops });
        }
        let mut cur = Cursor::new(buf);
        if cur.u8()? != VERSION {
            return Err(DecodeError::BadFraming);
        }
        while cur.remaining() > 0 {
            ops.push(read_op(&mut cur)?);
        }
        Ok(Turn { ops })
    }
}

fn read_ids(cur: &mut Cursor<'_>) -> Result<Vec<NodeId>, DecodeError> {
    let n = cur.u32()? as usize;
    if n > cur.remaining() / 4 {
        return Err(DecodeError::LengthOverflow);
    }
    (0..n).map(|_| cur.u32()).collect()
}

fn read_props(cur: &mut Cursor<'_>) -> Result<PropSet, DecodeError> {
    let n = usize::from(cur.u16()?);
    if n > cur.remaining() / 3 {
        return Err(DecodeError::LengthOverflow);
    }
    let mut props = Vec::with_capacity(n);
    for _ in 0..n {
        let id = PropId(cur.u16()?);
        props.push((id, cur.value()?));
    }
    Ok(props)
}

fn read_op(cur: &mut Cursor<'_>) -> Result<NodeOp, DecodeError> {
    Ok(match cur.u8()? {
        1 => {
            let node = cur.u32()?;
            let len = usize::from(cur.u16()?);
            let name = core::str::from_utf8(cur.take(len)?).map_err(|_| DecodeError::BadUtf8)?;
            NodeOp::Create {
                node,
                kind: ElementKind::new(name),
                props: read_props(cur)?,
            }
        }
        2 => NodeOp::Update {
            node: cur.u32()?,
            props: read_props(cur)?,
        },
        3 => NodeOp::Children {
            node: cur.u32()?,
            children: read_ids(cur)?,
        },
        4 => NodeOp::Delete {
            nodes: read_ids(cur)?,
        },
        5 => NodeOp::Root { node: cur.u32()? },
        6 => NodeOp::Focus {
            node: match cur.u8()? {
                0 => None,
                1 => Some(cur.u32()?),
                _ => return Err(DecodeError::BadFraming),
            },
        },
        7 => NodeOp::ScrollTo {
            node: cur.u32()?,
            value: cur.value()?,
        },
        8 => NodeOp::RequestFrame,
        9 => NodeOp::Motion {
            words: read_ids(cur)?,
        },
        10 => {
            let node = cur.u32()?;
            let n = cur.u32()? as usize;
            if n > cur.remaining() / 12 {
                return Err(DecodeError::LengthOverflow);
            }
            let mut edits = Vec::with_capacity(n);
            for _ in 0..n {
                let keep = cur.u32()?;
                let del = cur.u32()?;
                edits.push(ChildEdit {
                    keep,
                    del,
                    ids: read_ids(cur)?,
                });
            }
            NodeOp::ChildEdits { node, edits }
        }
        other => return Err(DecodeError::BadTag(other)),
    })
}

/// Whether an op carries a tag-12 value.
fn has_patch(op: &NodeOp) -> bool {
    fn patch(v: &Value) -> bool {
        match v {
            Value::Sparse { .. } => true,
            Value::Array(items) => items.iter().any(patch),
            Value::Object(props) => props.iter().any(|(_, v)| patch(v)),
            Value::Map(entries) => entries.iter().any(|(_, v)| patch(v)),
            _ => false,
        }
    }
    match op {
        NodeOp::Create { props, .. } | NodeOp::Update { props, .. } => {
            props.iter().any(|(_, v)| patch(v))
        }
        NodeOp::ScrollTo { value, .. } => patch(value),
        _ => false,
    }
}

/// The platform's retained element tree plus its `view_props` store.
/// Structure (kinds, children) lives in [`Node`]s; visual state lives in
/// [`ViewProps`], keyed by `(node, PropId)`.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    nodes: BTreeMap<NodeId, Node>,
    props: ViewProps,
    root: Option<NodeId>,
    focus: Option<NodeId>,
}

/// What a turn did to each node's children, so a test or a drive can confirm
/// the sibling diff was minimal.
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

    /// The store, for the platform's own writes: what its motion engine
    /// shows each frame, with no turn from the guest.
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

    /// The node the last Root op named.
    #[must_use]
    pub fn root(&self) -> Option<NodeId> {
        self.root
    }

    /// The node the last Focus op named.
    #[must_use]
    pub fn focus(&self) -> Option<NodeId> {
        self.focus
    }

    /// `turn` with each `ChildEdits` op written out as the Children op it
    /// amounts to, against this tree and the ops before it. An edit list
    /// that runs past its parent's list is an error.
    pub fn resolve(&self, turn: &Turn) -> Result<Turn, DecodeError> {
        let mut lists: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
        let mut ops = Vec::with_capacity(turn.ops.len());
        for op in &turn.ops {
            match op {
                NodeOp::ChildEdits { node, edits } => {
                    let old = lists.get(node).cloned().unwrap_or_else(|| {
                        self.node(*node)
                            .map(|n| n.children.clone())
                            .unwrap_or_default()
                    });
                    let children = apply_edits(&old, edits).ok_or(DecodeError::BadFraming)?;
                    lists.insert(*node, children.clone());
                    ops.push(NodeOp::Children {
                        node: *node,
                        children,
                    });
                }
                NodeOp::Children { node, children } => {
                    lists.insert(*node, children.clone());
                    ops.push(op.clone());
                }
                NodeOp::Create { node, .. } => {
                    lists.insert(*node, Vec::new());
                    ops.push(op.clone());
                }
                _ => ops.push(op.clone()),
            }
        }
        Ok(Turn { ops })
    }

    /// Apply a decoded turn, reporting the keyed child diffs it ran. New
    /// child lists are reconciled against the retained ones with
    /// [`diff_children`], so a node kept across turns keeps its identity.
    /// A Create replaces any node with its id. A `ChildEdits` op that runs
    /// past its parent's list leaves the list as it was. A tag-12 value is
    /// stored as the list it stands for ([`Turn::own_words`]), since the
    /// next buffer starts a new words table.
    pub fn apply(&mut self, turn: &Turn) -> TurnReport {
        let mut owned;
        let turn = if turn.ops.iter().any(has_patch) {
            owned = turn.clone();
            // A reference the turn can't resolve stays as it was sent.
            let _ = owned.own_words();
            &owned
        } else {
            turn
        };
        let mut report = TurnReport::default();
        for op in &turn.ops {
            match op {
                NodeOp::Create { node, kind, props } => {
                    self.nodes.insert(*node, Node::new(*node, kind.clone()));
                    self.props.clear_node(*node);
                    self.props.apply_set(*node, props.clone());
                }
                NodeOp::Update { node, props } => {
                    self.props.apply_set(*node, props.clone());
                }
                NodeOp::Children { node, children } => {
                    self.set_children(*node, children, &mut report);
                }
                NodeOp::ChildEdits { node, edits } => {
                    let old = self
                        .nodes
                        .get(node)
                        .map(|n| n.children.clone())
                        .unwrap_or_default();
                    if let Some(children) = apply_edits(&old, edits) {
                        self.set_children(*node, &children, &mut report);
                    }
                }
                NodeOp::Delete { nodes } => {
                    for node in nodes {
                        self.nodes.remove(node);
                        self.props.clear_node(*node);
                        if self.focus == Some(*node) {
                            self.focus = None;
                        }
                    }
                }
                NodeOp::Root { node } => self.root = Some(*node),
                NodeOp::Focus { node } => self.focus = *node,
                NodeOp::ScrollTo { .. } | NodeOp::RequestFrame | NodeOp::Motion { .. } => {}
            }
        }
        report
    }

    fn set_children(&mut self, node: NodeId, children: &[NodeId], report: &mut TurnReport) {
        let old = self
            .nodes
            .get(&node)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        let ops = diff_children(&old, children);
        if let Some(n) = self.nodes.get_mut(&node) {
            n.children = children.to_vec();
        }
        report.child_ops.push((node, ops));
    }

    /// Apply a turn straight from its wire bytes: decode, then apply.
    pub fn apply_bytes(&mut self, buf: &[u8]) -> Result<TurnReport, DecodeError> {
        Ok(self.apply(&Turn::from_bytes(buf)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_take_the_old_list_to_the_new_one() {
        let old = [1, 2, 3, 4, 5, 6, 7, 8];
        for new in [
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9],
            vec![2, 3, 4, 5, 6, 7, 8, 9],
            vec![1, 2, 3, 10, 4, 5, 6, 7, 8],
            vec![1, 2, 4, 5, 6, 7, 8],
        ] {
            let edits = child_edits(&old, &new).expect("smaller than the list");
            assert_eq!(apply_edits(&old, &edits).as_deref(), Some(&new[..]));
        }
        // Short lists and big changes go as Children.
        assert_eq!(child_edits(&[1, 2], &[1, 2, 3]), None);
        assert_eq!(child_edits(&old, &[8, 7, 6, 5, 4, 3, 2, 1]), None);
        assert_eq!(child_edits(&[], &[1, 2, 3, 4]), None);
        assert_eq!(
            apply_edits(
                &[1],
                &[ChildEdit {
                    keep: 2,
                    del: 0,
                    ids: vec![]
                }]
            ),
            None
        );
    }

    #[test]
    fn a_patch_names_a_list_of_its_own_buffer() {
        let words = PropId(740);
        let mut turn = Turn {
            ops: vec![
                NodeOp::Update {
                    node: 1,
                    props: vec![(words, Value::U32Array(vec![1, 2, 3, 4]))],
                },
                NodeOp::Update {
                    node: 2,
                    props: vec![(
                        words,
                        Value::Sparse {
                            list: 0,
                            pairs: vec![(3, 9)],
                        },
                    )],
                },
            ],
        };
        let bytes = turn.to_bytes();
        // On its own the second op names a list its buffer doesn't hold.
        let second = Turn {
            ops: vec![turn.ops[1].clone()],
        };
        assert_eq!(
            Turn::from_bytes(&second.to_bytes()),
            Err(DecodeError::BadWordsRef)
        );
        let mut tree = Tree::new();
        tree.apply_bytes(&bytes).unwrap();
        assert_eq!(
            tree.props().get(2, words),
            Some(&Value::U32Array(vec![1, 2, 3, 9]))
        );
        turn.own_words().unwrap();
        turn.ops.remove(0);
        let alone = Turn::from_bytes(&turn.to_bytes()).unwrap();
        assert_eq!(
            alone.ops[0],
            NodeOp::Update {
                node: 2,
                props: vec![(words, Value::U32Array(vec![1, 2, 3, 9]))],
            }
        );
    }

    #[test]
    fn create_writes_text_first_and_on_last() {
        let props = create_props(vec![
            (ON, Value::Array(vec![Value::Str("press".into())])),
            (PropId(90), Value::color(0xff00_0000)),
            (TEXT, Value::Str("hi".into())),
        ]);
        let order: Vec<PropId> = props.iter().map(|(p, _)| *p).collect();
        assert_eq!(order, vec![TEXT, PropId(90), ON]);
    }
}
