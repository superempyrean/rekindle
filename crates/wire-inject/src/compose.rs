//! Text put into the guest's text fields: what `insert-text` does on the
//! wire.
//!
//! A field's text lives on the platform. The platform reports each edit to
//! the guest as `changeText`, an object of `text` and `eventCount`, and the
//! guest sets the field's text with `forceValue` ([`ForceValue`]). The
//! engine follows both directions for every `input` node: the text the
//! platform last reported or the guest last wrote, the platform's
//! `eventCount`, and which field the user focused or typed in last.
//!
//! The platform reports the caret with `selectionChange`, an object of
//! `start` and `end` in UTF-16 units, on a field that lists it.
//!
//! [`Engine::insert_text`](crate::Engine::insert_text) puts text in that
//! field at the caret, over the selection, or at the end when no caret was
//! reported, with a `forceValue` of rekindle's own, as if the guest had
//! written it. Its `eventCount` is the platform's, so the platform applies
//! it without a rebase, and the platform reports the edit to the guest as
//! it reports the user's typing.

use std::collections::BTreeMap;

use rekindle_wire::event::{Event, names};
use rekindle_wire::text::ForceValue;
use rekindle_wire::{ElementKind, NodeId, NodeOp, PropId, Tree, Turn, Value};

use crate::select::{Selector, View};

/// An input's `value` (543).
pub const VALUE: PropId = PropId(543);
/// An input's `forceValue` (209).
pub const FORCE_VALUE: PropId = PropId(209);
/// The `changeText` event (576).
pub const CHANGE_TEXT: PropId = PropId(576);
/// The `selectionChange` event (626).
pub const SELECTION_CHANGE: PropId = PropId(626);

/// The first `seq` of rekindle's own `forceValue`s. The platform applies a
/// `forceValue` whose `seq` differs from the last one it applied, so
/// rekindle's must never equal the guest's next one.
const SEQ_BASE: i64 = 0x5245_4B00_0000_0000;

/// What the engine knows of one field.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Field {
    /// The text the platform last reported or the guest last wrote.
    pub text: String,
    /// The platform's last `eventCount` for the field.
    pub events: Option<i64>,
    /// The selection the platform last reported, `start..end` in UTF-16
    /// units. `None` puts inserted text at the end.
    pub selection: Option<(usize, usize)>,
}

/// Every `input` node's text, and which one the user last used.
#[derive(Clone, Debug, Default)]
pub(crate) struct Fields {
    by_node: BTreeMap<NodeId, Field>,
    /// The field the user last focused or typed in.
    last: Option<NodeId>,
    /// How many `forceValue`s rekindle has written.
    written: i64,
}

/// Where [`Fields::target`] found the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// The field the user last focused or typed in.
    Used,
    /// The field the guest last focused with a Focus op.
    Focused,
    /// The field the composer selector names.
    Selector,
    /// The only field on screen.
    Only,
}

/// What [`Engine::insert_text`](crate::Engine::insert_text) wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct Inserted {
    pub node: NodeId,
    pub found: Found,
    /// The `forceValue` written: the field's text after the insert, and
    /// the caret after the inserted text.
    pub force: ForceValue,
}

impl Fields {
    /// Note a UI event on a guest node.
    pub(crate) fn on_event(&mut self, ev: &Event) {
        if ev.name == names::FOCUS {
            self.last = Some(ev.node);
        } else if ev.name == CHANGE_TEXT {
            let field = self.by_node.entry(ev.node).or_default();
            match &ev.value {
                Value::Str(s) => field.text.clone_from(s),
                v => {
                    if let Some(f) = ForceValue::from_value(v) {
                        field.text = f.text;
                        // `changeText` names its count `eventCount`, which
                        // `ForceValue` reads too.
                        field.events = f.event_count.or(field.events);
                    }
                }
            }
            self.last = Some(ev.node);
        } else if ev.name == SELECTION_CHANGE
            && let Some(f) = ForceValue::from_value(&ev.value)
        {
            let field = self.by_node.entry(ev.node).or_default();
            field.selection = Some(span(f.start, f.end));
            self.last = Some(ev.node);
        }
    }

    /// Note a turn the guest sent, already applied to `tree`.
    pub(crate) fn on_turn(&mut self, turn: &Turn, tree: &Tree) {
        for op in &turn.ops {
            match op {
                NodeOp::Create { node, kind, props } if *kind == ElementKind::INPUT => {
                    let mut field = Field::default();
                    write(&mut field, props);
                    self.by_node.insert(*node, field);
                }
                NodeOp::Update { node, props } => {
                    if let Some(field) = self.by_node.get_mut(node) {
                        write(field, props);
                    }
                }
                NodeOp::Delete { nodes } => {
                    for n in nodes {
                        self.by_node.remove(n);
                    }
                }
                _ => {}
            }
        }
        if self.last.is_some_and(|n| tree.node(n).is_none()) {
            self.last = None;
        }
    }

    /// The field text goes into: the one the user last used, the one the
    /// guest focused, the one `composer` names, or the only one on screen.
    pub(crate) fn target(&self, tree: &Tree, composer: Option<&Selector>) -> Option<(NodeId, Found)> {
        let view = View::new(tree);
        let shown = |n: NodeId| self.by_node.contains_key(&n) && on_screen(tree, &view, n);
        if let Some(n) = self.last.filter(|n| shown(*n)) {
            return Some((n, Found::Used));
        }
        if let Some(n) = tree.focus().filter(|n| shown(*n)) {
            return Some((n, Found::Focused));
        }
        if let Some(sel) = composer
            && let Some(n) = view.select(sel).into_iter().find(|n| shown(*n))
        {
            return Some((n, Found::Selector));
        }
        let mut on = self.by_node.keys().copied().filter(|n| shown(*n));
        match (on.next(), on.next()) {
            (Some(n), None) => Some((n, Found::Only)),
            _ => None,
        }
    }

    /// Put `text` in `node`'s field over its selection, or at its end: the
    /// `forceValue` to write, with the caret after the text.
    pub(crate) fn insert(&mut self, node: NodeId, text: &str) -> ForceValue {
        self.written += 1;
        let field = self.by_node.entry(node).or_default();
        let len = utf16_len(&field.text);
        let (start, end) = field.selection.unwrap_or((len, len));
        let (start, end) = (start.min(len), end.min(len));
        let (a, b) = (byte_at(&field.text, start), byte_at(&field.text, end));
        field.text.replace_range(a..b, text);
        let caret = start + utf16_len(text);
        field.selection = Some((caret, caret));
        self.last = Some(node);
        let at = i32::try_from(caret).unwrap_or(i32::MAX);
        ForceValue {
            seq: SEQ_BASE + self.written,
            text: field.text.clone(),
            start: at,
            end: at,
            event_count: field.events,
        }
    }

    /// Every field's id and what is known of it.
    pub(crate) fn all(&self) -> &BTreeMap<NodeId, Field> {
        &self.by_node
    }
}

/// The text a Create or Update sets: its `forceValue`'s, with its
/// selection, or its `value`.
fn write(field: &mut Field, props: &[(PropId, Value)]) {
    for (p, v) in props {
        if *p == FORCE_VALUE
            && let Some(f) = ForceValue::from_value(v)
        {
            field.text = f.text;
            field.selection = Some(span(f.start, f.end));
        } else if *p == VALUE
            && let Value::Str(s) = v
        {
            field.text.clone_from(s);
        }
    }
}

/// A selection as `start..end`, in order, with a negative end at 0.
fn span(start: i32, end: i32) -> (usize, usize) {
    let (a, b) = (start.max(0) as usize, end.max(0) as usize);
    (a.min(b), a.max(b))
}

fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// The byte index of UTF-16 offset `at` in `s`, or of the char it falls
/// inside.
fn byte_at(s: &str, at: usize) -> usize {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        if units >= at {
            return i;
        }
        units += c.len_utf16();
    }
    s.len()
}

/// Whether `node` hangs under the window's root.
fn on_screen(tree: &Tree, view: &View<'_>, node: NodeId) -> bool {
    let Some(root) = tree.root() else {
        return false;
    };
    let mut at = node;
    for _ in 0..4096 {
        if at == root {
            return true;
        }
        match view.parent(at) {
            Some(p) => at = p,
            None => return false,
        }
    }
    false
}
