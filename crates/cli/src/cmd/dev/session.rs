//! A preview session: a [`Runtime`] driving a [`MockBackend`], the plugins
//! loaded into it, and (for `rekindle dev`) the source each one rebuilds from.
//!
//! The session lives on one thread. Builds run on worker threads and report
//! back through a channel; [`Session::tick`] collects them, polls the source
//! watchers, and echoes new plugin logs to the terminal.
//!
//! The mock's [`World`](rekindle_plugin_host::backend::World) is the client
//! the page shows: its guilds, channels, people and messages. Every target
//! the page can click on comes from it, and every interaction is checked
//! against it the way a real client could only offer what it holds.

use std::collections::{BTreeMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use rekindle_plugin_host::api::UserRef;
use rekindle_plugin_host::backend::MockBackend;
use rekindle_plugin_host::builder::BuildOptions;
use rekindle_plugin_host::capability::Trust;
use rekindle_plugin_host::model::LogRecord;
use rekindle_plugin_host::runtime::{self, Runtime, RuntimeConfig, SharedBackend, StorageLocation};
use rekindle_plugin_host::sourcemap::SourceMap;
use serde_json::Value;

use super::api::{Action, ApiError};
use super::term;

mod actions;
mod build;
mod state;
mod world;

pub(super) use build::compile;
use build::{BuildDone, Source};

/// Activity lines kept (and sent).
const KEEP_ACTIVITY: usize = 120;

/// Dev mode runs the developer's own code as first-party (network still
/// limited to the hosts its manifest declares).
pub(super) const DEV_TRUST: Trust = Trust::FirstParty;

/// A plugin in the session.
struct Entry {
    id: String,
    trust: Trust,
    /// Last bytes that loaded successfully.
    bytes: Option<Vec<u8>>,
    load_error: Option<String>,
    /// Where it rebuilds from, for `rekindle dev`.
    source: Option<Source>,
}

pub(super) struct Session {
    title: String,
    runtime: Runtime,
    backend: Arc<Mutex<MockBackend>>,
    entries: Vec<Entry>,
    /// The profile popout the page last opened (its sections are in state).
    profile: Option<UserRef>,
    build_tx: Sender<BuildDone>,
    build_rx: Receiver<BuildDone>,
    log_seq: u64,
    last_poll: Instant,
    /// The last message sent from the preview, after `compose` plugins.
    last_send: Option<Value>,
    /// The last slash command run, and its reply.
    last_slash: Option<Value>,
    /// What happened, for the page (also echoed to the terminal).
    activity: VecDeque<(u64, String)>,
    activity_seq: u64,
    /// Bumped by every action that went through (the world changes without
    /// the mock noticing).
    changes: u64,
    storage: StorageLocation,
    /// JS plugins' source maps, by id (re-applied after a reset).
    maps: BTreeMap<String, Arc<SourceMap>>,
    /// Print plugin logs and activity to the terminal as they come in
    /// (`rekindle test` turns this off and prints the logs only when a test
    /// fails).
    echo_logs: bool,
}

/// A runtime over a fresh mock client.
fn fresh_client(storage: &StorageLocation) -> Result<(Runtime, Arc<Mutex<MockBackend>>)> {
    let backend = Arc::new(Mutex::new(MockBackend::default()));
    let shared: SharedBackend = backend.clone();
    let config = RuntimeConfig {
        storage: storage.clone(),
        ..Default::default()
    };
    Ok((Runtime::new(shared, config)?, backend))
}

impl Session {
    /// A session over a fresh mock client. `storage` should usually be a
    /// directory so plugin settings survive restarts.
    pub(super) fn new(title: impl Into<String>, storage: StorageLocation) -> Result<Self> {
        let (runtime, backend) = fresh_client(&storage)?;
        let (build_tx, build_rx) = channel();
        Ok(Self {
            title: title.into(),
            runtime,
            backend,
            entries: Vec::new(),
            profile: None,
            build_tx,
            build_rx,
            log_seq: 0,
            last_poll: Instant::now(),
            last_send: None,
            last_slash: None,
            activity: VecDeque::new(),
            activity_seq: 0,
            changes: 0,
            storage,
            maps: BTreeMap::new(),
            echo_logs: true,
        })
    }

    pub(super) fn title(&self) -> &str {
        &self.title
    }

    pub(super) fn set_echo_logs(&mut self, on: bool) {
        self.echo_logs = on;
    }

    /// The last `n` plugin log records.
    pub(super) fn recent_logs(&self, n: usize) -> Vec<LogRecord> {
        let all = self.runtime.logs().snapshot();
        all[all.len().saturating_sub(n)..].to_vec()
    }

    /// Rewrite stack frames in `id`'s logs with `map` from now on.
    pub(super) fn set_source_map(&mut self, id: &str, map: Option<Arc<SourceMap>>) {
        match &map {
            Some(m) => self.maps.insert(id.to_string(), m.clone()),
            None => self.maps.remove(id),
        };
        self.runtime.logs().set_source_map(id, map);
    }

    /// Start over: a fresh mock client and storage (in memory, or the same
    /// directory), and every plugin loaded again from its last good build.
    /// `rekindle test` resets between tests through `/api/reset`.
    fn reset(&mut self) -> Result<(), ApiError> {
        let (runtime, backend) =
            fresh_client(&self.storage).map_err(|e| ApiError::new(500, format!("{e:#}")))?;
        self.runtime = runtime;
        self.backend = backend;
        self.profile = None;
        self.last_send = None;
        self.last_slash = None;
        self.activity.clear();
        self.log_seq = 0;
        for (id, map) in &self.maps {
            self.runtime.logs().set_source_map(id, Some(map.clone()));
        }
        for idx in 0..self.entries.len() {
            if let Some(bytes) = self.entries[idx].bytes.clone() {
                self.install(idx, bytes)
                    .map_err(|e| ApiError::new(409, e))?;
            }
        }
        Ok(())
    }

    /// Add a plugin and load it.
    ///
    /// `bytes` is the component, or the build error if the first build
    /// failed (only meaningful with a `source`, which is then rebuilt on
    /// change). Load failures are recorded and shown in the preview; they are
    /// also returned so the caller can report them.
    pub(super) fn add(
        &mut self,
        id: &str,
        trust: Trust,
        bytes: Result<Vec<u8>, String>,
        source: Option<BuildOptions>,
    ) -> Result<(), String> {
        self.entries.push(Entry {
            id: id.to_string(),
            trust,
            bytes: None,
            load_error: None,
            source: source.map(|opts| Source::new(opts, bytes.as_ref().err())),
        });
        self.install(self.entries.len() - 1, bytes?)
    }

    /// Load (or reload) `bytes` for entry `idx`. If the new bytes fail to
    /// load, the previous good version is restored.
    fn install(&mut self, idx: usize, bytes: Vec<u8>) -> Result<(), String> {
        // The manifest id may have changed since the last build.
        let new_id = match runtime::inspect_artifact(self.runtime.engine(), &bytes) {
            Ok(i) => i.manifest.id,
            Err(e) => return self.load_failed(idx, format!("{e:#}")),
        };
        let old_id = self.entries[idx].id.clone();
        let reloading = self.runtime.plugin(&old_id).is_some();
        if new_id != old_id {
            if reloading {
                // The plugin is replaced either way; an unload that fails
                // has nothing left to clean up.
                let _ = self.runtime.unload(&old_id);
            }
            term::note(&format!("plugin id changed: {old_id} -> {new_id}"));
            self.entries[idx].id.clone_from(&new_id);
        }
        let trust = self.entries[idx].trust;
        match self.runtime.reload(&bytes, trust) {
            Ok(_) => {
                let e = &mut self.entries[idx];
                e.bytes = Some(bytes);
                e.load_error = None;
                if self.echo_logs && reloading {
                    term::ok(&format!("reloaded {new_id}"));
                } else if self.echo_logs {
                    self.print_grants(&new_id);
                }
                Ok(())
            }
            Err(err) => {
                let err = format!("{err:#}");
                let restored = match self.entries[idx].bytes.clone() {
                    Some(old) if new_id == old_id => self.runtime.load(&old, trust).is_ok(),
                    _ => false,
                };
                let msg = if restored {
                    format!("{err} (kept the previous build running)")
                } else {
                    err
                };
                self.load_failed(idx, msg)
            }
        }
    }

    fn load_failed(&mut self, idx: usize, msg: String) -> Result<(), String> {
        term::error(&format!("{}: failed to load: {msg}", self.entries[idx].id));
        self.entries[idx].load_error = Some(msg.clone());
        Err(msg)
    }

    fn print_grants(&self, id: &str) {
        let Some(info) = self.runtime.plugin(id) else {
            return;
        };
        let granted: Vec<&str> = info.granted.iter().map(|c| c.as_str()).collect();
        let granted = if granted.is_empty() {
            "(none)".to_string()
        } else {
            granted.join(", ")
        };
        term::ok(&format!(
            "loaded {} {} ({}) as {}",
            info.name,
            info.version,
            info.id,
            info.trust.as_str()
        ));
        term::line(&format!("    granted: {granted}"));
        if !info.network_allowlist.is_empty() {
            let hosts: Vec<&str> = info.network_allowlist.iter().map(String::as_str).collect();
            term::line(&format!("    network: {}", hosts.join(", ")));
        }
    }

    /// Collect finished builds, fire due timers, rescan sources, echo new
    /// logs.
    pub(super) fn tick(&mut self) {
        while let Ok(done) = self.build_rx.try_recv() {
            self.finish_build(done);
        }
        // Plugins' timers (`set-timer`, and the JS prelude's setTimeout).
        self.runtime.fire_due_timers();
        self.poll_sources(Instant::now());
        self.print_logs();
    }

    fn print_logs(&mut self) {
        let (recs, next) = self.runtime.logs().since(self.log_seq);
        self.log_seq = next;
        if self.echo_logs {
            for r in &recs {
                term::log(r);
            }
        }
    }

    /// Changes whenever anything the preview shows changes.
    pub(super) fn fingerprint(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.backend.lock().unwrap().revision().hash(&mut h);
        self.runtime.logs().since(u64::MAX).1.hash(&mut h);
        self.changes.hash(&mut h);
        for e in &self.entries {
            e.id.hash(&mut h);
            e.load_error.hash(&mut h);
            if let Some(s) = &e.source {
                s.status.hash(&mut h);
            }
        }
        for p in self.runtime.plugins() {
            format!("{:?}", p.status).hash(&mut h);
        }
        h.finish()
    }

    /// Record what happened, for the page and the terminal.
    fn note(&mut self, msg: String) {
        if self.echo_logs {
            term::note(&msg);
        }
        self.activity_seq += 1;
        self.activity.push_back((self.activity_seq, msg));
        while self.activity.len() > KEEP_ACTIVITY {
            self.activity.pop_front();
        }
    }

    /// Perform an API action. The caller responds with [`Session::state`].
    pub(super) fn act(&mut self, action: Action) -> Result<(), ApiError> {
        // Reading state must not count as a change, or the page's long-poll
        // would wake itself forever.
        let reading = action == Action::State;
        let r = self.perform(action);
        if r.is_ok() && !reading {
            self.changes += 1;
        }
        r
    }

    fn find(&self, id: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }
}

/// A plugin call the runtime refused, or that failed in the plugin.
#[expect(
    clippy::needless_pass_by_value,
    reason = "a `map_err` argument, which gets the error by value"
)]
fn conflict(e: anyhow::Error) -> ApiError {
    ApiError::new(409, format!("{e:#}"))
}

#[cfg(test)]
mod fixtures {
    use rekindle_plugin_host::api::Message;
    use rekindle_plugin_host::backend::World;
    use rekindle_plugin_host::backend::fixtures::GENERAL;
    use rekindle_plugin_host::model::{Icon, MenuPredicate, SlotItem};
    use rekindle_plugin_host::runtime::StorageLocation;

    use super::Session;

    pub(super) fn session() -> Session {
        Session::new("t", StorageLocation::Memory).unwrap()
    }

    pub(super) fn world<R>(s: &Session, f: impl FnOnce(&World) -> R) -> R {
        f(&s.backend.lock().unwrap().world)
    }

    pub(super) fn general_messages(s: &Session) -> Vec<Message> {
        world(s, |w| w.messages[&GENERAL].clone())
    }

    pub(super) fn menu(id: &str, when: Vec<MenuPredicate>) -> SlotItem {
        SlotItem::MenuItem {
            id: id.into(),
            label: id.into(),
            icon: None,
            danger: false,
            group: None,
            checked: None,
            when,
        }
    }

    pub(super) fn button(id: &str) -> SlotItem {
        SlotItem::IconButton {
            id: id.into(),
            icon: Icon::Star,
            image: None,
            tooltip: id.into(),
            active: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use rekindle_plugin_host::backend::fixtures::{ALICE, GENERAL};
    use rekindle_plugin_host::model::{UiEvent, UiValue};

    use super::fixtures::{general_messages, session, world};
    use super::*;

    #[test]
    fn state_reflects_backend_and_validates_actions() {
        let mut s = session();
        let st = s.state(3);
        assert_eq!(st["rev"], 3);
        assert_eq!(
            st["theme"].as_array().unwrap().len(),
            rekindle_plugin_host::tokens::default_tokens().len()
        );
        assert!(st["theme"][0]["value"].as_str().unwrap().starts_with('#'));
        let visible = world(&s, |w| w.channels.iter().filter(|c| c.visible).count());
        assert_eq!(
            st["client"]["channels"].as_array().unwrap().len(),
            visible,
            "hidden channels aren't listed"
        );
        assert_eq!(st["client"]["selection"]["channel"], GENERAL);
        assert_eq!(
            st["client"]["rows"].as_array().unwrap().len(),
            general_messages(&s).len()
        );
        assert_eq!(st["client"]["me"]["name"], "You");

        // Nothing is loaded: plugin-targeted actions are refused, not panics.
        let refused = [
            Action::Command {
                plugin: "nope".into(),
                id: "x".into(),
            },
            Action::Reload {
                plugin: Some("nope".into()),
            },
            Action::Keybind {
                plugin: "nope".into(),
                id: "k".into(),
            },
            Action::Setting {
                plugin: "nope".into(),
                key: "k".into(),
                value: None,
            },
            Action::Slash {
                plugin: "nope".into(),
                command: "c".into(),
                options: BTreeMap::new(),
                channel: None,
            },
            Action::Decoration {
                plugin: "nope".into(),
                kind: "user.name-badge".into(),
                key: ALICE,
            },
            Action::Ui {
                plugin: "nope".into(),
                event: UiEvent {
                    surface: "notice".into(),
                    element: "button".into(),
                    value: UiValue::Clicked,
                },
            },
        ];
        for a in refused {
            assert_eq!(s.act(a).unwrap_err().status, 404);
        }
        assert!(
            s.act(Action::SetPresence {
                status: "idle".into()
            })
            .is_ok()
        );
        assert_eq!(world(&s, |w| w.status.clone()), "idle");

        // A bad component is recorded as a load error and shown in state.
        let before = s.fingerprint();
        assert!(
            s.add("broken", DEV_TRUST, Ok(b"not wasm".to_vec()), None)
                .is_err()
        );
        let st = s.state(4);
        assert_eq!(st["plugins"][0]["id"], "broken");
        assert!(st["plugins"][0]["load_error"].as_str().is_some());
        assert_ne!(before, s.fingerprint());
    }

    #[test]
    fn reading_state_is_not_a_change() {
        let mut s = session();
        let fp = s.fingerprint();
        s.act(Action::State).unwrap();
        assert_eq!(
            fp,
            s.fingerprint(),
            "a state read must not wake the long-poll"
        );
        s.act(Action::SetPresence {
            status: "dnd".into(),
        })
        .unwrap();
        assert_ne!(fp, s.fingerprint());
        let fp = s.fingerprint();
        assert!(s.act(Action::SelectChannel { channel: 1 }).is_err());
        assert_eq!(fp, s.fingerprint(), "a refused action changes nothing");
    }
}
