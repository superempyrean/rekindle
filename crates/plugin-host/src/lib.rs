//! Host runtime for rekindle plugins.
//!
//! Plugins are WebAssembly components implementing `rekindle:plugin`. The
//! host loads them into wasmtime, grants the capabilities their manifest
//! requests, and checks every host call against them. A plugin has no
//! filesystem, network or clock access beyond what a requested capability
//! exposes.
//!
//! - [`capability`]: capabilities and the three trust levels.
//! - [`manifest`]: the metadata embedded in each component.
//! - [`theme`]: themes without code (a `.theme.json` the host applies), and
//!   [`tokens`], the theme tokens.
//! - [`runtime`]: loading, gating, fault isolation.
//! - [`backend`]: the [`ClientBackend`] seam and [`MockBackend`].
//! - [`api`]: the contract records (state, events, compose, render), shared
//!   with backends; [`model`]: host-side types for UI and theming.
//! - [`settings`]: manifest-declared settings and their values;
//!   [`storage`]: each plugin's key-value storage.
//! - [`net`]: HTTP for `network`, limited to the declared hosts.
//! - [`home`]: installed plugins and developer mode; [`review`]: the install
//!   screen; [`manager`]: rekindle's own settings panel.
//! - [`channel`] and [`updates`]: the signed release channel and updating
//!   from it; [`store`]: the community index as the user browses it;
//!   [`index`]: checking and building plugins for the index.
//! - [`builder`]: `rekindle build` (crate to component), with
//!   [`sourcemap`] for JavaScript stack traces.
//! - [`validate`]: whether an interaction is one a real client could deliver.
//! - [`testing`]: a harness for plugin authors' `cargo test`.

pub mod api;
pub mod backend;
mod bindings;
pub mod builder;
pub mod capability;
pub mod channel;
pub mod home;
pub mod index;
pub mod manager;
pub mod manifest;
pub mod model;
pub mod net;
pub mod review;
pub mod runtime;
pub mod settings;
pub mod sourcemap;
pub mod storage;
pub mod store;
pub mod testing;
pub mod theme;
pub mod tokens;
pub mod updates;
pub mod validate;

/// Keys and signatures (they live in `rekindle-release`, shared with the launcher).
pub use rekindle_release::signing;

pub use backend::{ClientBackend, MockBackend};
pub use capability::{Capability, Grants, Trust, resolve_grants};
pub use manifest::Manifest;
pub use runtime::{Runtime, RuntimeConfig, SharedBackend};
