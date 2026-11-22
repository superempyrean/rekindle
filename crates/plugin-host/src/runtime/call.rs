//! Calls into a plugin's main instance: the fuel each call gets, the gesture
//! it may carry, and what a trap does. A plugin that traps is marked faulted
//! and its contributions are removed; other plugins keep running.

use anyhow::{Context, Result, bail};
use wasmtime::Store;

use super::state::{Gesture, PluginState};
use super::{Hooks, Runtime, Status};
use crate::model::{self, LogLevel, Mods};

impl Runtime {
    /// The index of `plugin`, which must be loaded and not faulted.
    pub(super) fn active(&self, plugin: &str) -> Result<usize> {
        let idx = self
            .find(plugin)
            .with_context(|| format!("plugin `{plugin}` is not loaded"))?;
        if let Status::Faulted(why) = &self.plugins[idx].status {
            bail!("plugin `{plugin}` has faulted: {why}");
        }
        Ok(idx)
    }

    /// [`Self::call`] while delivering one of the user's own gestures. The
    /// gesture-bound host functions work for its duration only, within its
    /// budget.
    pub(super) fn call_gesture<R>(
        &mut self,
        idx: usize,
        mods: Mods,
        f: impl FnOnce(&Hooks, &mut Store<PluginState>) -> wasmtime::Result<R>,
    ) -> Option<R> {
        self.plugins[idx].store.data_mut().gesture = Some(Gesture::new(mods));
        let r = self.call(idx, f);
        self.plugins[idx].store.data_mut().gesture = None;
        r
    }

    /// [`Self::call_with_fuel`] with the fuel of an ordinary call.
    pub(super) fn call<R>(
        &mut self,
        idx: usize,
        f: impl FnOnce(&Hooks, &mut Store<PluginState>) -> wasmtime::Result<R>,
    ) -> Option<R> {
        let fuel = self.config.limits.fuel_per_call;
        self.call_with_fuel(idx, fuel, f)
    }

    /// Call into plugin `idx`'s main instance with `fuel`, then deliver what
    /// the call queued (settings it changed, render invalidations). `None`
    /// if the plugin isn't active, or if the call trapped, which faults it.
    pub(super) fn call_with_fuel<R>(
        &mut self,
        idx: usize,
        fuel: u64,
        f: impl FnOnce(&Hooks, &mut Store<PluginState>) -> wasmtime::Result<R>,
    ) -> Option<R> {
        if self.plugins[idx].status != Status::Active {
            return None;
        }
        let r = self.run(idx, fuel, f);
        self.drain_pending(idx);
        r
    }

    /// Run `f` on plugin `idx`'s main instance with `fuel`, and fault the
    /// plugin if it traps.
    fn run<R>(
        &mut self,
        idx: usize,
        fuel: u64,
        f: impl FnOnce(&Hooks, &mut Store<PluginState>) -> wasmtime::Result<R>,
    ) -> Option<R> {
        let log_mark = self.logs.since(u64::MAX).1;
        let p = &mut self.plugins[idx];
        p.store
            .set_fuel(fuel)
            .expect("the engine meters fuel (`engine()` turns it on)");
        match f(&p.hooks, &mut p.store) {
            Ok(r) => Some(r),
            Err(e) => {
                self.fault(idx, &e, log_mark);
                None
            }
        }
    }

    /// Mark plugin `idx` faulted after the trap `e`, drop its render isolate
    /// and remove what it added to the client. `log_mark` is where the log
    /// stood when the call began: the last error the plugin logged since is
    /// the likely cause.
    fn fault(&mut self, idx: usize, e: &wasmtime::Error, log_mark: u64) {
        let p = &mut self.plugins[idx];
        let id = p.manifest.id.clone();
        let logged = self.logs.since(log_mark).0;
        let last_error = logged
            .iter()
            .rev()
            .find(|r| r.plugin == id && r.level == LogLevel::Error)
            .map(|r| r.message.as_str());
        let reason = describe_trap(e, last_error);
        let message = format!("plugin faulted and was disabled: {reason}");
        p.status = Status::Faulted(reason);
        p.isolate = None;
        self.backend.lock().unwrap().plugin_unloaded(&id);
        self.host_log(&id, LogLevel::Error, &message);
    }

    /// Deliver what the last call queued: render invalidations, then
    /// `on-settings-changed` for each setting the plugin changed itself.
    /// Settings changed in those calls wait for the next call.
    fn drain_pending(&mut self, idx: usize) {
        let p = &mut self.plugins[idx];
        let st = p.store.data_mut();
        let settings = std::mem::take(&mut st.pending_settings);
        for channel in std::mem::take(&mut st.pending_invalidate) {
            p.forget_renders(channel);
        }
        if !settings.is_empty() {
            p.forget_renders(None);
        }
        for key in settings {
            if self.plugins[idx].status != Status::Active {
                break;
            }
            let fuel = self.config.limits.fuel_per_call;
            self.run(idx, fuel, |h, store| match &h.lifecycle {
                Some(l) => l.call_on_settings_changed(store, &key),
                None => Ok(()),
            });
        }
    }

    /// Log `message` for `plugin`, marked as the host's.
    pub(super) fn host_log(&self, plugin: &str, level: LogLevel, message: &str) {
        self.logs.push(model::LogRecord {
            plugin: plugin.to_string(),
            level,
            message: format!("[host] {message}"),
            at_ms: self.start.elapsed().as_millis() as u64,
        });
    }
}

/// Why a call trapped, for the plugin's status and the log. `last_error` is
/// the last error the plugin logged during the call (a Rust panic message, a
/// script's uncaught error): the actual cause, when there is one.
pub(super) fn describe_trap(e: &wasmtime::Error, last_error: Option<&str>) -> String {
    // Its first line, which is the summary (a script's stack follows it); a
    // Rust panic's location ends in `:` and the message is the next line.
    let said = last_error.map(|m| {
        let mut lines = m.lines().map(str::trim);
        let first = lines.next().unwrap_or_default();
        let first = first.strip_prefix("plugin panicked: ").unwrap_or(first);
        match lines.next() {
            Some(next) if first.ends_with(':') => format!("{first} {next}"),
            _ => first.to_string(),
        }
    });
    let Some(t) = e.downcast_ref::<wasmtime::Trap>() else {
        return format!("{e:#}");
    };
    match (t, said) {
        (wasmtime::Trap::OutOfFuel, _) => "exceeded its CPU budget (out of fuel)".into(),
        (wasmtime::Trap::Interrupt, _) => "took too long handling events".into(),
        (wasmtime::Trap::UnreachableCodeReached, Some(said)) => said,
        (wasmtime::Trap::UnreachableCodeReached, None) => "crashed without logging an error (wasm `unreachable`). In a JS/TS \
            plugin this usually means a rekindle call got a value of the wrong type (a number where a bigint is \
            expected, an unknown enum string or variant tag, a missing record field). The SDK (`rekindle` module) \
            builds the values for you"
            .into(),
        (other, Some(said)) => format!("trap: {other} ({said})"),
        (other, None) => format!("trap: {other}"),
    }
}
