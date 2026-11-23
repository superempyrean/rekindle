//! wasmtime bindings for `rekindle:plugin@0.9.0`, generated from the WIT in
//! `crates/plugin-api/wit`. Crate-private; backends see the shared records
//! through [`crate::api`].

wasmtime::component::bindgen!({
    path: "../plugin-api/wit",
    world: "plugin",
    imports: { default: trappable },
    additional_derives: [serde::Serialize, serde::Deserialize, PartialEq],
});

pub(crate) use exports::rekindle::plugin as ex;
pub(crate) use rekindle::plugin as wit;
