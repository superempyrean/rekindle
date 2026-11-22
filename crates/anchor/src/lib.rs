//! Durable *anchors* for integration points, and a resolver that re-locates
//! them in each build.
//!
//! A release binary moves code and data between builds, so rekindle never
//! hardcodes addresses. Each integration point (a function to hook, a field
//! offset) is described by one or more **locators** that survive rebuilds: a
//! string the point loads, a relocation-masked byte signature, an
//! instruction shape, a panic location, a place in the call graph. This
//! crate resolves a set of anchors against a binary and, given two builds,
//! reports which anchors **broke**.
//!
//! It reads files and reports locations. It never modifies a binary.
//!
//! - [`descriptor`]: anchors and locators, as `anchors.json` holds them.
//! - [`resolve`]: an anchor set resolved against a binary, with a verdict
//!   per anchor.
//! - [`suggest`]: locators built from a binary, checked before they're
//!   returned. [`inspect`] reads what they're built from off a function.
//! - [`heal`]: new locators for the anchors a build weakened.
//! - [`diff`]: what changed between two resolutions.

pub mod descriptor;
pub mod diff;
pub mod heal;
mod index;
pub mod inspect;
pub mod resolve;
mod shape;
pub mod suggest;

#[cfg(test)]
mod fixtures;

pub use descriptor::{Anchor, AnchorSet, Locator, Operand, file_key};
pub use diff::{AnchorChange, Change, DiffReport, diff};
pub use heal::{Healing, heal};
pub use inspect::{InstructionOperands, callers, calls, find_value, operands};
pub use resolve::{AnchorResult, LocatorResult, ResolutionReport, Status, Summary, Verdict};
pub use suggest::{
    suggest, suggest_call, suggest_operand, suggest_operand_by_value, suggest_shape,
};
