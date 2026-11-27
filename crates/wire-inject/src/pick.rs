//! Selectors made for one node of the guest's tree: what a picker returns
//! for the node the user pressed.
//!
//! [`pick`] builds every selector it can for the node from the cells the
//! guest gave it and its neighbours, counts how many of the tree's nodes
//! each one matches, and ranks them. A selector that matches only the node
//! ranks first. Among those, the score decides: an `accessibilityId` (9)
//! the guest wrote is the most durable, text and labels come next, and
//! structure (`ChildOf` chains, kinds) last. Each level of structure
//! between the node and the id that anchors it costs a few points, since a
//! guest that wraps the node in one more `view` breaks the chain. `Within`
//! an ancestor costs the same at any depth, and survives the wrapper, but
//! matches more nodes.
//!
//! An id that ends in a run of digits (`dm_row_1531461354488201407`) names
//! one entity. Its `Prefix` selector finds every node of the same kind of
//! row, and the whole id finds this one only, so both are offered. Text and
//! labels with digits in them (counts, times) lose points too.
//!
//! When no selector matches the node alone, the best one is offered first
//! as a [`Selector::Nth`] too: the node's place among the nodes it matches,
//! in tree order. A guest that draws an element twice gives both copies the
//! same cells and subtree, so their place is all that tells them apart.

use std::collections::HashSet;

use rekindle_wire::a11y::ids as a11y;
use rekindle_wire::{ElementKind, NodeId, Tree, Value};
use serde::{Deserialize, Serialize};

use crate::select::{Selector, View};

/// How far up [`pick`] looks for ancestors with ids, and how many it uses.
const MAX_SEARCH: usize = 64;
const MAX_ANCESTORS: usize = 3;
/// The longest `ChildOf` chain [`pick`] writes. A deeper ancestor is
/// reached with `Within` only.
const MAX_CHAIN: usize = 4;
/// Text longer than this is a message body or a description, not a name.
const MAX_TEXT: usize = 48;
/// The ranked list keeps this many selectors after the best.
const MAX_FALLBACKS: usize = 8;
/// Points one level of `ChildOf` costs.
const PER_LEVEL: u32 = 4;
/// Points `Within` costs, at any depth.
const WITHIN: u32 = 6;
/// Points text or a label with digits in it costs.
const DIGITS: u32 = 15;
/// Points telling a node from its copies by their order costs.
const NTH: u32 = 10;

/// What a selector is built from, most durable first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    /// The node's own `accessibilityId`.
    Id,
    /// The node's own id, with an ancestor's id to tell it from others.
    IdUnder,
    /// The fixed part of an id that ends in an entity's number.
    IdPrefix,
    /// The whole of an id that ends in an entity's number.
    IdEntity,
    /// A child's `accessibilityId`.
    ChildId,
    /// The node's `accessibilityLabel`, under an ancestor with an id.
    LabelUnder,
    /// The node's text, under an ancestor with an id.
    TextUnder,
    /// The node's `accessibilityLabel`.
    Label,
    /// The node's text.
    Text,
    /// The node's kind, under an ancestor with an id.
    KindUnder,
    /// The node's `role`.
    Role,
    /// The best selector, which matches copies of the node, and which of
    /// them in tree order.
    Nth,
}

/// One selector for the picked node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub selector: Selector,
    pub basis: Basis,
    /// Durability, 0 to 100.
    pub score: u32,
    /// How many of the tree's nodes the selector matches. 1 is the node.
    pub matches: usize,
}

impl Candidate {
    pub fn unique(&self) -> bool {
        self.matches == 1
    }
}

/// What [`pick`] made for a node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pick {
    pub node: NodeId,
    #[serde(with = "crate::json::kind")]
    pub kind: ElementKind,
    /// A name for the element, from the cells the best selector reads:
    /// `settings-row-account`, `dm-row`.
    pub name: String,
    pub best: Candidate,
    /// The other selectors, best first.
    pub fallbacks: Vec<Candidate>,
}

/// Selectors for `node` in `tree`, ranked. `None` when the tree has no such
/// node.
pub fn pick(tree: &Tree, node: NodeId) -> Option<Pick> {
    let n = tree.node(node)?;
    let view = View::new(tree);
    let mut ranked: Vec<Candidate> = Vec::new();
    let mut seen = HashSet::new();
    for (selector, basis, score) in candidates(&view, tree, node, &n.kind) {
        let key = serde_json::to_string(&selector).unwrap_or_default();
        if !seen.insert(key) || !view.matches(node, &selector) {
            continue;
        }
        let matches = view.select(&selector).len();
        // An ancestor that doesn't narrow an id only makes it longer. A
        // label or text keeps its ancestor, since another screen can show
        // the same words.
        if let Some(without) = without_anchor(basis, &selector)
            && view.select(&without).len() <= matches
        {
            continue;
        }
        ranked.push(Candidate {
            selector,
            basis,
            score,
            matches,
        });
    }
    ranked.sort_by(|a, b| {
        b.unique()
            .cmp(&a.unique())
            .then(b.score.cmp(&a.score))
            .then(a.matches.cmp(&b.matches))
    });
    // Of the selectors with the same basis that match as many nodes, the
    // best scored one is enough.
    let mut kept = HashSet::new();
    ranked.retain(|c| kept.insert((c.basis, c.matches)));
    ranked.truncate(MAX_FALLBACKS + 1);
    let first = ranked.first()?;
    let name = name(tree, &view, node, &n.kind, first);
    if let Some(nth) = Some(first)
        .filter(|c| !c.unique())
        .and_then(|c| nth(&view, node, c))
    {
        ranked.insert(0, nth);
    }
    let mut ranked = ranked.into_iter();
    let best = ranked.next()?;
    Some(Pick {
        node,
        kind: n.kind.clone(),
        name,
        best,
        fallbacks: ranked.collect(),
    })
}

/// `best`, which matches more nodes than `node`, narrowed to `node` by its
/// place among them. A guest that draws an element twice makes both copies
/// alike, so where nothing else tells them apart, their order does. It
/// scores less than `best`, since a guest that draws one more copy ahead
/// of it moves the count.
fn nth(view: &View<'_>, node: NodeId, best: &Candidate) -> Option<Candidate> {
    let n = view
        .select_in_order(&best.selector)
        .iter()
        .position(|m| *m == node)?;
    Some(Candidate {
        selector: best.selector.clone().nth(n),
        basis: Basis::Nth,
        score: best.score.saturating_sub(NTH),
        matches: 1,
    })
}

/// An id selector built under an ancestor, without the ancestor: the last
/// selector of its `All`.
fn without_anchor(basis: Basis, sel: &Selector) -> Option<Selector> {
    match sel {
        Selector::All(all) if basis == Basis::IdUnder && all.len() > 1 => {
            Some(Selector::All(all[..all.len() - 1].to_vec()))
        }
        _ => None,
    }
}

/// Every selector for `node`, with its basis and score, unranked.
fn candidates(
    view: &View<'_>,
    tree: &Tree,
    node: NodeId,
    kind: &ElementKind,
) -> Vec<(Selector, Basis, u32)> {
    let mut out = Vec::new();
    let kind_sel = Selector::Kind(kind.clone());
    let label = string_cell(tree, node, a11y::LABEL);
    let text = text_of(view, node);
    let with_kind = |s: Selector| Selector::All(vec![kind_sel.clone(), s]);

    // The node's own id.
    if let Some(id) = string_cell(tree, node, a11y::ID) {
        match entity_prefix(id) {
            Some(prefix) => {
                out.push((id_prefix(prefix), Basis::IdPrefix, 90));
                out.push((Selector::cell(a11y::ID, id), Basis::IdEntity, 80));
            }
            None => out.push((Selector::cell(a11y::ID, id), Basis::Id, 100)),
        }
    }

    // A child's id.
    for c in view.children(node) {
        if let Some(id) = string_cell(tree, *c, a11y::ID) {
            let (sel, score) = match entity_prefix(id) {
                Some(prefix) => (id_prefix(prefix), 70),
                None => (Selector::cell(a11y::ID, id), 75),
            };
            out.push((with_kind(sel.parent_of()), Basis::ChildId, score));
        }
    }

    // What the node holds, under each of the nearest ancestors with ids.
    // An id loses half the cost of the way up, since it already names the
    // node.
    let mut owns: Vec<(Option<Selector>, Basis, u32)> = Vec::new();
    if let Some(id) = string_cell(tree, node, a11y::ID) {
        let own = entity_prefix(id).map_or_else(|| Selector::cell(a11y::ID, id), id_prefix);
        owns.push((Some(own), Basis::IdUnder, 96));
    }
    if let Some(l) = label {
        owns.push((
            Some(Selector::Label(l.to_string())),
            Basis::LabelUnder,
            76 - digits(l),
        ));
    }
    if let Some(t) = &text {
        owns.push((Some(contains(t)), Basis::TextUnder, 72 - digits(t)));
    }
    owns.push((None, Basis::KindUnder, 50));
    for (up, cost) in ancestors(view, tree, node) {
        for (own, basis, score) in &owns {
            let mut all = vec![kind_sel.clone()];
            all.extend(own.clone());
            all.push(up.clone());
            let cost = if *basis == Basis::IdUnder {
                cost / 2
            } else {
                cost
            };
            out.push((Selector::All(all), *basis, score.saturating_sub(cost)));
        }
    }

    // Labels, text and roles, with no id to anchor them.
    let role = string_cell(tree, node, a11y::ROLE);
    if let Some(l) = label {
        let mut all = vec![kind_sel.clone(), Selector::Label(l.to_string())];
        if let Some(r) = role {
            all.push(Selector::cell(a11y::ROLE, r));
        }
        out.push((
            Selector::All(all),
            Basis::Label,
            62u32.saturating_sub(digits(l)),
        ));
    }
    if let Some(t) = &text {
        out.push((
            with_kind(contains(t)),
            Basis::Text,
            45u32.saturating_sub(digits(t)),
        ));
    }
    if let Some(r) = role {
        out.push((with_kind(Selector::cell(a11y::ROLE, r)), Basis::Role, 20));
    }
    out
}

/// The ways up from `node` to each of the nearest ancestors with ids, with
/// what each costs: `Within` the ancestor, and a `ChildOf` chain when the
/// ancestor is close.
fn ancestors(view: &View<'_>, tree: &Tree, node: NodeId) -> Vec<(Selector, u32)> {
    let mut out = Vec::new();
    let mut path: Vec<NodeId> = Vec::new();
    let mut at = node;
    let mut found = 0;
    while let Some(parent) = view.parent(at) {
        if path.len() >= MAX_SEARCH || found >= MAX_ANCESTORS {
            break;
        }
        path.push(parent);
        at = parent;
        let Some(id) = string_cell(tree, parent, a11y::ID) else {
            continue;
        };
        found += 1;
        let entity = if entity_prefix(id).is_some() { 10 } else { 0 };
        let anchor = Selector::cell(a11y::ID, id);
        out.push((anchor.clone().within(), WITHIN + entity));
        if path.len() <= MAX_CHAIN {
            out.push((
                chain(tree, &path, anchor),
                PER_LEVEL * path.len() as u32 + entity,
            ));
        }
    }
    out
}

/// `ChildOf` from the node up `path` (its parent first) to the ancestor
/// `top` matches, the last node of `path`. Each node in between is matched
/// by its kind.
fn chain(tree: &Tree, path: &[NodeId], top: Selector) -> Selector {
    let mut sel = top;
    for n in path[..path.len() - 1].iter().rev() {
        let kind = tree.node(*n).map(|n| n.kind.clone());
        sel = match kind {
            Some(k) => Selector::All(vec![Selector::Kind(k), sel.child_of()]),
            None => sel.child_of(),
        };
    }
    sel.child_of()
}

fn id_prefix(prefix: &str) -> Selector {
    Selector::Prefix {
        prop: a11y::ID,
        prefix: prefix.to_string(),
    }
}

fn contains(text: &str) -> Selector {
    Selector::Text {
        contains: text.to_string(),
        from: None,
    }
}

fn string_cell(tree: &Tree, node: NodeId, prop: rekindle_wire::PropId) -> Option<&str> {
    match tree.props().get(node, prop) {
        Some(Value::Str(s)) if !s.trim().is_empty() => Some(s.as_str()),
        _ => None,
    }
}

/// The node's text, if it is short enough to name it.
fn text_of(view: &View<'_>, node: NodeId) -> Option<String> {
    let text = view.text_of(node);
    let text = text.trim();
    (!text.is_empty() && text.chars().count() <= MAX_TEXT).then(|| text.to_string())
}

/// The part of `id` before a trailing entity number of four digits or
/// more, with what separates them: `dm_row_` for `dm_row_1531461354488201407`.
fn entity_prefix(id: &str) -> Option<&str> {
    let digits = id.len() - id.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let prefix = &id[..id.len() - digits];
    (digits >= 4
        && !prefix
            .trim_end_matches(['_', '-', '.', ':', '/'])
            .is_empty())
    .then_some(prefix)
}

fn digits(s: &str) -> u32 {
    if s.chars().any(|c| c.is_ascii_digit()) {
        DIGITS
    } else {
        0
    }
}

/// A name for the element: the id the best selector reads, else its label
/// or text with its kind, after the ancestor id that anchors it.
fn name(
    tree: &Tree,
    view: &View<'_>,
    node: NodeId,
    kind: &ElementKind,
    best: &Candidate,
) -> String {
    let own = || -> Option<String> {
        let id = string_cell(tree, node, a11y::ID)?;
        Some(slug(entity_prefix(id).unwrap_or(id)))
    };
    let thing = || {
        let what = string_cell(tree, node, a11y::LABEL)
            .map(str::to_string)
            .or_else(|| text_of(view, node))
            .map(|t| slug(&t))
            .filter(|t| !t.is_empty());
        match what {
            Some(w) => format!("{w}-{}", slug(kind.name())),
            None => slug(kind.name()),
        }
    };
    let anchor = || {
        let mut at = node;
        while let Some(p) = view.parent(at) {
            if let Some(id) = string_cell(tree, p, a11y::ID) {
                return Some(slug(entity_prefix(id).unwrap_or(id)));
            }
            at = p;
        }
        None
    };
    let name = match best.basis {
        Basis::Id | Basis::IdUnder | Basis::IdPrefix | Basis::IdEntity => own(),
        Basis::ChildId => view.children(node).iter().find_map(|c| {
            let id = string_cell(tree, *c, a11y::ID)?;
            Some(format!(
                "{}-{}",
                slug(entity_prefix(id).unwrap_or(id)),
                slug(kind.name())
            ))
        }),
        Basis::LabelUnder | Basis::TextUnder | Basis::KindUnder => {
            anchor().map(|a| format!("{a}-{}", thing()))
        }
        Basis::Label | Basis::Text | Basis::Role | Basis::Nth => None,
    };
    let mut name = name.unwrap_or_else(thing);
    if name.chars().count() > 48 {
        name = name.chars().take(48).collect();
        name = name.trim_end_matches('-').to_string();
    }
    name
}

/// Lower case, with each run of other characters as one `-`.
fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}
