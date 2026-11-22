//! Per-plugin host state: what each store carries, the gesture model and
//! rate limits.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use wasmtime::StoreLimits;

use super::{Logs, SharedBackend};
use crate::capability::{Capability, Grants};
use crate::manifest::Manifest;
use crate::model::{self, ClickBinding, Mods};
use crate::net::{NetClient, NetLimits};
use crate::settings::SettingsStore;
use crate::storage::PluginStorage;

/// Which instance of the plugin a store belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// Linked with everything the plugin was granted.
    Main,
    /// The render isolate (design doc §6): sees message content through
    /// `on-render` and `on-notification`, with no way to keep or send it.
    Render,
}

/// A user gesture being delivered (design doc §5.1) and what's left of its
/// budget.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Gesture {
    pub mods: Mods,
    pub local: u32,
    pub react: u32,
    pub outward: u32,
    pub presence: u32,
    pub clipboard: u32,
    pub navigation: u32,
    pub files: u32,
}

impl Gesture {
    pub(crate) fn new(mods: Mods) -> Self {
        Self {
            mods,
            local: 10,
            react: 3,
            outward: 1,
            presence: 1,
            clipboard: 1,
            navigation: 1,
            files: 1,
        }
    }
}

/// Which budget an action draws on.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Budget {
    Local,
    React,
    Outward,
    Presence,
    Clipboard,
    Navigation,
    Files,
}

/// A sliding-window rate limit.
#[derive(Debug, Default)]
pub(crate) struct Rate(VecDeque<u64>);

impl Rate {
    /// Record a use at `now` if fewer than `max` happened in the last
    /// `window_ms`; `false` if over the limit.
    pub(crate) fn take(&mut self, now: u64, max: usize, window_ms: u64) -> bool {
        while self
            .0
            .front()
            .is_some_and(|t| now.saturating_sub(*t) >= window_ms)
        {
            self.0.pop_front();
        }
        if self.0.len() >= max {
            return false;
        }
        self.0.push_back(now);
        true
    }
}

/// A scheduled timer. `next_ms` is host-elapsed ms; `interval` is `Some` for a
/// repeating timer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TimerEntry {
    pub next_ms: u64,
    pub interval: Option<u64>,
}

/// State a plugin's two instances share (the isolate only reads it).
#[derive(Debug, Default)]
pub(crate) struct Shared {
    /// Images registered with `ui.register-image`: id -> (w, h).
    pub images: BTreeMap<String, (u32, u32)>,
    /// Accessory element ids the main instance declared for the isolate.
    pub declared_actions: BTreeSet<String>,
}

pub(crate) struct PluginState {
    pub id: String,
    pub role: Role,
    pub grants: Grants,
    pub manifest: Arc<Manifest>,
    pub backend: SharedBackend,
    pub storage: Arc<Mutex<PluginStorage>>,
    pub settings: Arc<Mutex<SettingsStore>>,
    pub shared: Arc<Mutex<Shared>>,
    pub net: Arc<dyn NetClient>,
    pub net_limits: NetLimits,
    pub logs: Logs,
    pub start: Instant,
    pub limits: StoreLimits,
    /// Slot items this plugin added: (slot, id).
    pub slot_items: BTreeSet<(String, String)>,
    pub timers: BTreeMap<String, TimerEntry>,
    pub overlay_images: BTreeSet<String>,
    pub overlay_sprites: BTreeSet<String>,
    /// `Some` only while the host is delivering one of the user's own
    /// gestures (design doc §5.1).
    pub gesture: Option<Gesture>,
    /// Event kinds subscribed to (`None`: everything granted).
    pub subscriptions: Option<BTreeSet<u8>>,
    /// Event kinds the plugin's `event-filter` sees (`host-events.intercept`).
    pub intercepts: BTreeSet<u8>,
    pub decorations: BTreeSet<(String, u64)>,
    pub accessories: BTreeSet<u64>,
    /// channel -> ghost ids.
    pub ghosts: BTreeMap<u64, BTreeSet<u64>>,
    /// Palette commands registered, by id.
    pub commands: BTreeSet<String>,
    pub keybinds: BTreeSet<String>,
    pub slash_commands: BTreeSet<String>,
    pub click_bindings: BTreeMap<String, ClickBinding>,
    pub sounds: BTreeSet<String>,
    pub rates: Rates,
    /// Settings keys changed by the plugin itself; `on-settings-changed`
    /// fires once the current call returns.
    pub pending_settings: Vec<String>,
    /// `render-control.invalidate` calls: `None` = everything.
    pub pending_invalidate: Vec<Option<u64>>,
    /// The client's locale (for `host.tr`).
    pub locale: String,
    /// `Some` while a call with a wall-clock limit runs (an event handler).
    pub wall: Option<WallClock>,
    /// Slots the plugin added to that this client build can't show.
    pub missing_slots: BTreeSet<String>,
    /// `Some` while `on-compose` runs: whether it is a send, and the parts
    /// the plugin asked to split it into.
    pub composing: Option<Composing>,
}

/// The plugin's rate limits, one per kind of call.
#[derive(Debug, Default)]
pub(crate) struct Rates {
    pub toast: Rate,
    pub react: Rate,
    pub outward: Rate,
    pub status: Rate,
    /// Rich presence / custom status, which need no gesture.
    pub activity: Rate,
    pub notify: Rate,
    pub speak: Rate,
    pub link: Rate,
    pub nav: Rate,
}

/// One `on-compose` call (see [`PluginState::composing`]).
#[derive(Debug, Clone, Default)]
pub(crate) struct Composing {
    pub sending: bool,
    pub split: Option<Vec<String>>,
}

/// The wall-clock limit of one call. Time the plugin spends waiting on the
/// host (an HTTP request) doesn't count against it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WallClock {
    pub started: Instant,
    pub limit: std::time::Duration,
    /// Time spent inside host calls that wait (HTTP).
    pub waited: std::time::Duration,
}

impl WallClock {
    pub(crate) fn new(limit: std::time::Duration) -> Self {
        Self {
            started: Instant::now(),
            limit,
            waited: std::time::Duration::ZERO,
        }
    }

    /// The plugin's own time so far is over the limit.
    pub(crate) fn over(&self) -> bool {
        self.started.elapsed().saturating_sub(self.waited) > self.limit
    }
}

impl PluginState {
    pub(crate) fn now(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    pub(crate) fn require(&self, cap: Capability) -> wasmtime::Result<()> {
        if self.role == Role::Render && !matches!(cap, Capability::Storage | Capability::Theming) {
            return Err(wasmtime::Error::msg(format!(
                "plugin `{}` called a `{}` host function from its render isolate, where only log, clock, \
                 formatting, settings/storage reads and theme reads are linked (design doc §6)",
                self.id,
                cap.interface().unwrap_or(cap.as_str())
            )));
        }
        if self.grants.has(cap) {
            Ok(())
        } else {
            Err(wasmtime::Error::msg(format!(
                "plugin `{}` called a `{}` host function without the `{cap}` capability",
                self.id,
                cap.interface().unwrap_or(cap.as_str())
            )))
        }
    }

    /// Trap if called from the render isolate.
    pub(crate) fn main_only(&self, what: &str) -> wasmtime::Result<()> {
        if self.role == Role::Render {
            return Err(wasmtime::Error::msg(format!(
                "plugin `{}` called `{what}` from its render isolate, which can't write or schedule anything",
                self.id
            )));
        }
        Ok(())
    }

    /// Spend one unit of a gesture budget. `false` outside a gesture or when
    /// that budget is spent.
    pub(crate) fn spend(&mut self, b: Budget) -> bool {
        let Some(g) = self.gesture.as_mut() else {
            return false;
        };
        let slot = match b {
            Budget::Local => &mut g.local,
            Budget::React => &mut g.react,
            Budget::Outward => &mut g.outward,
            Budget::Presence => &mut g.presence,
            Budget::Clipboard => &mut g.clipboard,
            Budget::Navigation => &mut g.navigation,
            Budget::Files => &mut g.files,
        };
        if *slot == 0 {
            return false;
        }
        *slot -= 1;
        true
    }

    pub(crate) fn log(&self, level: model::LogLevel, message: String) {
        let message = if self.role == Role::Render {
            format!("[render] {message}")
        } else {
            message
        };
        self.logs.push(model::LogRecord {
            plugin: self.id.clone(),
            level,
            message,
            at_ms: self.now(),
        });
    }

    pub(crate) fn warn(&self, message: impl Into<String>) {
        self.log(model::LogLevel::Warn, format!("[host] {}", message.into()));
    }

    pub(crate) fn image_registered(&self, id: &str) -> bool {
        self.shared.lock().unwrap().images.contains_key(id)
    }
}

/// Sanitize a short user-visible string: no control characters, at most
/// `max` characters. `None` if it ends up empty.
pub(crate) fn clean(s: &str, max: usize) -> Option<String> {
    let out: String = s.chars().filter(|c| !c.is_control()).take(max).collect();
    (!out.trim().is_empty()).then_some(out)
}

/// Like [`clean`] but keeps newlines and tabs.
pub(crate) fn clean_multiline(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .take(max)
        .collect()
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}
