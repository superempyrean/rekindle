//! Code-signature matching library for rekindle.
//!
//! Compile a candidate crate, reduce each function to a relocation-masked byte
//! signature, and locate it in a target binary. It is all static analysis of code we
//! compiled plus a file we read, never modifying a binary. The
//! `rekindle-sigmatch` binary is a thin CLI over this; the [`sig`] and [`scan`]
//! modules are also reused by the anchor resolver (`rekindle-anchor`) to resolve
//! byte-signature anchors.

pub mod compile;
pub mod extract;
pub mod scan;
pub mod short;
pub mod sig;
pub mod sigfile;
pub mod snapshots;
