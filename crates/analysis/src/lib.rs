//! Static binary-triage library for rekindle.
//!
//! Recover, from a compiled Rust release binary and without modifying it, the
//! `rustc` version, the vendored crate versions, and the GPUI stack. The
//! `rekindle-analyze` binary is a thin CLI over this; the [`crate::strings`] and
//! [`crate::sections`] modules are also reused by the anchor resolver
//! (`rekindle-anchor`).

pub mod crates;
pub mod fork_point;
pub mod gpui_stack;
pub mod locations;
pub mod relocs;
pub mod report;
pub mod rustc_id;
pub mod sections;
pub mod strings;
