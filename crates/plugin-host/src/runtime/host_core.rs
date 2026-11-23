//! `host`, `host-events`, `settings`, `storage`, and the type-only
//! interfaces.

use super::convert;
use super::state::{PluginState, TimerEntry};
use super::timefmt;
use crate::bindings::wit;
use crate::capability::Capability;
use crate::model;

impl wit::types::Host for PluginState {}
impl wit::records::Host for PluginState {}
impl wit::events::Host for PluginState {}
impl wit::action_types::Host for PluginState {}
impl wit::compose_types::Host for PluginState {}
impl wit::command_types::Host for PluginState {}
impl wit::render_types::Host for PluginState {}
impl wit::notification_types::Host for PluginState {}

/// Timer limits.
const MAX_TIMERS: usize = 16;
const MIN_TIMER_MS: u64 = 50;

impl wit::host::Host for PluginState {
    fn log(&mut self, level: wit::host::LogLevel, message: String) -> wasmtime::Result<()> {
        use wit::host::LogLevel as L;
        let level = match level {
            L::Trace => model::LogLevel::Trace,
            L::Debug => model::LogLevel::Debug,
            L::Info => model::LogLevel::Info,
            L::Warn => model::LogLevel::Warn,
            L::Error => model::LogLevel::Error,
        };
        // Cap message size so a plugin can't balloon host memory via logs.
        let mut message = message;
        if message.len() > 8192 {
            let mut cut = 8192;
            while !message.is_char_boundary(cut) {
                cut -= 1;
            }
            message.truncate(cut);
            message.push('…');
        }
        PluginState::log(self, level, message);
        Ok(())
    }

    fn now_millis(&mut self) -> wasmtime::Result<u64> {
        Ok(self.now())
    }

    fn snowflake_ms(&mut self, id: u64) -> wasmtime::Result<u64> {
        Ok(timefmt::snowflake_ms(id))
    }

    fn format_time(
        &mut self,
        epoch_ms: u64,
        style: wit::host::TimeStyle,
    ) -> wasmtime::Result<String> {
        if let Some(s) = self.backend.lock().unwrap().format_time(epoch_ms, style) {
            return Ok(s);
        }
        Ok(timefmt::format(epoch_ms, style, timefmt::now_ms()))
    }

    fn format_duration(&mut self, millis: u64) -> wasmtime::Result<String> {
        Ok(timefmt::duration(millis))
    }

    fn set_timer(
        &mut self,
        id: String,
        delay_ms: u64,
        repeat: bool,
    ) -> wasmtime::Result<Result<(), wit::host::TimerError>> {
        self.main_only("host.set-timer")?;
        if delay_ms < MIN_TIMER_MS {
            return Ok(Err(wit::host::TimerError::TooFast));
        }
        if !self.timers.contains_key(&id) && self.timers.len() >= MAX_TIMERS {
            return Ok(Err(wit::host::TimerError::Limit));
        }
        let now = self.now();
        self.timers.insert(
            id,
            TimerEntry {
                next_ms: now.saturating_add(delay_ms),
                interval: repeat.then_some(delay_ms),
            },
        );
        Ok(Ok(()))
    }

    fn clear_timer(&mut self, id: String) -> wasmtime::Result<()> {
        self.main_only("host.clear-timer")?;
        self.timers.remove(&id);
        Ok(())
    }

    fn client(&mut self) -> wasmtime::Result<wit::host::ClientInfo> {
        let mut info = self.backend.lock().unwrap().client_info();
        info.host_version = env!("CARGO_PKG_VERSION").into();
        Ok(info)
    }

    fn features(&mut self) -> wasmtime::Result<Vec<String>> {
        Ok(super::features_of(&*self.backend.lock().unwrap()))
    }

    fn has_feature(&mut self, name: String) -> wasmtime::Result<bool> {
        Ok(super::features_of(&*self.backend.lock().unwrap()).contains(&name))
    }

    fn tr(&mut self, key: String) -> wasmtime::Result<String> {
        Ok(self
            .manifest
            .tr(&self.locale, &key)
            .map(str::to_string)
            .unwrap_or(key))
    }
}

impl wit::host_events::Host for PluginState {
    fn subscribe(
        &mut self,
        kinds: Vec<wit::events::EventKind>,
    ) -> wasmtime::Result<
        Vec<(
            wit::events::EventKind,
            Result<(), wit::host_events::SubscribeError>,
        )>,
    > {
        self.main_only("host-events.subscribe")?;
        let mut set = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for k in kinds {
            let ok = self.grants.has(convert::kind_capability(k));
            if ok {
                set.insert(k as u8);
                out.push((k, Ok(())));
            } else {
                out.push((k, Err(wit::host_events::SubscribeError::NotGranted)));
            }
        }
        self.subscriptions = Some(set);
        Ok(out)
    }

    fn intercept(
        &mut self,
        kinds: Vec<wit::events::EventKind>,
    ) -> wasmtime::Result<
        Vec<(
            wit::events::EventKind,
            Result<(), wit::host_events::SubscribeError>,
        )>,
    > {
        use wit::host_events::SubscribeError as E;
        self.main_only("host-events.intercept")?;
        let filtering = self.grants.has(Capability::Intercept);
        let mut set = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for k in kinds {
            let r = if !filtering || !self.grants.has(convert::kind_capability(k)) {
                Err(E::NotGranted)
            } else if !super::events::interceptable(k) {
                Err(E::Unavailable)
            } else {
                set.insert(k as u8);
                Ok(())
            };
            out.push((k, r));
        }
        self.intercepts = set;
        Ok(out)
    }

    fn intercept_http(
        &mut self,
        kinds: Vec<wit::events::EventKind>,
    ) -> wasmtime::Result<
        Vec<(
            wit::events::EventKind,
            Result<(), wit::host_events::SubscribeError>,
        )>,
    > {
        use wit::host_events::SubscribeError as E;
        self.main_only("host-events.intercept-http")?;
        let filtering = self.grants.has(Capability::InterceptHttp);
        let mut set = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for k in kinds {
            let r = if !filtering || !self.grants.has(convert::kind_capability(k)) {
                Err(E::NotGranted)
            } else if !super::http::routable(k) {
                Err(E::Unavailable)
            } else {
                set.insert(k as u8);
                Ok(())
            };
            out.push((k, r));
        }
        self.http_intercepts = set;
        Ok(out)
    }
}

impl wit::settings::Host for PluginState {
    fn get(&mut self, key: String) -> wasmtime::Result<Option<wit::settings::SettingValue>> {
        Ok(self
            .settings
            .lock()
            .unwrap()
            .get(&key)
            .map(convert::setting_to_wit))
    }

    fn set(
        &mut self,
        key: String,
        value: wit::settings::SettingValue,
    ) -> wasmtime::Result<Result<(), String>> {
        self.main_only("settings.set")?;
        let r = self
            .settings
            .lock()
            .unwrap()
            .set(&key, convert::setting_from_wit(value));
        Ok(match r {
            Ok(changed) => {
                if changed {
                    self.pending_settings.push(key);
                }
                Ok(())
            }
            Err(e) => Err(e),
        })
    }

    fn keys(&mut self) -> wasmtime::Result<Vec<String>> {
        Ok(self.settings.lock().unwrap().keys())
    }
}

impl wit::storage::Host for PluginState {
    fn get(&mut self, key: String) -> wasmtime::Result<Option<String>> {
        self.require(Capability::Storage)?;
        Ok(self.storage.lock().unwrap().get(&key).map(str::to_string))
    }

    fn get_many(&mut self, keys: Vec<String>) -> wasmtime::Result<Vec<Option<String>>> {
        self.require(Capability::Storage)?;
        let s = self.storage.lock().unwrap();
        Ok(keys
            .iter()
            .take(1024)
            .map(|k| s.get(k).map(str::to_string))
            .collect())
    }

    fn set(
        &mut self,
        key: String,
        value: String,
    ) -> wasmtime::Result<Result<(), wit::storage::StorageError>> {
        use crate::storage::StorageError as E;
        use wit::storage::StorageError as W;
        self.require(Capability::Storage)?;
        self.main_only("storage.set")?;
        Ok(self
            .storage
            .lock()
            .unwrap()
            .set(&key, &value)
            .map_err(|e| match e {
                E::QuotaExceeded => W::QuotaExceeded,
                E::KeyTooLong => W::KeyTooLong,
                E::ValueTooLarge => W::ValueTooLarge,
            }))
    }

    fn delete(&mut self, key: String) -> wasmtime::Result<()> {
        self.require(Capability::Storage)?;
        self.main_only("storage.delete")?;
        self.storage.lock().unwrap().delete(&key);
        Ok(())
    }

    fn keys(&mut self, prefix: Option<String>) -> wasmtime::Result<Vec<String>> {
        self.require(Capability::Storage)?;
        let keys = self.storage.lock().unwrap().keys();
        Ok(match prefix {
            Some(p) => keys.into_iter().filter(|k| k.starts_with(&p)).collect(),
            None => keys,
        })
    }

    fn usage(&mut self) -> wasmtime::Result<(u64, u64)> {
        self.require(Capability::Storage)?;
        Ok(self.storage.lock().unwrap().usage())
    }
}
