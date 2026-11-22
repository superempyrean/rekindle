//! `rekindle-wire`: Phoenix's recovered gpui-native wire shape, so the mock can
//! speak the real bytes.
//!
//! Phoenix is GPUI cut in two across a wasm boundary. A sandboxed wasm guest's
//! runtime reconciles its views against a retained element tree and hands the
//! native host one op buffer per turn. On the Android build the platform's
//! views apply the ops and paint; whether a desktop build paints them in the
//! guest's runtime is not known yet. This crate is the portion of that seam
//! that was recovered
//! byte-for-byte from the shipped Android host library
//! (`liblibdiscord_wasm_host.so`, build 356207), with the parts no binary has
//! shown yet modelled around it. The host links it to read and rewrite turns,
//! and a mock client speaks it so rekindle's wire-level techniques can be
//! tested against an oracle.
//!
//! Each modelled item's docs start with "Modelled." and say what a desktop
//! build or a recovered guest module would replace. The crate's `README.md`
//! has the whole table.
//!
//! # Recovered vs modelled
//!
//! Two layers are copied faithfully from the binary and must stay byte-exact:
//!
//! - the **13-tag value codec** ([`Value`], [`decode`]) — fixed-width
//!   little-endian, read from the decode function at VA `0x172bc4c`, which
//!   decodes host-to-guest events with it;
//! - the **855-entry `PropId` table** ([`PropId`]) — the id->name and name->id
//!   tables at VA `0x5d9fcf0` / `0x5da3260`, generated from `propid-names.tsv`.
//!
//! Two layers are rekindle's **model**, consistent with what is recovered but
//! not yet recovered from the host binary, and marked as such at each site:
//!
//! - each `PropId`'s **value type** (which tag it carries) — the id space is
//!   untyped at the wire layer; this crate never asserts a pairing;
//! - the **node / turn framing** above the value layer ([`turn`]) — how a turn
//!   groups node create / remove / move and property-set ops. No guest `.cwasm`
//!   ships in the APK, but build 356207 compiles the guest into the host, so
//!   this framing is recoverable from that binary; until it is read, this is a
//!   plausible stand-in that respects the recovered constraints (string element
//!   kinds, `&[u32]` child-list diff, tag-8 property sets into `view_props`).
//!
//! Motion is the host's: [`motion`] models the native engine that animates
//! a view's channels per frame from the spec its `viewMotion` cell declares.
//!
//! Input crosses the other way: [`event`] models the buffer the host hands
//! the guest through `ldh_dispatch`, whose shape is recovered.
//!
//! Accessibility travels in the same store: [`a11y`] has the recovered
//! accessibility `PropIds` and role vocabulary, and builds the semantic tree a
//! platform would build from a tree's cells.
//!
//! The [`view_props`][ViewProps] store keying (`(node, PropId)`) and the keyed
//! sibling diff ([`diff_children`]) are recovered in *shape*; their in-memory
//! representation and the precise move-minimisation are modelled.

pub mod a11y;
pub mod capture;
mod element;
pub mod event;
pub mod motion;
mod prop_id;
mod props;
mod tree;
pub mod turn;
mod value;

pub use element::ElementKind;
pub use prop_id::{PropId, ids};
pub use props::{PropSet, ViewProps};
pub use tree::{ABSENT, ChildOp, Node, NodeId, diff_children};
pub use turn::{NodeOp, Tree, Turn, TurnReport};
pub use value::{Cursor, DecodeError, Value, decode};
