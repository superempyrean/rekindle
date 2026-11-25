//! Engine work the plugin writes itself (requires `wire-raw`).
//!
//! For what [`elements`](crate::elements) can't express. The work is JSON
//! in the shape of `rekindle-wire-inject`'s `Work`: `mounts`, `cells`,
//! `rows`, `presses`, `labels`, `recolors` and `prop_recolors`, each with a
//! selector over the guest's cells and structure. [`selector`] gives the
//! selector a name has on this build, to start from.
//!
//! ```no_run
//! # use rekindle_sdk::wire_raw;
//! # fn f() -> Result<(), String> {
//! let gift = wire_raw::selector("composer.gift").ok_or("no gift button here")?;
//! wire_raw::set_work(&format!(
//!     r#"{{"rows": [{{"select": {gift}, "action": {{"cells": [{{"prop": "opacity", "value": {{"f64": 0.3}}}}]}}}}]}}"#
//! ))?;
//! # Ok(())
//! # }
//! ```
//!
//! Mounts take node ids from `0xF000_0000` to `0xF000_FFFF`. A press on a
//! node with a `plugin` handler, or one a `plugin` press rule keeps,
//! reaches [`Plugin::on_ui`](crate::Plugin::on_ui) as `UiValue::Clicked`
//! on surface [`SURFACE`], with the handler's element.

use crate::raw::rekindle::plugin::wire_raw as raw;

/// The [`Plugin::on_ui`](crate::Plugin::on_ui) surface of a press the
/// plugin's work keeps.
pub const SURFACE: &str = "wire-raw";

/// Replaces the plugin's work. The error says what the host refused.
pub fn set_work(json: &str) -> Result<(), String> {
    raw::set_work(json)
}

/// Drops the plugin's work.
pub fn clear() {
    // An empty work is never refused.
    let _ = raw::set_work("");
}

/// The selector the element `name` has on this build, as JSON.
pub fn selector(name: &str) -> Option<String> {
    raw::selector(name)
}
