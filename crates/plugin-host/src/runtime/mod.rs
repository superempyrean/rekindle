//! The plugin runtime: loads components, enforces grants, and mediates every
//! host call.
//!
//! Enforcement happens twice:
//! 1. **At load.** The component's imports are mapped to capabilities. If it
//!    imports an interface whose capability its manifest doesn't request, it
//!    is refused with a clear error and none of its code runs. Imports
//!    outside `rekindle:plugin` (e.g. WASI) are refused outright.
//! 2. **At call time.** Every host function re-checks the grant and traps on a
//!    violation. That can't happen for components that passed (1); it's a
//!    backstop.
//!
//! Exports are split per hook (design doc §3.2): only `lifecycle` is
//! required, and the host looks the others up by name, so a plugin that
//! doesn't handle events is never called with them. Hooks whose capability
//! wasn't granted (`compose-handler` without `compose`, `renderer` without
//! `render-messages`, `notification-filter` without `notifications`) are
//! never called either.
//!
//! Each plugin gets its own wasmtime `Store`, with fuel (CPU) and memory caps.
//! A plugin that traps is marked faulted and its contributions are removed;
//! other plugins keep running. Plugins with a `renderer` or
//! `notification-filter` get a second instance, the render isolate (design
//! doc §6), created on first use.
//!
//! A theme file ([`crate::theme`]) loads through the same calls but isn't a
//! component: the runtime applies its values to the backend under its id
//! (re-applying when its variant setting or the client's features change)
//! and lists it with the plugins. Nothing of it runs.
//!
//! [`Runtime`]'s methods are spread over the submodules by what they do:
//! `inspect` and `load` for files, `events`, `gestures`,
//! `compose` and `isolate` for the hooks, `call` for calling into a plugin
//! and its faults, `stores` for storage and settings, `themes` for theme
//! files. The `host_*` modules implement the host functions plugins import.

mod call;
pub mod compiled;
mod compose;
mod convert;
mod events;
mod gestures;
mod host_actions;
mod host_core;
mod host_services;
mod host_state;
mod host_ui;
mod http;
mod inspect;
mod isolate;
mod load;
mod logs;
mod render;
mod state;
mod stores;
mod themes;
mod timefmt;

pub use events::event_kind;
pub use gestures::message_predicate;
pub use host_state::redact;
pub use http::{CALLS_PER_MINUTE, MAX_BODY, RequestOutcome, RestRequest, route_kind};
pub use inspect::{
    HOOKS, Inspection, engine, hook_capability, inspect, inspect_artifact, inspect_artifact_cached,
    inspect_cached,
};
pub use isolate::Rendered;
pub use load::LoadTimes;
pub use logs::Logs;
pub use stores::{SettingEntry, SettingsPage};

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use serde::Serialize;
use wasmtime::component::{InstancePre, Linker};
use wasmtime::{Engine, Store};

use crate::api;
use crate::backend::ClientBackend;
use crate::bindings::ex;
use crate::capability::{Capability, Grants, Trust};
use crate::manifest::Manifest;
use crate::net::{self, NetClient, NetLimits};
use crate::settings::SettingsStore;
use crate::storage::{PluginStorage, StorageQuota};
use crate::theme::{self, Theme};

use state::PluginState;

/// The client, shared by the runtime and every plugin's host functions.
pub type SharedBackend = Arc<Mutex<dyn ClientBackend>>;

/// Where plugin storage lives.
#[derive(Debug, Clone)]
pub enum StorageLocation {
    /// Lost when the runtime is dropped (survives reloads within it).
    Memory,
    /// One JSON file per plugin id (and account) in this directory, plus
    /// `<id>.settings.json` for schema settings.
    Dir(PathBuf),
}

/// What each plugin may use. The defaults are what rekindle runs with.
#[derive(Debug, Clone)]
pub struct Limits {
    /// Wasm fuel per host→plugin call (roughly, instructions).
    pub fuel_per_call: u64,
    /// Fuel per message in an `on-render` batch, and per `on-autocomplete`
    /// / `on-notification` call (design doc §3.5: a few ms).
    pub fuel_per_render: u64,
    /// Wall-clock time per `on-events` call, less time waiting on HTTP. A
    /// plugin that takes longer is stopped and turned off, so a slow one
    /// can't hold up the others (design doc §3.5).
    pub event_time: std::time::Duration,
    /// Max linear memory per plugin instance, in bytes.
    pub memory_bytes: usize,
    /// The plugin's `storage`, per account.
    pub storage: StorageQuota,
    /// Each `http.fetch`.
    pub net: NetLimits,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            fuel_per_call: 2_000_000_000,
            fuel_per_render: 50_000_000,
            event_time: std::time::Duration::from_millis(50),
            memory_bytes: 64 * 1024 * 1024,
            storage: StorageQuota::default(),
            net: NetLimits::default(),
        }
    }
}

/// How a [`Runtime`] keeps plugin data and reaches the network.
pub struct RuntimeConfig {
    pub storage: StorageLocation,
    /// The transport for `http.fetch`, called after the request passed
    /// [`net::check_request`].
    pub net: Arc<dyn NetClient>,
    pub limits: Limits,
    /// Max log records retained in memory.
    pub log_capacity: usize,
    /// Where compiled plugins are kept between launches
    /// ([`compiled::CompileCache`]), so a plugin is compiled once per version
    /// instead of at every load. `None` compiles every time. The host runs
    /// what it finds there as machine code, so the directory must be as
    /// trusted as the host's own files: rekindle's home
    /// (`Home::runtime_config`), never a shared one.
    pub compile_cache: Option<PathBuf>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            storage: StorageLocation::Memory,
            net: Arc::new(net::UreqClient),
            limits: Limits::default(),
            log_capacity: 2000,
            compile_cache: None,
        }
    }
}

/// Whether a loaded plugin runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "reason", rename_all = "kebab-case")]
pub enum Status {
    Active,
    /// It trapped, and the host stopped calling it. The reason is for the
    /// user: a panic message, "exceeded its CPU budget" and the like.
    Faulted(String),
}

/// A loaded plugin or theme, as the client lists it.
#[derive(Debug, Clone, Serialize)]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub trust: Trust,
    pub granted: BTreeSet<Capability>,
    pub network_allowlist: BTreeSet<String>,
    pub status: Status,
    /// Hooks the host calls for this plugin.
    pub hooks: Vec<String>,
    /// Why the render isolate was switched off, if it was.
    pub renderer_disabled: Option<String>,
    /// Slots the plugin added to that this client build can't show: those
    /// features of it are missing here.
    pub missing_slots: Vec<String>,
}

/// The loaded hook exports of one instance.
#[derive(Default)]
struct Hooks {
    lifecycle: Option<ex::lifecycle::Guest>,
    events: Option<ex::event_handler::Guest>,
    ui: Option<ex::ui_handler::Guest>,
    timer: Option<ex::timer_handler::Guest>,
    compose: Option<ex::compose_handler::Guest>,
    renderer: Option<ex::renderer::Guest>,
    notifications: Option<ex::notification_filter::Guest>,
    filter: Option<ex::event_filter::Guest>,
    http: Option<ex::http_filter::Guest>,
}

/// A plugin's render isolate ([`isolate`]).
struct Isolate {
    store: Store<PluginState>,
    hooks: Hooks,
}

/// (message id, edit time) -> transforms.
type RenderCache = HashMap<(u64, Option<u64>), Vec<api::Transform>>;

/// A plugin loaded from a component.
struct Loaded {
    manifest: Arc<Manifest>,
    trust: Trust,
    grants: Grants,
    /// Linked, for the render isolate and for starting it again after a
    /// failed reload.
    pre: InstancePre<PluginState>,
    /// The main instance.
    store: Store<PluginState>,
    hooks: Hooks,
    /// The hooks the host calls: exported, and their capability granted.
    hook_names: Vec<String>,
    status: Status,
    isolate: Option<Isolate>,
    /// Traps in the render isolate since the plugin loaded.
    isolate_failures: u32,
    renderer_disabled: Option<String>,
    /// Emptied by `render-control.invalidate` and settings changes.
    render_cache: RenderCache,
    /// message id -> channel, for per-channel invalidation.
    render_channels: HashMap<u64, u64>,
}

impl Loaded {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: self.manifest.id.clone(),
            name: self.manifest.name.clone(),
            version: self.manifest.version.clone(),
            trust: self.trust,
            granted: self.grants.granted.clone(),
            network_allowlist: self.grants.network_allowlist.clone(),
            status: self.status.clone(),
            hooks: self.hook_names.clone(),
            renderer_disabled: self.renderer_disabled.clone(),
            missing_slots: self.store.data().missing_slots.iter().cloned().collect(),
        }
    }
}

/// A theme file being shown.
struct LoadedTheme {
    manifest: Arc<Manifest>,
    theme: Arc<Theme>,
    trust: Trust,
    /// What it has set in the backend, so a re-apply undoes what it drops.
    applied: theme::Applied,
}

impl LoadedTheme {
    fn info(&self) -> PluginInfo {
        PluginInfo {
            id: self.manifest.id.clone(),
            name: self.manifest.name.clone(),
            version: self.manifest.version.clone(),
            trust: self.trust,
            granted: BTreeSet::from([Capability::Theming]),
            network_allowlist: BTreeSet::new(),
            status: Status::Active,
            hooks: Vec::new(),
            renderer_disabled: None,
            missing_slots: Vec::new(),
        }
    }
}

/// The plugin host: the loaded plugins and themes, and everything the host
/// keeps for them. The client drives it, one call per event, gesture or
/// paint.
pub struct Runtime {
    /// The phases of the last component load.
    pub last_load: LoadTimes,
    compiled: Option<compiled::CompileCache>,
    engine: Engine,
    linker: Linker<PluginState>,
    backend: SharedBackend,
    config: RuntimeConfig,
    logs: Logs,
    start: Instant,
    /// Storage by namespace (`id`, or `id@account`).
    storages: HashMap<String, Arc<Mutex<PluginStorage>>>,
    settings: HashMap<String, Arc<Mutex<SettingsStore>>>,
    /// In the order they were enabled, which is also the order hooks are
    /// called in and click bindings are tried in.
    plugins: Vec<Loaded>,
    themes: Vec<LoadedTheme>,
    /// The logged-in account (storage namespaces are per account).
    account: Option<u64>,
}

impl Runtime {
    /// A runtime with nothing loaded, on the process's engine ([`engine`]),
    /// with every host function linked.
    pub fn new(backend: SharedBackend, config: RuntimeConfig) -> Result<Self> {
        let engine = engine()?;
        let compiled = config
            .compile_cache
            .clone()
            .map(compiled::CompileCache::new);
        let mut linker = Linker::new(&engine);
        crate::bindings::Plugin::add_to_linker::<
            PluginState,
            wasmtime::component::HasSelf<PluginState>,
        >(&mut linker, |s| s)?;
        let logs = Logs::new(config.log_capacity);
        Ok(Self {
            engine,
            linker,
            backend,
            config,
            logs,
            start: Instant::now(),
            storages: HashMap::new(),
            settings: HashMap::new(),
            plugins: Vec::new(),
            themes: Vec::new(),
            account: None,
            last_load: LoadTimes::default(),
            compiled,
        })
    }

    /// The wasmtime engine plugins are compiled for.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// The client the runtime drives.
    pub fn backend(&self) -> &SharedBackend {
        &self.backend
    }

    /// Every plugin's log.
    pub fn logs(&self) -> &Logs {
        &self.logs
    }

    /// Loaded plugins, then loaded themes.
    pub fn plugins(&self) -> Vec<PluginInfo> {
        self.plugins
            .iter()
            .map(Loaded::info)
            .chain(self.themes.iter().map(LoadedTheme::info))
            .collect()
    }

    /// The loaded plugin or theme `id`.
    pub fn plugin(&self, id: &str) -> Option<PluginInfo> {
        self.find(id)
            .map(|i| self.plugins[i].info())
            .or_else(|| self.find_theme(id).map(|i| self.themes[i].info()))
    }

    /// The manifest of the loaded plugin or theme `id`.
    pub fn manifest(&self, id: &str) -> Option<&Manifest> {
        self.find(id)
            .map(|i| &*self.plugins[i].manifest)
            .or_else(|| self.find_theme(id).map(|i| &*self.themes[i].manifest))
    }

    fn find(&self, id: &str) -> Option<usize> {
        self.plugins.iter().position(|p| p.manifest.id == id)
    }

    fn find_theme(&self, id: &str) -> Option<usize> {
        self.themes.iter().position(|t| t.manifest.id == id)
    }

    fn is_loaded(&self, id: &str) -> bool {
        self.find(id).is_some() || self.find_theme(id).is_some()
    }

    /// Everything that resolved on this build, as plugins see it through
    /// `host.features()`.
    pub fn features(&self) -> Vec<String> {
        features_of(&*self.backend.lock().unwrap())
    }

    /// The client build changed what resolved (a live update, a re-anchor):
    /// tell every plugin, and re-apply every theme (a token or surface it
    /// couldn't set may exist now).
    pub fn features_changed(&mut self) {
        for idx in 0..self.plugins.len() {
            self.call(idx, |h, store| match &h.lifecycle {
                Some(l) => l.call_on_features_changed(store),
                None => Ok(()),
            });
        }
        for idx in 0..self.themes.len() {
            self.apply_theme(idx);
        }
    }
}

/// `host.features()`: slots, decoration kinds, `surface:`-prefixed surfaces,
/// `tweak:`-prefixed tweak ids and the backend's extra features.
fn features_of(b: &dyn ClientBackend) -> Vec<String> {
    let mut f: BTreeSet<String> = BTreeSet::new();
    f.extend(b.available_slots());
    f.extend(b.available_decorations());
    f.extend(b.surfaces().into_iter().map(|s| format!("surface:{s}")));
    f.extend(
        b.tweaks()
            .into_iter()
            .filter(|t| t.available)
            .map(|t| format!("tweak:{}", t.id)),
    );
    f.extend(b.extra_features());
    f.into_iter().collect()
}
