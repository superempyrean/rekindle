//! What a plugin file is, found without running it: its manifest, the
//! capabilities its imports need and the hooks it exports. Also the
//! process's wasmtime engine, which compiles it.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use wasmtime::Engine;
use wasmtime::component::Component;

use super::compiled::CompileCache;
use crate::capability::{ALWAYS_IMPORTABLE, Capability};
use crate::manifest::{self, Manifest};
use crate::theme;

/// The hook exports (design doc §3.2).
pub const HOOKS: [&str; 9] = [
    "lifecycle",
    "event-handler",
    "ui-handler",
    "timer-handler",
    "compose-handler",
    "renderer",
    "notification-filter",
    "event-filter",
    "http-filter",
];

/// The capability a hook export needs before the host calls it.
pub fn hook_capability(hook: &str) -> Option<Capability> {
    match hook {
        "compose-handler" => Some(Capability::Compose),
        "renderer" => Some(Capability::RenderMessages),
        "notification-filter" => Some(Capability::Notifications),
        "event-filter" => Some(Capability::Intercept),
        "http-filter" => Some(Capability::InterceptHttp),
        _ => None,
    }
}

/// What static inspection of a component reveals, without running it.
#[derive(Debug, Clone, Serialize)]
pub struct Inspection {
    pub manifest: Manifest,
    /// Capabilities implied by the component's imports.
    pub imported: BTreeSet<Capability>,
    /// Raw import names, for display.
    pub imports: Vec<String>,
    /// Hook interfaces the component exports (`event-handler`, `renderer`,
    /// ...), whether or not their capability is requested.
    pub hooks: Vec<String>,
}

impl Inspection {
    /// Hooks the host will actually call, given the manifest's requests.
    pub fn effective_hooks(&self) -> Vec<&str> {
        self.hooks
            .iter()
            .map(String::as_str)
            .filter(|h| hook_capability(h).is_none_or(|c| self.manifest.requests(c)))
            .collect()
    }
}

/// The process's wasmtime engine, shared by every runtime: a compiled
/// component belongs to the engine that compiled it, so one engine lets any
/// runtime use what another compiled. Fails if the engine can't be built or
/// the thread that advances its epoch can't start.
pub fn engine() -> Result<Engine> {
    static ENGINE: std::sync::Mutex<Option<Engine>> = std::sync::Mutex::new(None);
    let mut slot = ENGINE.lock().unwrap();
    if let Some(e) = &*slot {
        return Ok(e.clone());
    }
    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    config.consume_fuel(true);
    // Running wasm checks the engine's epoch, which a thread advances every
    // EPOCH_TICK, so a call's wall-clock limit is enforced to about a tick.
    config.epoch_interruption(true);
    // Android and iOS run plugins on Pulley, wasmtime's interpreter. Its
    // code is bytecode in data pages, so the app never maps memory
    // executable, which SELinux may refuse an app and iOS refuses any app
    // without a JIT entitlement. Bounds are checked in the interpreter, so
    // no signal handler of wasmtime's sits beside the runtime's own.
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        config.target("pulley64")?;
        config.signals_based_traps(false);
    }
    // An iOS app's address space is a fraction of Android's, and the
    // default 4 GiB reserved for each plugin's memory runs out of it after a
    // few plugins. The interpreter checks bounds itself, so a memory needs
    // no reservation or guard pages.
    #[cfg(target_os = "ios")]
    {
        config.memory_reservation(0);
        config.memory_guard_size(0);
        config.memory_reservation_for_growth(1 << 20);
    }
    let engine = Engine::new(&config)?;
    let ticker = engine.clone();
    std::thread::Builder::new()
        .name("rekindle-epoch".into())
        .spawn(move || {
            loop {
                std::thread::sleep(EPOCH_TICK);
                ticker.increment_epoch();
            }
        })
        .context("starting the epoch thread")?;
    *slot = Some(engine.clone());
    Ok(engine)
}

const EPOCH_TICK: std::time::Duration = std::time::Duration::from_millis(5);

/// Inspect a component's manifest, imports and hook exports without
/// instantiating it.
pub fn inspect(engine: &Engine, bytes: &[u8]) -> Result<(Inspection, Component)> {
    inspect_cached(engine, None, bytes)
}

/// [`inspect`], compiling through `cache` when there is one.
pub fn inspect_cached(
    engine: &Engine,
    cache: Option<&CompileCache>,
    bytes: &[u8],
) -> Result<(Inspection, Component)> {
    if theme::is_theme(bytes) {
        bail!("this is a theme file, not a component (use `inspect_artifact`)");
    }
    let manifest = manifest::extract(bytes)?
        .context("no rekindle:manifest section (build it with `rekindle build`)")?;
    manifest.validate()?;
    let component = match cache {
        Some(c) => c.component(engine, bytes),
        None => Component::new(engine, bytes).map_err(anyhow::Error::from),
    }
    .context("not a valid component")?;
    let ty = component.component_type();
    let mut imported = BTreeSet::new();
    let mut imports = Vec::new();
    for (name, _) in ty.imports(engine) {
        imports.push(name.to_string());
        let iface = interface_of(name)?;
        if ALWAYS_IMPORTABLE.contains(&iface) {
            continue;
        }
        match Capability::for_interface(iface) {
            Some(cap) => {
                imported.insert(cap);
            }
            None => bail!("component imports unknown interface `{name}`"),
        }
    }
    let mut hooks = Vec::new();
    for (name, _) in ty.exports(engine) {
        let Ok(iface) = interface_of(name) else {
            continue;
        };
        if HOOKS.contains(&iface) {
            hooks.push(iface.to_string());
        }
    }
    Ok((
        Inspection {
            manifest,
            imported,
            imports,
            hooks,
        },
        component,
    ))
}

/// [`inspect`] for anything that installs: a component, or a theme file
/// (which imports nothing but `theming` and exports no hooks).
pub fn inspect_artifact(engine: &Engine, bytes: &[u8]) -> Result<Inspection> {
    inspect_artifact_cached(engine, None, bytes)
}

/// [`inspect_artifact`], compiling through `cache` when there is one.
pub fn inspect_artifact_cached(
    engine: &Engine,
    cache: Option<&CompileCache>,
    bytes: &[u8],
) -> Result<Inspection> {
    if theme::is_theme(bytes) {
        let manifest = theme::parse(bytes)?.manifest();
        return Ok(Inspection {
            manifest,
            imported: BTreeSet::from([Capability::Theming]),
            imports: Vec::new(),
            hooks: Vec::new(),
        });
    }
    inspect_cached(engine, cache, bytes).map(|(insp, _)| insp)
}

fn interface_of(name: &str) -> Result<&str> {
    let (pkg_iface, version) = name.split_once('@').unwrap_or((name, ""));
    let Some(iface) = pkg_iface.strip_prefix("rekindle:plugin/") else {
        bail!(
            "component imports `{name}`, which this host does not provide (plugins get no WASI or other ambient imports)"
        );
    };
    if version != manifest::rekindle_wit_version() {
        bail!(
            "component was built against rekindle:plugin@{version}; this host implements @{}. Rebuild it with this \
             rekindle (`rekindle build`)",
            manifest::rekindle_wit_version()
        );
    }
    Ok(iface)
}
