//! What the host keeps for each plugin across loads: its storage, in a
//! namespace per account, and its schema settings with the generated
//! settings page.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::Serialize;

use super::{Runtime, Status, StorageLocation};
use crate::api;
use crate::manifest::Manifest;
use crate::model::UiOp;
use crate::settings::{SettingSpec, SettingValue, SettingsStore};
use crate::storage::PluginStorage;

/// One plugin's generated settings page (design doc §10), for the client to
/// render: each declared setting with its current value and whether it is
/// visible / enabled right now.
#[derive(Debug, Clone, Serialize)]
pub struct SettingsPage {
    pub plugin: String,
    pub entries: Vec<SettingEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SettingEntry {
    pub spec: SettingSpec,
    pub value: SettingValue,
    pub visible: bool,
    pub enabled: bool,
}

impl Runtime {
    /// The plugin's storage namespace for the current account (also
    /// available before/after it's loaded).
    pub fn storage(&mut self, id: &str) -> Result<Arc<Mutex<PluginStorage>>> {
        let ns = self.namespace(id);
        if let Some(s) = self.storages.get(&ns) {
            return Ok(s.clone());
        }
        let quota = self.config.limits.storage;
        let s = match &self.config.storage {
            StorageLocation::Memory => {
                // First account seen adopts pre-account data.
                match (self.account, self.storages.get(id)) {
                    (Some(_), Some(legacy))
                        if !self
                            .storages
                            .keys()
                            .any(|k| k.starts_with(&format!("{id}@"))) =>
                    {
                        let data = legacy.lock().unwrap().entries().clone();
                        let mut s = PluginStorage::in_memory(quota);
                        for (k, v) in data {
                            let _ = s.set(&k, &v);
                        }
                        s
                    }
                    _ => PluginStorage::in_memory(quota),
                }
            }
            StorageLocation::Dir(dir) => {
                if self.account.is_some() {
                    adopt_legacy_storage(dir, id, &ns);
                }
                PluginStorage::open(dir, &ns, quota)?
            }
        };
        let s = Arc::new(Mutex::new(s));
        self.storages.insert(ns, s.clone());
        Ok(s)
    }

    fn namespace(&self, id: &str) -> String {
        match self.account {
            Some(a) => format!("{id}@{a}"),
            None => id.to_string(),
        }
    }

    /// The logged-in account. Switching swaps every plugin's storage to that
    /// account's namespace, drops its per-account UI (decorations,
    /// accessories, ghost messages, DM sections) and delivers
    /// `account-switched`. Settings stay shared. If a plugin's storage for
    /// `account` can't be opened, nothing switches.
    pub fn set_account(&mut self, account: Option<u64>) -> Result<()> {
        if self.account == account {
            return Ok(());
        }
        let previous = std::mem::replace(&mut self.account, account);
        let ids: Vec<String> = self.plugins.iter().map(|p| p.manifest.id.clone()).collect();
        let storages: Result<Vec<_>> = ids.iter().map(|id| self.storage(id)).collect();
        let storages = match storages {
            Ok(s) => s,
            Err(e) => {
                self.account = previous;
                return Err(e);
            }
        };
        for (idx, (id, storage)) in ids.iter().zip(storages).enumerate() {
            self.plugins[idx].store.data_mut().storage = storage.clone();
            if let Some(iso) = self.plugins[idx].isolate.as_mut() {
                iso.store.data_mut().storage = storage;
            }
            let st = self.plugins[idx].store.data_mut();
            st.decorations.clear();
            st.accessories.clear();
            st.ghosts.clear();
            self.plugins[idx].forget_renders(None);
            // A client without per-account surfaces has nothing to clear.
            let _ = self
                .backend
                .lock()
                .unwrap()
                .ui_update(id, UiOp::ClearAccountState);
        }
        if let Some(a) = account {
            self.dispatch_event(&api::Event::AccountSwitched(a));
        }
        Ok(())
    }

    pub fn account(&self) -> Option<u64> {
        self.account
    }

    /// The plugin's schema settings (host-kept, per plugin, not per account).
    pub(super) fn settings_store(&mut self, m: &Manifest) -> Arc<Mutex<SettingsStore>> {
        if let Some(s) = self.settings.get(&m.id) {
            s.lock().unwrap().set_specs(m.settings.clone());
            return s.clone();
        }
        let s = match &self.config.storage {
            StorageLocation::Memory => SettingsStore::in_memory(m.settings.clone()),
            StorageLocation::Dir(dir) => SettingsStore::open(dir, &m.id, m.settings.clone()),
        };
        let s = Arc::new(Mutex::new(s));
        self.settings.insert(m.id.clone(), s.clone());
        s
    }

    /// The plugin's generated settings page.
    pub fn settings_page(&self, plugin: &str) -> Option<SettingsPage> {
        let s = self.settings.get(plugin)?.lock().unwrap();
        let entries = s
            .specs()
            .iter()
            .map(|spec| SettingEntry {
                spec: spec.clone(),
                value: s.get(&spec.key).expect("declared"),
                visible: s.visible(&spec.key),
                enabled: s.enabled(&spec.key),
            })
            .collect();
        Some(SettingsPage {
            plugin: plugin.to_string(),
            entries,
        })
    }

    /// Set a setting before the plugin loads (tests, imported settings).
    pub fn seed_setting(
        &mut self,
        manifest: &Manifest,
        key: &str,
        value: SettingValue,
    ) -> Result<()> {
        let s = self.settings_store(manifest);
        s.lock()
            .unwrap()
            .set(key, value)
            .map_err(anyhow::Error::msg)?;
        Ok(())
    }

    /// The user changed a setting on the generated page.
    pub fn set_setting(&mut self, plugin: &str, key: &str, value: SettingValue) -> Result<()> {
        let s = self
            .settings
            .get(plugin)
            .with_context(|| format!("plugin `{plugin}` has no settings"))?
            .clone();
        let changed = s
            .lock()
            .unwrap()
            .set(key, value)
            .map_err(anyhow::Error::msg)?;
        if changed {
            self.settings_changed(plugin, key);
        }
        Ok(())
    }

    /// The user reset a setting to its default.
    pub fn reset_setting(&mut self, plugin: &str, key: &str) -> Result<()> {
        let s = self
            .settings
            .get(plugin)
            .with_context(|| format!("plugin `{plugin}` has no settings"))?
            .clone();
        if s.lock().unwrap().reset(key) {
            self.settings_changed(plugin, key);
        }
        Ok(())
    }

    fn settings_changed(&mut self, plugin: &str, key: &str) {
        if let Some(idx) = self.find_theme(plugin) {
            self.apply_theme(idx);
            return;
        }
        let Some(idx) = self.find(plugin) else { return };
        self.plugins[idx].forget_renders(None);
        if self.plugins[idx].status == Status::Active {
            self.call(idx, |h, store| match &h.lifecycle {
                Some(l) => l.call_on_settings_changed(store, key),
                None => Ok(()),
            });
        }
    }
}

/// Move a pre-account storage file (`<id>.json`) to the first account's
/// namespace, if no account has one yet.
fn adopt_legacy_storage(dir: &std::path::Path, id: &str, ns: &str) {
    let legacy = dir.join(format!("{id}.json"));
    let target = dir.join(format!("{ns}.json"));
    if !legacy.exists() || target.exists() {
        return;
    }
    let prefix = format!("{id}@");
    let any_account = std::fs::read_dir(dir).is_ok_and(|rd| {
        rd.flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with(&prefix))
    });
    if !any_account {
        let _ = std::fs::rename(legacy, target);
    }
}
