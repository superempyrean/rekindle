//! Loading, reloading and unloading plugins: checking a component's imports
//! against its manifest, linking it, instantiating it within its limits and
//! calling its `activate`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use wasmtime::component::{Component, InstancePre};
use wasmtime::{Engine, Store, StoreLimitsBuilder};

use super::inspect::{Inspection, hook_capability, inspect_cached};
use super::state::{PluginState, Rates, Role, Shared};
use super::{Hooks, Loaded, LoadedTheme, Runtime, Status, convert, theme};
use crate::bindings::{ex, wit};
use crate::capability::{Capability, Grants, Trust, resolve_grants};
use crate::manifest::Manifest;
use crate::model;

/// Where the last [`Runtime::load`] of a component spent its time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LoadTimes {
    /// Reading the manifest, compiling (or fetching from the cache) and
    /// checking the imports.
    pub compile: std::time::Duration,
    /// Linking against the host (`instantiate_pre`).
    pub link: std::time::Duration,
    pub instantiate: std::time::Duration,
    /// The plugin's own `activate`.
    pub activate: std::time::Duration,
}

/// A plugin checked and linked, none of its code run yet.
struct Prepared {
    manifest: Arc<Manifest>,
    grants: Grants,
    hook_names: Vec<String>,
    pre: InstancePre<PluginState>,
}

/// A new store whose wasm stops when a call's wall-clock limit
/// ([`state::WallClock`]) runs out. Calls without one aren't interrupted.
pub(super) fn new_store(engine: &Engine, state: PluginState) -> Store<PluginState> {
    let mut store = Store::new(engine, state);
    store.set_epoch_deadline(1);
    store.epoch_deadline_callback(|s| {
        Ok(if s.data().wall.is_some_and(|w| w.over()) {
            wasmtime::UpdateDeadline::Interrupt
        } else {
            wasmtime::UpdateDeadline::Continue(1)
        })
    });
    store
}

impl Runtime {
    /// Load and activate a plugin. It holds what its manifest requests;
    /// `trust` is recorded for [`PluginInfo`] (whether it may load at all is
    /// the caller's decision: see `home`). A theme file is applied instead.
    /// Returns its id.
    pub fn load(&mut self, bytes: &[u8], trust: Trust) -> Result<String> {
        if theme::is_theme(bytes) {
            let t = theme::parse(bytes)?;
            if self.is_loaded(&t.id) {
                bail!("plugin `{}` is already loaded", t.id);
            }
            let manifest = Arc::new(t.manifest());
            self.settings_store(&manifest);
            return Ok(self.push_theme(manifest, t, trust));
        }
        let t = Instant::now();
        let (insp, component) = inspect_cached(&self.engine, self.compiled.as_ref(), bytes)?;
        self.last_load = LoadTimes {
            compile: t.elapsed(),
            ..Default::default()
        };
        self.load_inspected(&insp, &component, trust)
    }

    /// [`Self::load`] for several plugins, in order: the components are
    /// compiled (or fetched from the compile cache) on threads first, then
    /// linked and activated one by one, so they start in the order given and
    /// each result is what `load` would have returned.
    pub fn load_many(&mut self, plugins: &[(&[u8], Trust)]) -> Vec<Result<String>> {
        type Inspected = Result<(Inspection, Component)>;
        let (engine, cache) = (&self.engine, self.compiled.as_ref());
        let threads = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
        let mut inspected: Vec<Option<Inspected>> = (0..plugins.len()).map(|_| None).collect();
        let todo: Vec<usize> = (0..plugins.len())
            .filter(|&i| !theme::is_theme(plugins[i].0))
            .collect();
        for batch in todo.chunks(threads) {
            let done: Vec<(usize, Inspected)> = std::thread::scope(|s| {
                let handles: Vec<_> = batch
                    .iter()
                    .map(|&i| {
                        (
                            i,
                            s.spawn(move || inspect_cached(engine, cache, plugins[i].0)),
                        )
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|(i, h)| {
                        (
                            i,
                            h.join()
                                .unwrap_or_else(|_| Err(anyhow!("compiling panicked"))),
                        )
                    })
                    .collect()
            });
            for (i, r) in done {
                inspected[i] = Some(r);
            }
        }
        plugins
            .iter()
            .zip(inspected)
            .map(|(&(bytes, trust), pre)| match pre {
                None => self.load(bytes, trust),
                Some(r) => {
                    let (insp, component) = r?;
                    self.last_load = LoadTimes::default();
                    self.load_inspected(&insp, &component, trust)
                }
            })
            .collect()
    }

    fn load_inspected(
        &mut self,
        insp: &Inspection,
        component: &Component,
        trust: Trust,
    ) -> Result<String> {
        let t = Instant::now();
        let prepared = self.prepare(insp, component)?;
        self.last_load.link = t.elapsed();
        if self.is_loaded(&prepared.manifest.id) {
            bail!("plugin `{}` is already loaded", prepared.manifest.id);
        }
        self.activate(prepared, trust)
    }

    /// Everything about loading a plugin that runs none of its code: the
    /// manifest/import check and linking. A plugin that fails here never
    /// disturbs what is running.
    fn prepare(&self, insp: &Inspection, component: &Component) -> Result<Prepared> {
        let m = Arc::new(insp.manifest.clone());
        if !insp.hooks.iter().any(|h| h == "lifecycle") {
            bail!(
                "plugin `{}` doesn't export `rekindle:plugin/lifecycle` (built against an older rekindle?)",
                m.id
            );
        }
        for cap in &insp.imported {
            if !m.requests(*cap) {
                bail!(
                    "plugin `{}` uses the `{cap}` interface but doesn't request the `{cap}` capability in its manifest",
                    m.id
                );
            }
        }
        let grants = resolve_grants(&m.capabilities, &m.network);
        let hook_names: Vec<String> = insp
            .hooks
            .iter()
            .filter(|h| hook_capability(h).is_none_or(|c| grants.has(c)))
            .cloned()
            .collect();

        let pre = self
            .linker
            .instantiate_pre(component)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("linking `{}`", m.id))?;
        Ok(Prepared {
            manifest: m,
            grants,
            hook_names,
            pre,
        })
    }

    pub(super) fn new_state(
        &mut self,
        m: &Arc<Manifest>,
        grants: &Grants,
        role: Role,
        shared: Arc<Mutex<Shared>>,
    ) -> Result<PluginState> {
        let storage = self.storage(&m.id)?;
        let settings = self.settings_store(m);
        let locale = self.backend.lock().unwrap().client_info().locale;
        let ledger = self.ledger_of(&m.id);
        Ok(PluginState {
            id: m.id.clone(),
            role,
            grants: grants.clone(),
            manifest: m.clone(),
            backend: self.backend.clone(),
            storage,
            settings,
            shared,
            net: self.config.net.clone(),
            net_limits: self.config.limits.net,
            logs: self.logs.clone(),
            start: self.start,
            // Instances are fixed by the component itself (they can't grow at
            // runtime), so the cap only has to leave room for the largest.
            limits: StoreLimitsBuilder::new()
                .memory_size(self.config.limits.memory_bytes)
                .instances(64)
                .memories(4)
                .build(),
            slot_items: BTreeSet::new(),
            timers: BTreeMap::new(),
            overlay_images: BTreeSet::new(),
            overlay_sprites: BTreeSet::new(),
            gesture: None,
            subscriptions: None,
            intercepts: BTreeSet::new(),
            http_intercepts: BTreeSet::new(),
            decorations: BTreeSet::new(),
            accessories: BTreeSet::new(),
            ghosts: BTreeMap::new(),
            commands: BTreeSet::new(),
            keybinds: BTreeSet::new(),
            slash_commands: BTreeSet::new(),
            click_bindings: BTreeMap::new(),
            sounds: BTreeSet::new(),
            rates: Rates::default(),
            view: crate::model::ViewOverrides::default(),
            pending_settings: Vec::new(),
            pending_invalidate: Vec::new(),
            locale,
            wall: None,
            missing_slots: BTreeSet::new(),
            composing: None,
            ledger,
        })
    }

    pub(super) fn instantiate(
        &self,
        pre: &InstancePre<PluginState>,
        store: &mut Store<PluginState>,
        grants: &Grants,
        role: Role,
    ) -> Result<Hooks> {
        store.limiter(|s| &mut s.limits);
        store.set_fuel(self.config.limits.fuel_per_call)?;
        let instance = pre.instantiate(&mut *store)?;
        let mut hooks = Hooks::default();
        macro_rules! load {
            ($field:ident, $module:ident, $cap:expr) => {
                if $cap.map_or(true, |c: Capability| grants.has(c)) {
                    if let Ok(ix) = ex::$module::GuestIndices::new(pre) {
                        hooks.$field = Some(ix.load(&mut *store, &instance)?);
                    }
                }
            };
        }
        match role {
            Role::Main => {
                load!(lifecycle, lifecycle, None);
                load!(events, event_handler, None);
                load!(ui, ui_handler, None);
                load!(timer, timer_handler, None);
                load!(compose, compose_handler, Some(Capability::Compose));
                load!(filter, event_filter, Some(Capability::Intercept));
                load!(http, http_filter, Some(Capability::InterceptHttp));
            }
            Role::Render => {
                load!(renderer, renderer, Some(Capability::RenderMessages));
                load!(
                    notifications,
                    notification_filter,
                    Some(Capability::Notifications)
                );
            }
        }
        Ok(hooks)
    }

    /// Instantiate and activate a prepared plugin.
    fn activate(&mut self, prepared: Prepared, trust: Trust) -> Result<String> {
        let Prepared {
            manifest: m,
            grants,
            hook_names,
            pre,
        } = prepared;
        let t = Instant::now();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let state = self.new_state(&m, &grants, Role::Main, shared)?;
        let mut store = new_store(&self.engine, state);
        let hooks = self
            .instantiate(&pre, &mut store, &grants, Role::Main)
            .with_context(|| format!("instantiating `{}`", m.id))?;
        self.last_load.instantiate = t.elapsed();
        let t = Instant::now();

        let granted: Vec<wit::types::Capability> = grants
            .granted
            .iter()
            .map(|c| convert::cap_to_wit(*c))
            .collect();
        let id = m.id.clone();
        self.plugins.push(Loaded {
            manifest: m,
            trust,
            grants,
            pre,
            store,
            hooks,
            hook_names,
            status: Status::Active,
            isolate: None,
            isolate_failures: 0,
            renderer_disabled: None,
            render_cache: HashMap::new(),
            render_channels: HashMap::new(),
        });
        let idx = self.plugins.len() - 1;

        let r = self.call(idx, |h, store| match &h.lifecycle {
            Some(l) => l.call_activate(store, &granted),
            None => Ok(Ok(())),
        });
        self.last_load.activate = t.elapsed();
        match r {
            Some(Ok(())) => {
                self.host_log(&id, model::LogLevel::Debug, "activated");
                Ok(id)
            }
            Some(Err(msg)) => {
                self.remove_at(idx);
                bail!("plugin `{id}` failed to activate: {msg}")
            }
            None => {
                let Status::Faulted(why) = self.plugins[idx].status.clone() else {
                    unreachable!()
                };
                self.remove_at(idx);
                bail!("plugin `{id}` trapped during activate: {why}")
            }
        }
    }

    /// Deactivate and remove a plugin (or stop showing a theme). Its storage
    /// and settings are kept.
    pub fn unload(&mut self, id: &str) -> Result<()> {
        if let Some(idx) = self.find_theme(id) {
            self.remove_theme_at(idx);
            return Ok(());
        }
        let idx = self
            .find(id)
            .with_context(|| format!("plugin `{id}` is not loaded"))?;
        self.deactivate_and_remove(idx);
        Ok(())
    }

    pub(super) fn deactivate_and_remove(&mut self, idx: usize) -> Loaded {
        if self.plugins[idx].status == Status::Active {
            self.call(idx, |h, store| match &h.lifecycle {
                Some(l) => l.call_deactivate(store),
                None => Ok(()),
            });
        }
        self.remove_at(idx)
    }

    /// Replace a loaded plugin with new bytes (or load them, if it isn't
    /// loaded). The new bytes are checked and linked before the running
    /// version is touched; if they then fail to activate, the previous
    /// version is started again. Either way a failure is an error, which
    /// says whether the previous version is running.
    pub fn reload(&mut self, bytes: &[u8], trust: Trust) -> Result<String> {
        if theme::is_theme(bytes) {
            return self.reload_theme(bytes, trust);
        }
        let (insp, component) = inspect_cached(&self.engine, self.compiled.as_ref(), bytes)?;
        let new = self.prepare(&insp, &component)?;
        // Starting the new version applies its schema, which drops the values
        // of settings it no longer has. They come back if it doesn't start.
        let saved = self
            .settings
            .get(&new.manifest.id)
            .map(|s| s.lock().unwrap().values());
        // A theme with this id gives way to the component, and comes back if
        // the component doesn't start.
        if let Some(t) = self
            .find_theme(&new.manifest.id)
            .map(|i| self.remove_theme_at(i))
        {
            let id = new.manifest.id.clone();
            return self.activate(new, trust).map_err(|err| {
                // The attempt replaced its settings schema with the component's.
                let store = self.settings_store(&t.manifest.clone());
                if let Some(values) = saved {
                    store.lock().unwrap().restore(values);
                }
                self.themes.push(LoadedTheme { applied: theme::Applied::default(), ..t });
                self.apply_theme(self.themes.len() - 1);
                err.context(format!("the new version of `{id}` didn't start; the previous version (a theme) is showing again"))
            });
        }
        let Some(idx) = self.find(&new.manifest.id) else {
            return self.activate(new, trust);
        };
        let old = self.deactivate_and_remove(idx);
        let id = new.manifest.id.clone();
        let err = match self.activate(new, trust) {
            Ok(id) => return Ok(id),
            Err(e) => e,
        };
        if old.status != Status::Active {
            return Err(err);
        }
        let store = self.settings_store(&old.manifest);
        if let Some(values) = saved {
            store.lock().unwrap().restore(values);
        }
        let previous = Prepared {
            manifest: old.manifest,
            grants: old.grants,
            hook_names: old.hook_names,
            pre: old.pre,
        };
        match self.activate(previous, old.trust) {
            Ok(_) => Err(err.context(format!(
                "the new version of `{id}` didn't start; the previous version is running again"
            ))),
            Err(e) => Err(err.context(format!(
                "the new version of `{id}` didn't start, and neither did the previous one: {e:#}"
            ))),
        }
    }

    fn remove_at(&mut self, idx: usize) -> Loaded {
        let p = self.plugins.remove(idx);
        self.backend.lock().unwrap().plugin_unloaded(&p.manifest.id);
        p
    }
}
