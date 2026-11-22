//! Theme files ([`crate::theme`]). The runtime applies a theme's values to
//! the backend under its id, and applies them again when its variant
//! setting or the client's features change. Nothing of it runs.

use std::sync::Arc;

use anyhow::Result;

use super::{LoadedTheme, Runtime};
use crate::capability::Trust;
use crate::manifest::Manifest;
use crate::model;
use crate::settings::SettingValue;
use crate::theme::{self, Theme};

impl Runtime {
    /// Start showing a checked theme (its settings store already open).
    pub(super) fn push_theme(&mut self, manifest: Arc<Manifest>, t: Theme, trust: Trust) -> String {
        let id = manifest.id.clone();
        self.themes.push(LoadedTheme {
            manifest,
            theme: Arc::new(t),
            trust,
            applied: theme::Applied::default(),
        });
        let idx = self.themes.len() - 1;
        self.theme_warnings(idx);
        self.apply_theme(idx);
        self.host_log(&id, model::LogLevel::Debug, "theme applied");
        id
    }

    fn theme_warnings(&self, idx: usize) {
        let t = &self.themes[idx];
        for w in &t.theme.warnings {
            self.host_log(&t.manifest.id, model::LogLevel::Warn, w);
        }
    }

    /// Show a theme's values for its selected variant, undoing what the last
    /// application set and this one doesn't.
    pub(super) fn apply_theme(&mut self, idx: usize) {
        let id = self.themes[idx].manifest.id.clone();
        let selected = match self
            .settings
            .get(&id)
            .and_then(|s| s.lock().unwrap().get(theme::VARIANT_SETTING))
        {
            Some(SettingValue::Selected(i)) => Some(i as usize),
            _ => None,
        };
        let want = self.themes[idx].theme.resolved(selected);
        let (applied, missing) = theme::apply(
            &mut *self.backend.lock().unwrap(),
            &id,
            &want,
            &self.themes[idx].applied,
        );
        self.themes[idx].applied = applied;
        for m in missing {
            self.host_log(&id, model::LogLevel::Warn, &m);
        }
    }

    pub(super) fn remove_theme_at(&mut self, idx: usize) -> LoadedTheme {
        let t = self.themes.remove(idx);
        self.backend.lock().unwrap().plugin_unloaded(&t.manifest.id);
        t
    }

    /// [`Self::reload`] for a theme file. It is checked before anything
    /// running is touched; a theme already showing is updated in place
    /// (only what changed is reset), a component with its id is replaced.
    pub(super) fn reload_theme(&mut self, bytes: &[u8], trust: Trust) -> Result<String> {
        let t = theme::parse(bytes)?;
        let manifest = Arc::new(t.manifest());
        // A component goes before the settings schema becomes the theme's,
        // so its `deactivate` still sees its own.
        if let Some(idx) = self.find(&t.id) {
            self.deactivate_and_remove(idx);
        }
        self.settings_store(&manifest);
        if let Some(idx) = self.find_theme(&t.id) {
            let lt = &mut self.themes[idx];
            (lt.manifest, lt.theme, lt.trust) = (manifest, Arc::new(t), trust);
            self.theme_warnings(idx);
            self.apply_theme(idx);
            return Ok(self.themes[idx].manifest.id.clone());
        }
        Ok(self.push_theme(manifest, t, trust))
    }
}
