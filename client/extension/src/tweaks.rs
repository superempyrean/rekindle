//! The tweaks the host maintains in the client, for `tweaks.set`.
//!
//! A tweak is offered only when the host implements it on this client: the
//! profile names those ids ([`crate::RecoveredProfile::tweaks`]). The host
//! reads a tweak's value with [`on`] where it carries the behaviour out.
//!
//! Each plugin's preset is kept, and the latest one counts. The presets are
//! saved to `tweaks.json` in the plugin home, so a tweak stays set across
//! launches until its plugin resets it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use rekindle_plugin_host::model::{Tweak, TweakError, TweakKind};

/// Don't send typing indicators.
pub const SILENT_TYPING: &str = "silent-typing";

/// The tweaks a host can implement: id, label, description. Each is a
/// toggle.
const KNOWN: &[(&str, &str, &str)] = &[(
    SILENT_TYPING,
    "Silent typing",
    "Don't send typing indicators.",
)];

#[derive(Default)]
struct State {
    /// The ids the host implements on this client.
    offered: Vec<String>,
    /// Per tweak, each plugin's preset, oldest first.
    presets: BTreeMap<String, Vec<(String, String)>>,
    /// Where the presets are saved. `None` keeps them in memory.
    file: Option<PathBuf>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut s = STATE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    f(s.get_or_insert_with(State::default))
}

/// Offer the tweaks in `ids` that rekindle knows.
pub(crate) fn offer(ids: &[String]) {
    with(|s| {
        s.offered = ids
            .iter()
            .filter(|id| KNOWN.iter().any(|(k, ..)| k == id))
            .cloned()
            .collect();
    });
}

/// Keep the presets in `file`, and read the ones saved there.
pub(crate) fn load(file: PathBuf) {
    let saved = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    with(|s| {
        s.presets = saved;
        s.file = Some(file);
    });
}

fn save(s: &State) {
    if let Some(file) = &s.file
        && let Ok(text) = serde_json::to_string_pretty(&s.presets)
    {
        let _ = std::fs::write(file, text);
    }
}

fn value(s: &State, id: &str) -> Option<String> {
    s.presets.get(id)?.last().map(|(_, v)| v.clone())
}

/// True if the tweak `id` is offered and set to `true`.
pub fn on(id: &str) -> bool {
    with(|s| s.offered.iter().any(|o| o == id) && value(s, id).as_deref() == Some("true"))
}

/// Every tweak rekindle knows, with its value and whether it is offered.
pub(crate) fn all() -> Vec<Tweak> {
    with(|s| {
        KNOWN
            .iter()
            .map(|(id, label, description)| Tweak {
                id: (*id).to_string(),
                label: (*label).to_string(),
                description: (*description).to_string(),
                kind: TweakKind::Toggle,
                choices: Vec::new(),
                value: value(s, id).unwrap_or_else(|| "false".into()),
                available: s.offered.iter().any(|o| o == id),
            })
            .collect()
    })
}

pub(crate) fn set(plugin: &str, id: &str, v: &str) -> Result<(), TweakError> {
    with(|s| {
        if !KNOWN.iter().any(|(k, ..)| *k == id) {
            return Err(TweakError::Unknown);
        }
        if !s.offered.iter().any(|o| o == id) {
            return Err(TweakError::Unavailable);
        }
        if !matches!(v, "true" | "false") {
            return Err(TweakError::InvalidValue);
        }
        let p = s.presets.entry(id.to_string()).or_default();
        p.retain(|(owner, _)| owner != plugin);
        p.push((plugin.to_string(), v.to_string()));
        save(s);
        Ok(())
    })
}

pub(crate) fn reset(plugin: &str, id: &str) {
    with(|s| {
        if let Some(p) = s.presets.get_mut(id) {
            p.retain(|(owner, _)| owner != plugin);
            if p.is_empty() {
                s.presets.remove(id);
            }
            save(s);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test, since the state is the process's.
    #[test]
    fn offered_tweaks_take_the_latest_preset_and_persist() {
        let file = std::env::temp_dir().join(format!("rk-tweaks-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        load(file.clone());
        offer(&[]);
        assert_eq!(set("a", SILENT_TYPING, "true"), Err(TweakError::Unavailable));
        assert_eq!(set("a", "nope", "true"), Err(TweakError::Unknown));
        offer(&[SILENT_TYPING.to_string(), "nope".to_string()]);
        assert!(all().iter().all(|t| t.id != "nope"));
        assert_eq!(set("a", SILENT_TYPING, "yes"), Err(TweakError::InvalidValue));
        assert!(!on(SILENT_TYPING));
        set("a", SILENT_TYPING, "true").unwrap();
        assert!(on(SILENT_TYPING));
        set("b", SILENT_TYPING, "false").unwrap();
        assert!(!on(SILENT_TYPING));
        reset("b", SILENT_TYPING);
        assert!(on(SILENT_TYPING));
        // A new launch reads the saved presets.
        load(file.clone());
        assert!(on(SILENT_TYPING));
        reset("a", SILENT_TYPING);
        assert!(!on(SILENT_TYPING));
        assert_eq!(all()[0].value, "false");
        let _ = std::fs::remove_file(&file);
    }
}
