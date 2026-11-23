//! Raw guest-side bindings for the rekindle plugin contract.
//!
//! Most plugin authors should use `rekindle-sdk`, which wraps these in an
//! ergonomic API. This crate is the unadorned ABI: the WIT package in `wit/`
//! (`rekindle:plugin@0.9.0`) is the source of truth, and the bindings are
//! generated from it at compile time.
//!
//! Generated layout:
//!   - implement the `Guest` trait of every interface under
//!     `exports::rekindle::plugin::{lifecycle, event-handler, ui-handler,
//!     timer-handler, compose-handler, renderer, notification-filter,
//!     event-filter}`
//!   - imports live under `rekindle::plugin::{host, client-state, ui, ...}`
//!   - type-only interfaces under `rekindle::plugin::{types, records,
//!     events, compose-types, command-types, render-types, ...}`
//!   - finish with `rekindle_plugin_api::export!(MyPlugin with_types_in rekindle_plugin_api);`

#[expect(
    clippy::same_length_and_capacity,
    reason = "wit-bindgen rebuilds each list the host passes in from its pointer and length"
)]
mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "plugin",
        pub_export_macro: true,
        default_bindings_module: "rekindle_plugin_api",
        additional_derives: [PartialEq],
    });
}

mod impls;

pub use bindings::*;

/// The WIT package version this crate was generated from.
pub const WIT_VERSION: &str = "0.9.0";

#[cfg(test)]
mod tests {
    #[test]
    fn wit_version_is_the_packages() {
        let types = include_str!("../wit/types.wit");
        assert!(types.contains(&format!("package rekindle:plugin@{};", super::WIT_VERSION)));
    }
}
