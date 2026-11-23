//! `rekindle-wire`: the wire Phoenix's split GPUI speaks between its guest
//! and its platform, so the mock can speak the real bytes.
//!
//! Phoenix is GPUI cut in two across a wasm boundary. A sandboxed wasm guest's
//! runtime reconciles its views against a retained element tree and hands the
//! platform one op buffer per turn. On the Android build the platform's views
//! apply the ops and paint. Whether a desktop build paints them in the guest's
//! runtime is not known yet. The host links this crate to read and rewrite
//! turns, and the mock client speaks it so rekindle's wire-level techniques
//! can be tested against the real format.
//!
//! What is here was read out of the Android host library
//! (`liblibdiscord_wasm_host.so`, build 356207), which compiles the guest
//! in, and the writer functions were emulated to check the bytes:
//!
//! - the 13-tag value codec ([`Value`], [`decode`]), fixed-width
//!   little-endian, from the decoder at VA `0x172bc4c` and the encoder at
//!   `0x1751a88`. Colours are integers, `0xAARRGGBB` ([`Value::color`]);
//! - the 855-entry `PropId` table ([`PropId`]), from the tables at VA
//!   `0x5d9fcf0` and `0x5da3260`, generated from `propid-names.tsv`;
//! - the turn ([`turn`]): a version byte and ten ops, with kinds as strings
//!   ([`ElementKind`]);
//! - the UI events the platform sends back ([`event`]) and the host frames
//!   a guest's calls and a host module's events travel in ([`frame`]).
//!
//! The parts no binary has shown yet are modelled around those, and each
//! one's docs start with "Modelled." and say what would replace it: how
//! `view_props` stores a cell, a motion spec's contents, how a platform
//! builds an accessibility tree, and an event's reply. The crate's
//! `README.md` has the whole table.
//!
//! Motion is the platform's: [`motion`] models the native engine that
//! animates a view's channels per frame from the spec its `viewMotion`
//! cell declares. Accessibility travels in the same store: [`a11y`] has the
//! recovered accessibility `PropIds` and role vocabulary, and builds the
//! semantic tree a platform would build from a tree's cells.

pub mod a11y;
pub mod capture;
mod element;
pub mod event;
pub mod frame;
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
pub use turn::{ChildEdit, NodeOp, Tree, Turn, TurnReport};
pub use value::{Cursor, DecodeError, Value, decode};
