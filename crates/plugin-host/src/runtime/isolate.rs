//! The render isolate (design doc §6): a second instance of a plugin with a
//! `renderer` or `notification-filter`, made on first use. It sees message
//! content and can't write, schedule or call out; [`super::render`] holds
//! the rules for what it returns.

use anyhow::Result;
use serde::Serialize;
use wasmtime::Store;

use super::call::describe_trap;
use super::load::new_store;
use super::state::{PluginState, Role};
use super::{Hooks, Isolate, Loaded, Runtime, Status, render};
use crate::api;
use crate::model::LogLevel;

/// Traps after which a plugin's render isolate stays off until the plugin
/// is loaded again.
const MAX_ISOLATE_FAILURES: u32 = 3;

/// Renders cached per plugin. Past this many the cache starts over, so a
/// long session doesn't keep every message it ever showed.
const MAX_CACHED_RENDERS: usize = 4096;

/// Transforms one plugin returned for one message.
#[derive(Debug, Clone, Serialize)]
pub struct Rendered {
    pub plugin: String,
    pub transforms: Vec<api::Transform>,
}

impl Runtime {
    /// Ask every plugin with a `renderer` how these messages should look
    /// (one batch per paint). Results are cached per message and edit until
    /// the plugin invalidates them or its settings change, so a repaint
    /// calls no plugin. Index `i` of the result is for `batch[i]`.
    pub fn render(&mut self, batch: &[api::RenderInput]) -> Vec<Vec<Rendered>> {
        let mut out: Vec<Vec<Rendered>> = vec![Vec::new(); batch.len()];
        for idx in 0..self.plugins.len() {
            if !self.isolate_runs(idx, "renderer") {
                continue;
            }
            let p = &self.plugins[idx];
            let id = p.manifest.id.clone();
            let mut todo = Vec::new();
            for (i, input) in batch.iter().enumerate() {
                match p.render_cache.get(&cache_key(&input.message)) {
                    Some(t) if !t.is_empty() => out[i].push(Rendered {
                        plugin: id.clone(),
                        transforms: t.clone(),
                    }),
                    Some(_) => {}
                    None => todo.push(i),
                }
            }
            if todo.is_empty() {
                continue;
            }
            let inputs: Vec<api::RenderInput> = todo.iter().map(|i| batch[*i].clone()).collect();
            let fuel = self
                .config
                .limits
                .fuel_per_render
                .saturating_mul(inputs.len() as u64);
            let result = self.call_isolate(idx, fuel, |h, store| match &h.renderer {
                Some(r) => r.call_on_render(store, &inputs).map(Some),
                None => Ok(None),
            });
            let Some(Some(lists)) = result else { continue };
            if lists.len() != inputs.len() {
                let message = format!(
                    "on-render returned {} lists for {} messages; ignored",
                    lists.len(),
                    inputs.len()
                );
                self.host_log(&id, LogLevel::Warn, &message);
                continue;
            }
            let p = &mut self.plugins[idx];
            if p.render_cache.len() + todo.len() > MAX_CACHED_RENDERS {
                p.forget_renders(None);
            }
            let shared = p.store.data().shared.clone();
            let shared = shared.lock().unwrap();
            for (i, transforms) in todo.into_iter().zip(lists) {
                let message = &batch[i].message;
                let clean = render::sanitize(transforms, &message.content, &shared);
                p.render_cache.insert(cache_key(message), clean.clone());
                p.render_channels.insert(message.id, message.channel);
                if !clean.is_empty() {
                    out[i].push(Rendered {
                        plugin: id.clone(),
                        transforms: clean,
                    });
                }
            }
        }
        out
    }

    /// Run a notification the client is about to show past every
    /// `notification-filter` (in the render isolate). The first plugin that
    /// decides anything other than `show` wins.
    pub fn filter_notification(
        &mut self,
        n: &api::ClientNotification,
    ) -> (api::NotificationDecision, Option<String>) {
        for idx in 0..self.plugins.len() {
            if !self.isolate_runs(idx, "notification-filter") {
                continue;
            }
            let fuel = self.config.limits.fuel_per_render;
            let d = self.call_isolate(idx, fuel, |h, store| match &h.notifications {
                Some(f) => f.call_on_notification(store, n).map(Some),
                None => Ok(None),
            });
            if let Some(Some(d)) = d
                && !matches!(d, api::NotificationDecision::Show)
            {
                return (d, Some(self.plugins[idx].manifest.id.clone()));
            }
        }
        (api::NotificationDecision::Show, None)
    }

    /// Whether plugin `idx` is active, its render isolate isn't switched
    /// off, and the host calls its `hook` there.
    fn isolate_runs(&self, idx: usize, hook: &str) -> bool {
        let p = &self.plugins[idx];
        p.status == Status::Active
            && p.renderer_disabled.is_none()
            && p.hook_names.iter().any(|h| h == hook)
    }

    fn ensure_isolate(&mut self, idx: usize) -> Result<()> {
        if self.plugins[idx].isolate.is_some() {
            return Ok(());
        }
        let p = &self.plugins[idx];
        let (m, grants, pre) = (p.manifest.clone(), p.grants.clone(), p.pre.clone());
        let shared = p.store.data().shared.clone();
        let state = self.new_state(&m, &grants, Role::Render, shared)?;
        let mut store = new_store(&self.engine, state);
        let hooks = self.instantiate(&pre, &mut store, &grants, Role::Render)?;
        self.plugins[idx].isolate = Some(Isolate { store, hooks });
        Ok(())
    }

    /// Call into plugin `idx`'s render isolate with `fuel`, making the
    /// isolate first if there is none. A trap drops the isolate and counts
    /// against the plugin; the plugin itself keeps running.
    fn call_isolate<R>(
        &mut self,
        idx: usize,
        fuel: u64,
        f: impl FnOnce(&Hooks, &mut Store<PluginState>) -> wasmtime::Result<R>,
    ) -> Option<R> {
        if let Err(e) = self.ensure_isolate(idx) {
            let p = &mut self.plugins[idx];
            p.renderer_disabled = Some(format!("couldn't create the render isolate: {e:#}"));
            let id = p.manifest.id.clone();
            self.host_log(&id, LogLevel::Error, &format!("render isolate: {e:#}"));
            return None;
        }
        let p = &mut self.plugins[idx];
        let iso = p.isolate.as_mut().expect("ensure_isolate made one");
        iso.store
            .set_fuel(fuel)
            .expect("the engine meters fuel (`engine()` turns it on)");
        let e = match f(&iso.hooks, &mut iso.store) {
            Ok(r) => return Some(r),
            Err(e) => e,
        };
        // The isolate's memory may be inconsistent after a trap. A new one
        // is made on next use.
        p.isolate = None;
        p.isolate_failures += 1;
        let reason = describe_trap(&e, None);
        let id = p.manifest.id.clone();
        if p.isolate_failures >= MAX_ISOLATE_FAILURES {
            p.renderer_disabled = Some(reason.clone());
            let message = format!("renderer disabled after repeated failures: {reason}");
            self.host_log(&id, LogLevel::Error, &message);
        } else {
            let message = format!("[render] {reason}; message shown untransformed");
            self.host_log(&id, LogLevel::Warn, &message);
        }
        None
    }
}

impl Loaded {
    /// Forget the cached renders of messages in `channel`, or of every
    /// message, so the next paint renders them again.
    pub(super) fn forget_renders(&mut self, channel: Option<u64>) {
        let Some(channel) = channel else {
            self.render_cache.clear();
            self.render_channels.clear();
            return;
        };
        let channels = &self.render_channels;
        self.render_cache
            .retain(|(message, _), _| channels.get(message) != Some(&channel));
        self.render_channels.retain(|_, c| *c != channel);
    }
}

/// A message's render cache key: its id and when it was last edited.
fn cache_key(m: &api::Message) -> (u64, Option<u64>) {
    (m.id, m.edited_ms)
}
