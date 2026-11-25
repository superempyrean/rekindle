//! `rekindle-wire-inject`: rekindle's transforms on a split client's wire.
//!
//! On a wire-split build, every element the guest creates and every cell it
//! writes reaches the host as one op buffer per turn, and every event
//! reaches the guest as one buffer from the host. [`Engine`] rewrites both
//! before the other side sees them, as a [`Work`] value says:
//!
//! - **Mounts.** Subtrees rekindle creates and mounts under a node of the
//!   guest's, last, or before or after a child a selector finds. The root
//!   is spliced into the guest's own Children op when a later turn rewrites
//!   that list, so the host's keyed diff keeps its node across turns. A turn that
//!   deletes the node takes the mount out first, and the mount is created
//!   again under the next node its selector finds. A node with a press
//!   handler lists `press` in its `on` (320) cell, since a platform raises
//!   only the events a node lists.
//! - **Cells.** Cells written by `PropId` on the nodes a selector finds, after
//!   the guest's ops. A cell is written again only when the host's tree holds
//!   a different value, which happens when the guest writes its own.
//! - **Rows.** The guest's nodes a selector finds are dropped (every op on
//!   the node or a node under it is left out of the turn), have a string
//!   cell rewritten inside the guest's own op, or get cells written.
//! - **Presses.** A press on a guest node a selector finds is consumed,
//!   retargeted at another node, or kept for a plugin, and then passed on
//!   to the guest or not. A
//!   press on rekindle's own nodes is always consumed, after its
//!   [`Handler`] runs. A [`Handler::Plugin`] keeps the press for the host to
//!   deliver ([`Engine::take_presses`]).
//! - **Back.** While the work has a back handler, the system back reaches
//!   it as a press instead of reaching the guest, so a page of rekindle's
//!   closes before the client's own screen does.
//! - **Labels.** An `accessibilityLabel` for each node the guest left
//!   without one, made from another of its cells.
//! - **Recolours.** A colour the guest writes, replaced in every colour cell
//!   that holds it, the guest's and the mounts', or only in the cells of
//!   one [`ColorRole`]: fills, borders, or text and icons. A theme's tokens
//!   map to these through the palette the guest names ([`theme`]), one
//!   recolour for each token a plugin overrides, limited to the token's
//!   role. A mount written in the client's own palette follows the theme
//!   as the guest's nodes do.
//! - **Prop recolours.** Colour cells replaced by which cell they are,
//!   whatever colour they hold: a node's `background` in any theme. A light
//!   and a dark theme write different colours in the same cell, and one
//!   theme's text colour is the other's surface, so a recolour by value
//!   reaches the wrong cells in one of them. A prop recolour can be limited
//!   to the nodes a selector finds, to greys or to opaque cells, and it
//!   paints gradient stops too.
//!
//! - **Text.** Text put into the guest's text field the user last used, at
//!   its caret, as a `forceValue` ([`Engine::insert_text`], [`compose`]).
//! - **Picks.** After [`Engine::pick`], the next press on one of the guest's
//!   nodes is consumed, and its node's selectors are kept, ranked
//!   ([`Engine::take_pick`], [`pick`]).
//!
//! [`Engine::set_work`] replaces the work while turns go through. Cells the
//! old work wrote and the new one doesn't are set back to the guest's own
//! values in the next turn. A mount the new work leaves out is taken out of
//! the host's tree in the next turn, and a new one is created.
//! [`Engine::flush`] makes that turn when the guest has none to send.
//!
//! [`Engine`] keeps two trees: the guest's, from the turns as the guest sent
//! them, and a mirror of the host's, from the turns it rewrote. Rows,
//! presses and labels are matched in the guest's tree, so a dropped or
//! rewritten node is still found by what the guest said. Mounts and cells are
//! placed from the mirror.
//!
//! The turns are `rekindle_wire::turn` and the events
//! `rekindle_wire::event`, both read from Phoenix's Android host. A
//! `ChildEdits` op of the guest's is read as the Children op it amounts to,
//! and a turn the engine rewrites carries whole child lists and no tag-12
//! values, since each names a words list by its place in the buffer. Host
//! frames and UI events other than an event on a node pass unchanged. A
//! BATCH frame's UI events are filtered one by one as a bare event would
//! be, and the frames beside them in the batch pass.
//!
//! # Modelled
//!
//! The node ids rekindle takes ([`RESERVED`] and up) assume the guest never
//! reaches them. The real client should supply how its guest allocates ids,
//! or a range the host never hands out.

pub mod compose;
mod json;
pub mod pick;
mod select;
mod switches;
pub mod theme;
mod work;

use std::collections::{BTreeMap, BTreeSet};

use rekindle_wire::a11y::ids as a11y;
use rekindle_wire::event::{self, Event, UiEvent};
use rekindle_wire::frame::{self, Frame};
use rekindle_wire::motion::ids::WORKLET;
use rekindle_wire::motion::worklet::{PressLook, Worklet};
use rekindle_wire::turn::{ON, SPANS, create_props};
use rekindle_wire::{DecodeError, ElementKind, NodeId, NodeOp, PropId, Tree, Turn, Value};

pub use compose::{
    CHANGE_TEXT, Composer, FORCE_VALUE, Field, Found, Inserted, SELECTION_CHANGE, Typed, VALUE,
};
pub use pick::{Basis, Candidate, Pick};
pub use select::{Selector, TEXT};
pub use switches::{NodeCells, Switch, SwitchState, Switches};
pub use work::{
    Cell, ColorRole, Handler, LabelRule, Mount, MountNode, PluginPress, PluginText, PressAction, PressRule,
    PropRecolor, Recolor, RowAction, RowRule, Work, WorkError, Write,
};

/// The first of the node ids rekindle takes for its own nodes.
pub const RESERVED: NodeId = 0xF000_0000;

/// The cells a [`Recolor`] or [`PropRecolor`] reads and writes: every
/// recolour id that holds one colour.
pub const COLOR_PROPS: &[PropId] = &[
    rekindle_wire::ids::BACKGROUND,
    rekindle_wire::ids::COLOR,
    rekindle_wire::ids::BORDER_COLOR,
    rekindle_wire::ids::TINT,
    rekindle_wire::ids::SCRIM,
    rekindle_wire::ids::SCRIM_COLOR,
    rekindle_wire::ids::FOCUS_BORDER_COLOR,
    rekindle_wire::ids::ELEVATION_COLOR,
    rekindle_wire::ids::NAV_SCRIM_COLOR,
    rekindle_wire::ids::PLACEHOLDER_COLOR,
    rekindle_wire::ids::SELECTION_COLOR,
    rekindle_wire::ids::HANDLE_COLOR,
    rekindle_wire::ids::ACTIVE_BACKGROUND,
    rekindle_wire::ids::ACTIVE_BORDER_COLOR,
];

/// The gradient cells a [`PropRecolor`] can also name. It paints each of
/// the gradient's colour stops as it would a colour cell, so a fade from a
/// surface to transparent stays a fade.
pub const GRADIENT_PROPS: &[PropId] = &[
    rekindle_wire::ids::BACKGROUND_GRADIENT,
    rekindle_wire::ids::TEXT_GRADIENT,
];

/// The `colors` field of a gradient written as an object.
const COLORS: PropId = PropId(577);

/// A colour as the wire carries it, an integer `0xAARRGGBB`, from
/// `0xRRGGBBAA`.
pub fn rgba(hex: u32) -> Value {
    Value::color(hex.rotate_right(8))
}

/// What [`Engine::event`] does with an event.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// The guest receives it as the host raised it.
    Pass,
    /// The guest receives this event instead.
    Replace(Event),
    /// The guest never receives it. If the event pressed one of rekindle's
    /// nodes with a handler, the value is the handler's reply. Phoenix's
    /// dispatch returns no data for an event, so the reply is rekindle's
    /// own model, which the mock's guest reads.
    Consume(Option<Value>),
    /// The guest receives this BATCH frame instead: the one it was sent,
    /// with each UI event in it dropped or replaced. A handler's reply to
    /// an event inside a batch isn't sent, since a dispatch has one reply
    /// slot and the batch's other entries still reach the guest.
    Batch(Vec<u8>),
}

/// Rewrites a guest's turns and the host's events, as its [`Work`] says.
/// See the crate docs.
pub struct Engine {
    work: Work,
    /// The tree as the guest's own turns would leave it.
    guest: Tree,
    /// The host's tree as the rewritten turns leave it.
    mirror: Tree,
    /// The roots of the mounts that have been created.
    created: BTreeSet<NodeId>,
    /// Mounts an earlier work made, to take out of the host's tree in the
    /// next turn.
    unmount: Vec<Mount>,
    /// Presses on nodes with a [`Handler::Plugin`], for the host.
    presses: Vec<PluginPress>,
    /// Text typed in rekindle's own fields since the last [`Engine::take_texts`].
    texts: Vec<PluginText>,
    /// The guest's nodes kept out of the host's tree.
    dropped: BTreeSet<NodeId>,
    /// The cells each of rekindle's nodes should hold.
    cells: BTreeMap<NodeId, Vec<(PropId, Value)>>,
    /// Presses each handler has counted.
    counts: BTreeMap<NodeId, u32>,
    /// Cells an earlier work wrote, to set back to the guest's values unless
    /// the current work writes them.
    restore: BTreeSet<(NodeId, PropId)>,
    /// The guest's text fields.
    fields: compose::Fields,
    /// Ops of rekindle's own on the guest's nodes, for the next turn.
    pending: Vec<NodeOp>,
    composer: Composer,
    /// Slash commands sent from a field, for the host.
    typed: Vec<Typed>,
    /// The last event of each name on one of the guest's nodes.
    last: BTreeMap<PropId, Event>,
    /// Set by [`Engine::pick`] until a press arrives.
    picking: bool,
    picked: Option<Pick>,
}

impl Engine {
    pub fn new(work: Work) -> Result<Engine, WorkError> {
        work.check()?;
        let mut engine = Engine {
            work,
            guest: Tree::new(),
            mirror: Tree::new(),
            created: BTreeSet::new(),
            unmount: Vec::new(),
            presses: Vec::new(),
            texts: Vec::new(),
            dropped: BTreeSet::new(),
            cells: BTreeMap::new(),
            counts: BTreeMap::new(),
            restore: BTreeSet::new(),
            fields: compose::Fields::default(),
            pending: Vec::new(),
            composer: Composer::default(),
            typed: Vec::new(),
            last: BTreeMap::new(),
            picking: false,
            picked: None,
        };
        engine.mount_cells();
        Ok(engine)
    }

    /// Set the cells of the mounts' nodes from the work, recoloured, with
    /// each count handler's count in its cell.
    fn mount_cells(&mut self) {
        let to = self.recolor_map();
        // A prop recolour's selector finds the guest's nodes, so only the
        // ones without one reach a mount.
        let roles: Vec<&PropRecolor> = self
            .work
            .prop_recolors
            .iter()
            .rev()
            .filter(|r| r.select.is_none())
            .collect();
        self.cells = self
            .work
            .mounts
            .iter()
            .flat_map(|m| &m.nodes)
            .map(|n| {
                let cells = n
                    .cells
                    .iter()
                    .map(|c| {
                        let value = prop_recolor_cell(c.prop, &c.value, roles.iter().copied())
                            .or_else(|| recolor_cell(c.prop, &c.value, &to));
                        (c.prop, value.unwrap_or_else(|| c.value.clone()))
                    })
                    .collect();
                (n.id, cells)
            })
            .collect();
        let handlers: Vec<NodeId> = self.handlers().map(|(id, _)| id).collect();
        for id in handlers {
            self.show_count(id);
        }
    }

    pub fn work(&self) -> &Work {
        &self.work
    }

    /// Replace the work. The trees and the counts stay. Cells the old
    /// work's rules wrote and the new one's don't go back to the guest's
    /// values in the next turn. A replaced text goes back only when the
    /// guest writes the cell again.
    ///
    /// A mount the new work keeps with the same shape stays in the host's
    /// tree, and only its cells change. A mount it leaves out, or changes
    /// the shape of, is taken out in the next turn: its root out of its
    /// parent's children, then a Delete of its nodes. A new
    /// mount is created as in the first turn.
    pub fn set_work(&mut self, work: Work) -> Result<(), WorkError> {
        work.check()?;
        let before = self.rule_writes();
        self.restore.extend(
            before
                .into_iter()
                .flat_map(|(n, cells)| cells.into_iter().map(move |(p, _)| (n, p))),
        );
        for old in &self.work.mounts {
            if let Some(new) = work.mounts.iter().find(|m| m.same_shape(old)) {
                // Cells the kept mount no longer writes go to null.
                for (o, n) in old.nodes.iter().zip(&new.nodes) {
                    for c in &o.cells {
                        if !n.cells.iter().any(|x| x.prop == c.prop) {
                            self.restore.insert((o.id, c.prop));
                        }
                    }
                }
                continue;
            }
            if self.created.remove(&old.root) {
                self.unmount.push(old.clone());
            }
            for n in &old.nodes {
                self.counts.remove(&n.id);
            }
        }
        self.work = work;
        self.mount_cells();
        Ok(())
    }

    /// The presses on nodes with a [`Handler::Plugin`] since the last call,
    /// in order.
    pub fn take_presses(&mut self) -> Vec<PluginPress> {
        std::mem::take(&mut self.presses)
    }

    /// The text typed in `input` nodes with a [`Handler::Plugin`] since the
    /// last call, in order, each the field's whole text after the edit.
    pub fn take_texts(&mut self) -> Vec<PluginText> {
        std::mem::take(&mut self.texts)
    }

    /// Put `text` at the caret of the guest's text field the user last
    /// focused or typed in, over its selection, or else the one the guest
    /// focused, the composer's field, or the only one on screen. The
    /// field's new text goes out as a `forceValue` in the next turn, or in
    /// [`Engine::flush`]. `None` when no field is found.
    pub fn insert_text(&mut self, text: &str) -> Option<Inserted> {
        let (node, found) = self.text_field()?;
        let force = self.fields.insert(node, text);
        self.write_force(node, &force);
        Some(Inserted { node, found, force })
    }

    /// The field [`Engine::insert_text`] would put text in.
    pub fn text_field(&self) -> Option<(NodeId, Found)> {
        self.fields
            .target(&self.guest, self.composer.field.as_ref())
    }

    /// Replace `field`'s text with `text`, the caret at its end, as
    /// [`Engine::insert_text`] writes it.
    pub fn set_text(&mut self, field: NodeId, text: &str) -> rekindle_wire::text::ForceValue {
        let force = self.fields.set(field, text);
        self.write_force(field, &force);
        force
    }

    fn write_force(&mut self, node: NodeId, force: &rekindle_wire::text::ForceValue) {
        self.pending.push(NodeOp::Update {
            node,
            props: vec![(FORCE_VALUE, force.to_value())],
        });
    }

    /// Describe the composer: its field, its send button and the slash
    /// commands the host runs.
    pub fn set_composer(&mut self, composer: Composer) {
        self.composer = composer;
    }

    /// Consume the next press or tap on one of the guest's nodes, and make
    /// selectors for its node ([`pick::pick`]), for [`Engine::take_pick`].
    /// The guest never receives the press. A pick that hasn't been taken
    /// is dropped.
    pub fn pick(&mut self) {
        self.picking = true;
        self.picked = None;
    }

    /// Stop waiting for a press after [`Engine::pick`].
    pub fn cancel_pick(&mut self) {
        self.picking = false;
    }

    /// Whether [`Engine::pick`] is waiting for a press.
    pub fn picking(&self) -> bool {
        self.picking
    }

    /// The selectors made for the press after [`Engine::pick`], once it
    /// arrived.
    pub fn take_pick(&mut self) -> Option<Pick> {
        self.picked.take()
    }

    /// Selectors for one of the guest's nodes, as a press on it would make
    /// them.
    pub fn pick_node(&self, node: NodeId) -> Option<Pick> {
        pick::pick(&self.guest, node)
    }

    /// The first of the guest's nodes `sel` matches, in id order.
    pub fn find(&self, sel: &Selector) -> Option<NodeId> {
        select::View::new(&self.guest).first(sel)
    }

    /// The last event named `name` the host raised on one of the guest's
    /// nodes, whatever was done with it: the `rowLongPress` that opened a
    /// menu, say.
    pub fn last_event(&self, name: PropId) -> Option<&Event> {
        self.last.get(&name)
    }

    /// The slash commands sent since the last call, in order.
    pub fn take_typed(&mut self) -> Vec<Typed> {
        std::mem::take(&mut self.typed)
    }

    /// The slash command a press on the send button or a field's `submit`
    /// sends, if the host runs it.
    fn typed_command(&self, ev: &Event) -> Option<Typed> {
        if self.composer.commands.is_empty() {
            return None;
        }
        let field = if ev.name == event::names::SUBMIT && self.fields.text(ev.node).is_some() {
            ev.node
        } else if ev.is_press()
            && let Some(send) = &self.composer.send
            && select::View::new(&self.guest).matches(ev.node, send)
        {
            self.fields
                .target(&self.guest, self.composer.field.as_ref())?
                .0
        } else {
            return None;
        };
        let text = self.fields.text(field)?;
        self.composer.command(text)?;
        Some(Typed {
            field,
            text: text.trim().to_string(),
        })
    }

    /// The guest's text fields, by node.
    pub fn fields(&self) -> &BTreeMap<NodeId, Field> {
        self.fields.all()
    }

    /// The tree as the guest's own turns leave it.
    pub fn guest(&self) -> &Tree {
        &self.guest
    }

    /// The tree as the host holds it after the turns this rewrote.
    pub fn mirror(&self) -> &Tree {
        &self.mirror
    }

    /// The guest's nodes the last turn left out of the host's tree.
    pub fn dropped(&self) -> &BTreeSet<NodeId> {
        &self.dropped
    }

    /// Presses the handler on `node` has counted.
    pub fn count(&self, node: NodeId) -> u32 {
        self.counts.get(&node).copied().unwrap_or(0)
    }

    fn handlers(&self) -> impl Iterator<Item = (NodeId, &Handler)> {
        self.work
            .mounts
            .iter()
            .flat_map(|m| &m.nodes)
            .filter_map(|n| n.on_press.as_ref().map(|h| (n.id, h)))
    }

    /// Write the count of the handler on `id` into the cell it shows it in.
    fn show_count(&mut self, id: NodeId) {
        let n = self.count(id);
        let Some(Handler::Count {
            node,
            prop,
            one,
            many,
        }) = self
            .handlers()
            .find(|(h, _)| *h == id)
            .map(|(_, h)| h.clone())
        else {
            return;
        };
        let value = Value::Str(Handler::count_text(&one, &many, n));
        let cells = self.cells.entry(node).or_default();
        match cells.iter_mut().find(|(p, _)| *p == prop) {
            Some((_, v)) => *v = value,
            None => cells.push((prop, value)),
        }
    }

    /// Decide what happens to one event before the guest sees it. A UI
    /// event that isn't on a node passes, and so does a host frame other
    /// than a BATCH. In a BATCH, each UI event is decided as it would be on
    /// its own, and a batch left with no entries is consumed.
    pub fn event(&mut self, bytes: &[u8]) -> Result<Verdict, DecodeError> {
        if event::is_ui(bytes) {
            return self.ui_event(bytes);
        }
        if bytes.first() != Some(&frame::BATCH) {
            return Ok(Verdict::Pass);
        }
        let batch = Frame::from_bytes(bytes)?;
        let mut entries: Vec<Vec<u8>> = Vec::new();
        let mut changed = false;
        for entry in batch.batch()? {
            match self.event(entry)? {
                Verdict::Pass => entries.push(entry.to_vec()),
                Verdict::Replace(ev) => {
                    entries.push(ev.to_bytes());
                    changed = true;
                }
                Verdict::Batch(inner) => {
                    entries.push(inner);
                    changed = true;
                }
                Verdict::Consume(_) => changed = true,
            }
        }
        if !changed {
            return Ok(Verdict::Pass);
        }
        if entries.is_empty() {
            return Ok(Verdict::Consume(None));
        }
        let body = frame::batch_body(entries.iter().map(Vec::as_slice));
        Ok(Verdict::Batch(Frame { body, ..batch }.to_bytes()))
    }

    /// [`Engine::event`] for one UI event.
    fn ui_event(&mut self, bytes: &[u8]) -> Result<Verdict, DecodeError> {
        let ev = match UiEvent::from_bytes(bytes)? {
            UiEvent::Node(ev) => ev,
            UiEvent::Back => return Ok(self.back()),
            _ => return Ok(Verdict::Pass),
        };
        if ev.node >= RESERVED {
            let handler = self
                .handlers()
                .find(|(id, _)| *id == ev.node)
                .map(|(_, h)| h.clone());
            match handler {
                Some(Handler::Count { .. }) if ev.is_press() => {
                    let n = self.count(ev.node).saturating_add(1);
                    self.counts.insert(ev.node, n);
                    self.show_count(ev.node);
                    return Ok(Verdict::Consume(Some(Value::I32(
                        i32::try_from(n).unwrap_or(i32::MAX),
                    ))));
                }
                Some(Handler::Plugin {
                    plugin,
                    surface,
                    element,
                }) if ev.is_press() => self.presses.push(PluginPress {
                    plugin,
                    surface,
                    element,
                }),
                Some(Handler::Plugin {
                    plugin,
                    surface,
                    element,
                }) if ev.name == compose::CHANGE_TEXT => {
                    if let Some(text) = compose::changed_text(&ev.value) {
                        self.texts.push(PluginText {
                            press: PluginPress {
                                plugin,
                                surface,
                                element,
                            },
                            text,
                        });
                    }
                }
                _ => {}
            }
            return Ok(Verdict::Consume(None));
        }
        if self.picking && (ev.is_press() || ev.name == event::names::TAP) {
            self.picking = false;
            self.picked = pick::pick(&self.guest, ev.node);
            return Ok(Verdict::Consume(None));
        }
        self.last.insert(ev.name, ev.clone());
        if let Some(typed) = self.typed_command(&ev) {
            self.typed.push(typed);
            return Ok(Verdict::Consume(None));
        }
        if !ev.is_press() {
            self.fields.on_event(&ev);
            return Ok(Verdict::Pass);
        }
        let guest = select::View::new(&self.guest);
        for rule in &self.work.presses {
            if !guest.matches(ev.node, &rule.select) {
                continue;
            }
            match &rule.action {
                PressAction::Drop => return Ok(Verdict::Consume(None)),
                PressAction::Retarget(to) => {
                    if let Some(target) = guest.first(to) {
                        return Ok(Verdict::Replace(Event { node: target, ..ev }));
                    }
                }
                PressAction::Plugin {
                    plugin,
                    surface,
                    element,
                    forward,
                } => {
                    self.presses.push(PluginPress {
                        plugin: plugin.clone(),
                        surface: surface.clone(),
                        element: element.clone(),
                    });
                    return Ok(if *forward {
                        Verdict::Pass
                    } else {
                        Verdict::Consume(None)
                    });
                }
            }
        }
        Ok(Verdict::Pass)
    }

    /// The system back: consumed and kept as a press while the work has a
    /// back handler, else passed to the guest.
    fn back(&mut self) -> Verdict {
        match &self.work.back {
            Some(Handler::Plugin {
                plugin,
                surface,
                element,
            }) => {
                self.presses.push(PluginPress {
                    plugin: plugin.clone(),
                    surface: surface.clone(),
                    element: element.clone(),
                });
                Verdict::Consume(None)
            }
            _ => Verdict::Pass,
        }
    }

    /// Rewrite one turn's bytes. `None` when the turn goes through unchanged.
    pub fn filter(&mut self, guest: &[u8]) -> Result<Option<Vec<u8>>, DecodeError> {
        let mut decoded = Turn::from_bytes(guest)?;
        // Dropping or adding ops would move the words lists a tag 12 names.
        decoded.own_words()?;
        let original = self.guest.resolve(&decoded)?;
        self.guest.apply(&original);
        self.fields.on_turn(&original, &self.guest);
        Ok(self.rewrite(&original))
    }

    /// A turn of rekindle's own, for when the work changed and the guest
    /// has nothing to send: the ops that bring the host's tree in line with
    /// the work, as the guest's next turn would carry them. `None` when the
    /// host's tree already is. A host applies it as it applies the guest's
    /// turns, so it goes through the guest's transport like one.
    pub fn flush(&mut self) -> Option<Vec<u8>> {
        self.rewrite(&Turn::new())
    }

    /// The guest's turn `original`, already applied to the guest's tree, as
    /// the host should receive it. `None` when that is unchanged.
    fn rewrite(&mut self, original: &Turn) -> Option<Vec<u8>> {
        let mut turn = self.intercept(original);
        let mut front = self.unmount();
        front.append(&mut self.orphans(&turn));
        self.mirror.apply(&turn);
        let (mut created, mut back) = self.inject(&mut turn);
        front.append(&mut created);
        front.append(&mut turn.ops);
        front.append(&mut back);
        let mut pending = std::mem::take(&mut self.pending);
        self.mirror.apply(&Turn {
            ops: pending.clone(),
        });
        front.append(&mut pending);
        if front == original.ops {
            return None;
        }
        Some(Turn { ops: front }.to_bytes())
    }

    /// The guest's nodes the row rules drop, and the rewrites they make, as
    /// the guest's tree stands.
    fn row_rules(&self) -> (BTreeSet<NodeId>, Vec<Rewrite<'_>>) {
        let guest = select::View::new(&self.guest);
        let mut drop = BTreeSet::new();
        let mut rewrites = Vec::new();
        for rule in &self.work.rows {
            match &rule.action {
                RowAction::Drop => {
                    for n in guest.select(&rule.select) {
                        guest.subtree(n, &mut drop);
                    }
                }
                RowAction::Replace { prop, from, to } => rewrites.push(Rewrite {
                    nodes: guest.select(&rule.select).into_iter().collect(),
                    prop: *prop,
                    from,
                    to,
                }),
                RowAction::Cells(_) => {}
            }
        }
        (drop, rewrites)
    }

    /// The guest's turn with the dropped nodes taken out and the replaced
    /// text rewritten, as the host should apply it.
    fn intercept(&mut self, original: &Turn) -> Turn {
        let (drop, rewrites) = self.row_rules();
        let rewrite = |node: NodeId, props: &mut [(PropId, Value)]| {
            for r in &rewrites {
                r.apply(node, props);
            }
        };
        let guest = select::View::new(&self.guest);
        let kept = |n: &NodeId| !drop.contains(n);

        // Nodes that stopped matching: rebuild them from the guest's tree.
        let restored: Vec<NodeId> = self
            .dropped
            .iter()
            .copied()
            .filter(|n| kept(n) && self.guest.node(*n).is_some())
            .collect();
        // Nodes that started matching after the host mounted them.
        let removed: Vec<NodeId> = drop
            .iter()
            .copied()
            .filter(|n| !self.dropped.contains(n) && self.mirror.node(*n).is_some())
            .collect();

        let mut ops = Vec::new();
        for &n in &restored {
            let Some(kind) = self.guest.node(n).map(|g| g.kind.clone()) else {
                continue;
            };
            let mut props: Vec<(PropId, Value)> = self
                .guest
                .props()
                .node_props(n)
                .map(|(p, v)| (p, v.clone()))
                .collect();
            rewrite(n, &mut props);
            ops.push(NodeOp::Create {
                node: n,
                kind,
                props: create_props(props),
            });
        }
        for &n in &restored {
            let children: Vec<NodeId> = guest.children(n).iter().copied().filter(kept).collect();
            if !children.is_empty() {
                ops.push(NodeOp::Children { node: n, children });
            }
        }

        for op in &original.ops {
            let mut op = op.clone();
            let node = match &mut op {
                NodeOp::Create { node, props, .. } | NodeOp::Update { node, props } => {
                    rewrite(*node, props);
                    Some(*node)
                }
                NodeOp::Children { node, children } => {
                    children.retain(kept);
                    Some(*node)
                }
                NodeOp::Delete { nodes } => {
                    nodes.retain(kept);
                    if nodes.is_empty() {
                        continue;
                    }
                    None
                }
                NodeOp::ScrollTo { node, .. } | NodeOp::Root { node } => Some(*node),
                NodeOp::Focus { node } => *node,
                NodeOp::ChildEdits { .. } | NodeOp::RequestFrame | NodeOp::Motion { .. } => None,
            };
            if node.is_none_or(|n| kept(&n)) {
                ops.push(op);
            }
        }

        // A parent whose children changed only by what was dropped or
        // restored gets its list again.
        let parents: BTreeSet<NodeId> = restored
            .iter()
            .chain(&removed)
            .filter_map(|n| guest.parent(*n))
            .filter(kept)
            .collect();
        for p in parents {
            if !ops
                .iter()
                .any(|op| matches!(op, NodeOp::Children { node, .. } if *node == p))
            {
                let children = guest.children(p).iter().copied().filter(kept).collect();
                ops.push(NodeOp::Children { node: p, children });
            }
        }
        if !removed.is_empty() {
            ops.push(NodeOp::Delete { nodes: removed });
        }
        self.dropped = drop;
        Turn { ops }
    }

    /// The ops that take out the mounts an earlier work made, against the
    /// host's tree before this turn: each root out of its parent's
    /// children, then its nodes removed. They go in front of the turn, so a
    /// mount made again with the same ids is created after them.
    fn unmount(&mut self) -> Vec<NodeOp> {
        let mut ops = Vec::new();
        for m in std::mem::take(&mut self.unmount) {
            let mut out = Vec::new();
            let view = select::View::new(&self.mirror);
            if let Some(parent) = view.parent(m.root) {
                let children = view
                    .children(parent)
                    .iter()
                    .copied()
                    .filter(|c| *c != m.root)
                    .collect();
                out.push(NodeOp::Children {
                    node: parent,
                    children,
                });
            }
            let nodes: Vec<NodeId> = m
                .nodes
                .iter()
                .filter(|n| self.mirror.node(n.id).is_some())
                .map(|n| n.id)
                .collect();
            if !nodes.is_empty() {
                out.push(NodeOp::Delete { nodes });
            }
            self.mirror.apply(&Turn { ops: out.clone() });
            ops.append(&mut out);
            // What the earlier work wrote on its nodes went with them.
            self.restore
                .retain(|(n, _)| m.nodes.iter().all(|x| x.id != *n));
        }
        ops
    }

    /// The ops that take out the mounts whose parent `turn` deletes, against
    /// the host's tree before the turn: each root out of its parent's
    /// children, then its nodes removed. The mount is created again the next
    /// time its selector finds a node, so it never depends on the host
    /// keeping a node whose parent went away.
    fn orphans(&mut self, turn: &Turn) -> Vec<NodeOp> {
        let deleted: BTreeSet<NodeId> = turn
            .ops
            .iter()
            .filter_map(|op| match op {
                NodeOp::Delete { nodes } => Some(nodes),
                _ => None,
            })
            .flatten()
            .copied()
            .collect();
        if deleted.is_empty() {
            return Vec::new();
        }
        let mut ops = Vec::new();
        let mut gone = Vec::new();
        let view = select::View::new(&self.mirror);
        for m in &self.work.mounts {
            if !self.created.contains(&m.root) {
                continue;
            }
            let Some(parent) = view.parent(m.root).filter(|p| deleted.contains(p)) else {
                continue;
            };
            let children = view
                .children(parent)
                .iter()
                .copied()
                .filter(|c| *c != m.root)
                .collect();
            ops.push(NodeOp::Children {
                node: parent,
                children,
            });
            let nodes: Vec<NodeId> = m
                .nodes
                .iter()
                .filter(|n| self.mirror.node(n.id).is_some())
                .map(|n| n.id)
                .collect();
            if !nodes.is_empty() {
                ops.push(NodeOp::Delete { nodes });
            }
            gone.push(m.root);
        }
        for root in gone {
            self.created.remove(&root);
        }
        self.mirror.apply(&Turn { ops: ops.clone() });
        ops
    }

    /// The mounts and the cells rekindle writes: the ops to put in front of
    /// the turn and after it.
    fn inject(&mut self, turn: &mut Turn) -> (Vec<NodeOp>, Vec<NodeOp>) {
        let mut front = Vec::new();
        let mut back = Vec::new();
        let mut writes = Vec::new();
        for i in 0..self.work.mounts.len() {
            if self.mount(i, turn, &mut front, &mut back) {
                for n in &self.work.mounts[i].nodes {
                    writes.push((n.id, self.cells.get(&n.id).cloned().unwrap_or_default()));
                }
            }
        }
        writes.append(&mut self.rule_writes());
        let written: BTreeSet<(NodeId, PropId)> = writes
            .iter()
            .flat_map(|(n, cells)| cells.iter().map(move |(p, _)| (*n, *p)))
            .collect();
        for (node, prop) in std::mem::take(&mut self.restore) {
            if written.contains(&(node, prop)) || self.mirror.node(node).is_none() {
                continue;
            }
            let own = self.guest.props().get(node, prop).cloned();
            writes.push((node, vec![(prop, own.unwrap_or(Value::Null))]));
        }

        for (node, cells) in writes {
            let props: Vec<_> = cells
                .into_iter()
                .filter(|(p, v)| self.mirror.props().get(node, *p).unwrap_or(&Value::Null) != v)
                .collect();
            if !props.is_empty() {
                back.push(NodeOp::Update { node, props });
            }
        }
        let sets = back
            .iter()
            .filter(|op| matches!(op, NodeOp::Update { .. }))
            .cloned()
            .collect();
        self.mirror.apply(&Turn { ops: sets });
        (front, back)
    }

    /// Create mount `i` if it is new and its node is in the host's tree, and
    /// keep it in that node's children, splicing it into the turn's own list
    /// op. False when the node isn't there.
    fn mount(
        &mut self,
        i: usize,
        turn: &mut Turn,
        front: &mut Vec<NodeOp>,
        back: &mut Vec<NodeOp>,
    ) -> bool {
        let mount = &self.work.mounts[i];
        let Some(under) = select::View::new(&self.mirror).first(&mount.under) else {
            return false;
        };
        if !self.created.contains(&mount.root) {
            let mut ops: Vec<NodeOp> = mount
                .nodes
                .iter()
                .map(|n| {
                    let mut props = self.cells.get(&n.id).cloned().unwrap_or_default();
                    if n.on_press.is_some() {
                        // A field reports its edits, anything else its presses.
                        let on = if n.kind == ElementKind::INPUT { "changeText" } else { "press" };
                        props.push((ON, Value::Array(vec![Value::Str(on.into())])));
                    }
                    NodeOp::Create {
                        node: n.id,
                        kind: n.kind.clone(),
                        props: create_props(props),
                    }
                })
                .collect();
            for n in mount.nodes.iter().filter(|n| !n.children.is_empty()) {
                ops.push(NodeOp::Children {
                    node: n.id,
                    children: n.children.clone(),
                });
            }
            self.mirror.apply(&Turn { ops: ops.clone() });
            front.append(&mut ops);
            self.created.insert(mount.root);
        }
        let listed = self
            .mirror
            .node(under)
            .map_or_else(Vec::new, |n| n.children.clone());
        if !listed.contains(&mount.root) {
            let mut mounted = listed;
            let view = select::View::new(&self.mirror);
            let find = |s: &Selector| mounted.iter().position(|c| view.matches(*c, s));
            let at = mount
                .before
                .as_ref()
                .and_then(find)
                .or_else(|| mount.after.as_ref().and_then(find).map(|i| i + 1))
                .unwrap_or(mounted.len());
            mounted.insert(at, mount.root);
            // Splice into the guest's own op when this turn rewrote the list,
            // so the host diffs the list once and keeps the mount.
            let own = turn.ops.iter_mut().find_map(|op| match op {
                NodeOp::Children { node, children } if *node == under => Some(children),
                _ => None,
            });
            match own {
                Some(c) => c.clone_from(&mounted),
                None => back.push(NodeOp::Children {
                    node: under,
                    children: mounted.clone(),
                }),
            }
            self.mirror.apply(&Turn {
                ops: vec![NodeOp::Children {
                    node: under,
                    children: mounted,
                }],
            });
        }
        true
    }

    /// The cells the `cells`, `labels` and row rules write this turn, before
    /// the ones the host's tree already holds are left out.
    fn rule_writes(&self) -> Vec<(NodeId, Vec<(PropId, Value)>)> {
        let pairs = |cells: &[Cell]| -> Vec<(PropId, Value)> {
            cells.iter().map(|c| (c.prop, c.value.clone())).collect()
        };
        let mut writes = Vec::new();
        let mirror = select::View::new(&self.mirror);
        for w in &self.work.cells {
            for n in mirror.select(&w.select) {
                writes.push((n, pairs(&w.cells)));
            }
        }
        let guest = select::View::new(&self.guest);
        let cell = |node: NodeId, p: PropId| {
            self.guest
                .props()
                .get(node, p)
                .filter(|v| **v != Value::Null)
        };
        for rule in &self.work.labels {
            for n in guest.select(&rule.select) {
                if self.mirror.node(n).is_none() || cell(n, a11y::LABEL).is_some() {
                    continue;
                }
                if let Some(Value::Str(s)) = cell(n, rule.from) {
                    writes.push((
                        n,
                        vec![(a11y::LABEL, Value::Str(format!("{}{s}", rule.prefix)))],
                    ));
                }
            }
        }
        for rule in &self.work.rows {
            let RowAction::Cells(cells) = &rule.action else {
                continue;
            };
            for n in guest.select(&rule.select) {
                if self.mirror.node(n).is_some() {
                    writes.push((n, pairs(cells)));
                }
            }
        }
        let ruled: BTreeSet<(NodeId, PropId)> = writes
            .iter()
            .flat_map(|(n, cells)| cells.iter().map(move |(p, _)| (*n, *p)))
            .collect();
        writes.append(&mut self.recolor_writes(&ruled));
        writes
    }

    /// The cells the recolours write, from the colours in the guest's tree,
    /// except the ones in `ruled`, which a rule writes.
    fn recolor_writes(
        &self,
        ruled: &BTreeSet<(NodeId, PropId)>,
    ) -> Vec<(NodeId, Vec<(PropId, Value)>)> {
        if self.work.recolors.is_empty() && self.work.prop_recolors.is_empty() {
            return Vec::new();
        }
        let to = self.recolor_map();
        let guest = select::View::new(&self.guest);
        let mut writes = Vec::new();
        for n in self.guest.nodes() {
            if self.mirror.node(n.id).is_none() {
                continue;
            }
            // The prop recolours that apply to this node, latest first.
            let roles: Vec<&PropRecolor> = self
                .work
                .prop_recolors
                .iter()
                .rev()
                .filter(|r| r.select.as_ref().is_none_or(|s| guest.matches(n.id, s)))
                .collect();
            let mut cells: Vec<(PropId, Value)> = COLOR_PROPS
                .iter()
                .filter(|p| !ruled.contains(&(n.id, **p)))
                .filter_map(|p| {
                    let v = self.guest.props().get(n.id, *p)?;
                    prop_recolor_cell(*p, v, roles.iter().copied())
                        .or_else(|| recolor_cell(*p, v, &to))
                        .map(|v| (*p, v))
                })
                .collect();
            // A gradient's stops, by its cell's role: a fade from a
            // surface's colour keeps fading from the theme's.
            for p in GRADIENT_PROPS {
                if ruled.contains(&(n.id, *p)) {
                    continue;
                }
                let Some(v) = self.guest.props().get(n.id, *p) else {
                    continue;
                };
                let role = if *p == rekindle_wire::ids::TEXT_GRADIENT {
                    ColorRole::Content
                } else {
                    ColorRole::Fill
                };
                let painted = prop_recolor_gradient(*p, v, &roles);
                let v = painted.as_ref().unwrap_or(v);
                let recolored = map_stops(v, |c| to.get(c, role).map(|t| recolored(c >> 24, t)));
                if let Some(v) = recolored.or(painted) {
                    cells.push((*p, v));
                }
            }
            // The colours of a text's spans, which the renderer reads over
            // its `color` cell.
            if !ruled.contains(&(n.id, SPANS))
                && let Some(v) = self.guest.props().get(n.id, SPANS)
                && let Some(v) = map_spans(v, |c| {
                    to.get(c, ColorRole::Content).map(|t| recolored(c >> 24, t))
                })
            {
                cells.push((SPANS, v));
            }
            if !ruled.contains(&(n.id, WORKLET))
                && let Some(w) = self.recolored_worklet(n.id, &to)
            {
                cells.push((WORKLET, w));
            }
            if !cells.is_empty() {
                writes.push((n.id, cells));
            }
        }
        writes
    }

    /// The recolours, by the red, green and blue they replace and the role
    /// of the cells they reach. A later recolour of the same colour and
    /// role wins.
    fn recolor_map(&self) -> RecolorMap {
        RecolorMap(
            self.work
                .recolors
                .iter()
                .map(|r| ((r.from & 0x00ff_ffff, r.role), r.to))
                .collect(),
        )
    }

    /// The node's `worklet` cell with the recolours applied to its colours,
    /// if it is a `press_look` and one of them changed. The worklet paints
    /// a button's fill each frame from its own colours, over the
    /// `background` cell.
    fn recolored_worklet(&self, node: NodeId, to: &RecolorMap) -> Option<Value> {
        let cell = self.guest.props().get(node, WORKLET)?;
        let mut look = PressLook::from_worklet(&Worklet::from_value(cell)?)?;
        look.recolor(|c| to.get(c, ColorRole::Fill).map(|t| recolored(c >> 24, t)))
            .then(|| look.to_worklet().to_value())
    }
}

/// [`Recolor`]s by the colour they replace and their role.
struct RecolorMap(BTreeMap<(u32, ColorRole), u32>);

impl RecolorMap {
    /// The colour a cell of `role` holding `argb` gets, if a recolour names
    /// it: the one for `role` first, then the one for every cell.
    fn get(&self, argb: u32, role: ColorRole) -> Option<u32> {
        let rgb = argb & 0x00ff_ffff;
        self.0
            .get(&(rgb, role))
            .or_else(|| self.0.get(&(rgb, ColorRole::Any)))
            .copied()
    }
}

/// The value a recolour gives cell `prop` holding `value`, if it gives one:
/// `prop` is a colour cell and `value` a colour the map names for the
/// cell's role.
fn recolor_cell(prop: PropId, value: &Value, to: &RecolorMap) -> Option<Value> {
    let role = ColorRole::of(prop)?;
    let c = value.as_color()?;
    to.get(c, role).map(|t| Value::color(recolored(c >> 24, t)))
}

/// The value the first of `roles` that paints cell `prop` holding `value`
/// gives it. A transparent cell is left as it is.
fn prop_recolor_cell<'w>(
    prop: PropId,
    value: &Value,
    mut roles: impl Iterator<Item = &'w PropRecolor>,
) -> Option<Value> {
    let c = value.as_color().filter(|c| c >> 24 != 0)?;
    roles
        .find(|r| r.paints(prop, c))
        .map(|r| Value::color(recolored(c >> 24, r.to)))
}

/// Gradient cell `prop` holding `value`, a list of colours or an object
/// with a `colors` list, with each stop the first of `roles` paints
/// painted. `None` when no stop changes.
fn prop_recolor_gradient(prop: PropId, value: &Value, roles: &[&PropRecolor]) -> Option<Value> {
    map_stops(value, |c| {
        prop_recolor_cell(prop, &Value::color(c), roles.iter().copied()).and_then(|v| v.as_color())
    })
}

/// Gradient `value`, a list of colours (values, or words) or an object
/// with a `colors` list, with `f` applied to each stop, `0xAARRGGBB`.
/// `None` when no stop changes.
fn map_stops(value: &Value, f: impl Fn(u32) -> Option<u32>) -> Option<Value> {
    let stops = |v: &Value| -> Option<Value> {
        let mut changed = false;
        let mut map = |c: u32| match f(c) {
            Some(new) => {
                changed = true;
                new
            }
            None => c,
        };
        let out = match v {
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|v| match v.as_color() {
                        Some(c) => Value::color(map(c)),
                        None => v.clone(),
                    })
                    .collect(),
            ),
            Value::U32Array(words) => Value::U32Array(words.iter().map(|c| map(*c)).collect()),
            _ => return None,
        };
        changed.then_some(out)
    };
    match value {
        Value::Object(fields) => {
            let mut fields = fields.clone();
            let (_, colors) = fields.iter_mut().find(|(p, _)| *p == COLORS)?;
            *colors = stops(colors)?;
            Some(Value::Object(fields))
        }
        v => stops(v),
    }
}

/// A `spans` cell with `f` applied to each span's `color`. `None` when no
/// span changes.
fn map_spans(value: &Value, f: impl Fn(u32) -> Option<u32>) -> Option<Value> {
    let Value::Array(spans) = value else {
        return None;
    };
    let mut changed = false;
    let out = spans
        .iter()
        .map(|span| {
            let Value::Object(fields) = span else {
                return span.clone();
            };
            let mut fields = fields.clone();
            for (p, v) in &mut fields {
                if *p == rekindle_wire::ids::COLOR
                    && let Some(new) = v.as_color().and_then(&f)
                {
                    *v = Value::color(new);
                    changed = true;
                }
            }
            Value::Object(fields)
        })
        .collect();
    changed.then_some(Value::Array(out))
}

/// `to` (`0xRRGGBBAA`) with its alpha multiplied by `alpha` (0 to 255), as
/// the wire's `0xAARRGGBB`.
fn recolored(alpha: u32, to: u32) -> u32 {
    let a = ((to & 0xff) * alpha + 127) / 255;
    (a << 24) | (to >> 8)
}

/// A [`RowAction::Replace`], with the nodes it applies to this turn.
struct Rewrite<'w> {
    nodes: BTreeSet<NodeId>,
    prop: PropId,
    from: &'w str,
    to: &'w str,
}

impl Rewrite<'_> {
    fn apply(&self, node: NodeId, props: &mut [(PropId, Value)]) {
        if !self.nodes.contains(&node) {
            return;
        }
        for (p, v) in props.iter_mut() {
            if let Value::Str(s) = &*v
                && *p == self.prop
                && s.contains(self.from)
            {
                *v = Value::Str(s.replace(self.from, self.to));
            }
        }
    }
}

#[cfg(test)]
mod tests;
