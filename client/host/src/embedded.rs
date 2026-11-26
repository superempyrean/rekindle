//! The plugin runtime for a client that has no wire to put plugins'
//! contributions on, run from hooks that live in a crate of their own.
//!
//! [`start`] runs `rekindle-extension`'s `Driver` on a thread of its own,
//! from a profile with no addresses, so it installs no detours. Its plugins
//! come from the plugin home in [`Options`], installed and signed as for any
//! other client. The tokens plugins can set are the ones in the palettes.
//!
//! After every tick the runtime publishes a [`Snapshot`]: the plugins
//! loaded, their chat-bar buttons, the toasts up, their panels, the theme
//! overrides and their slash commands. [`snapshot`] returns the latest one,
//! and `Options::report` names a file it is written to as JSON whenever it
//! changes. The platform's hooks draw from it, and hand a click on one of
//! them back with `rekindle_ui_hook::queue_click`, which the next tick runs.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rekindle_extension::theme::ThemeProfile;
use rekindle_extension::{Driver, ExtArgs, RecoveredProfile};
use serde::Serialize;

/// How long the runtime sleeps between ticks.
const TICK: Duration = Duration::from_millis(50);

/// What the platform gives the runtime.
pub struct Options {
    /// The plugin home the runtime loads installed plugins from.
    pub home: PathBuf,
    /// The client's colour for each theme token, `0xRRGGBB`, per client
    /// theme by name. Plugins can set the tokens named here.
    pub palettes: Vec<(String, Vec<(String, u32)>)>,
    /// The tweaks the platform implements (`rekindle_extension::tweaks`).
    pub tweaks: Vec<String>,
    /// Write one line to the platform's log.
    pub log: fn(&str),
    /// A file the snapshot is written to as JSON whenever it changes.
    pub report: Option<PathBuf>,
}

/// What the runtime last made of its plugins.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Snapshot {
    /// Milliseconds the runtime took to start.
    pub started_ms: u64,
    pub developer_mode: bool,
    pub plugins: Vec<Plugin>,
    pub chat_bar: Vec<ChatBarButton>,
    pub toasts: Vec<String>,
    /// Each plugin panel, as `plugin/panel`.
    pub panels: Vec<String>,
    /// Each token a plugin set, with the colour that applies, `#rrggbbaa`.
    pub theme: BTreeMap<String, String>,
    pub slash_commands: Vec<String>,
    /// Log lines the plugins wrote, the last [`LOG_LINES`].
    pub log: Vec<String>,
}

/// How many of the plugins' log lines a snapshot keeps.
pub const LOG_LINES: usize = 32;

/// A loaded plugin.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Plugin {
    pub id: String,
    pub version: String,
    pub trust: String,
    /// `Active`, or `Faulted: <reason>`.
    pub status: String,
    pub granted: Vec<String>,
}

/// A plugin's chat-bar button. A click on it is
/// `Click::Button { plugin, item: key }`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChatBarButton {
    pub plugin: String,
    pub key: String,
    pub glyph: String,
    pub tooltip: String,
}

static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);

/// The snapshot the runtime published last, or `None` before it is up.
pub fn snapshot() -> Option<Snapshot> {
    SNAPSHOT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// The palettes in a palette file: `{"themes": {name: {token: "#rrggbb"}}}`.
pub fn read_palette(path: &Path) -> Vec<(String, Vec<(String, u32)>)> {
    crate::layout::read_palette(path)
}

/// Start the runtime's thread. False if the thread didn't start; whether
/// the runtime came up is logged.
pub fn start(options: Options) -> bool {
    std::thread::Builder::new()
        .name("rekindle-runtime".into())
        .spawn(move || {
            let log = options.log;
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&options))).is_err() {
                log("runtime: panicked");
            }
        })
        .is_ok()
}

fn run(o: &Options) {
    let started = Instant::now();
    let mut driver = match up(o) {
        Ok(d) => d,
        Err(e) => {
            (o.log)(&format!("runtime: not started: {e:#}"));
            return;
        }
    };
    driver.rekindle().poll();
    driver.pump();
    let started_ms = started.elapsed().as_millis() as u64;
    let home = rekindle_plugin_host::home::Home::new(o.home.clone());
    let mut lines = Vec::new();
    let mut logs = 0;
    let mut last = None;
    loop {
        driver.tick();
        let (records, next) = driver.rekindle().logs_since(logs);
        logs = next;
        for r in records {
            let line = format!("{} {:?}: {}", r.plugin, r.level, r.message);
            (o.log)(&format!("plugin {line}"));
            lines.push(line);
        }
        if lines.len() > LOG_LINES {
            lines.drain(..lines.len() - LOG_LINES);
        }
        let now = Snapshot {
            started_ms,
            developer_mode: home.developer_mode().unwrap_or(false),
            log: lines.clone(),
            ..take(&mut driver)
        };
        if last.as_ref() != Some(&now) {
            if last.is_none() {
                (o.log)(&format!(
                    "runtime up in {started_ms} ms: home={} developer_mode={} plugins=[{}]",
                    o.home.display(),
                    now.developer_mode,
                    now.plugins
                        .iter()
                        .map(|p| format!("{}@{}({},{})", p.id, p.version, p.trust, p.status))
                        .collect::<Vec<_>>()
                        .join(" "),
                ));
            }
            if let Some(path) = &o.report
                && let Ok(json) = serde_json::to_string_pretty(&now)
            {
                let _ = std::fs::write(path, json);
            }
            *SNAPSHOT
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(now.clone());
            last = Some(now);
        }
        std::thread::sleep(TICK);
    }
}

/// The Driver on the plugin home, with the palettes' tokens as the client's.
fn up(o: &Options) -> anyhow::Result<Driver> {
    std::fs::create_dir_all(&o.home)?;
    let names: std::collections::BTreeSet<&String> = o
        .palettes
        .iter()
        .flat_map(|(_, t)| t.iter().map(|(name, _)| name))
        .collect();
    // With no theme function to detour, only the names count: they are the
    // tokens plugins can set.
    let profile = RecoveredProfile {
        theme: ThemeProfile {
            addr: None,
            return_size: names.len() * 16,
            offsets: names
                .iter()
                .enumerate()
                .map(|(i, t)| ((*t).clone(), i * 16))
                .collect(),
        },
        registry: None,
        compose: None,
        framework: None,
        surfaces: Vec::new(),
        slash_commands: true,
        wire_menus: false,
        wire_picker_tabs: false,
        elements: BTreeMap::new(),
        wire_values: false,
        tweaks: o.tweaks.clone(),
    };
    let args = ExtArgs {
        dev_paths: Vec::new(),
        load_installed: true,
        memory_storage: false,
        allow_network: false,
        wait_for_client: false,
        home: Some(o.home.clone()),
    };
    // SAFETY: the profile has no addresses, so the Driver installs nothing.
    let mut driver = unsafe { Driver::start(args, profile) }.map_err(|e| anyhow::anyhow!(e))?;
    driver.use_framework_theme(o.palettes.clone());
    Ok(driver)
}

/// What the Driver and the UI contributions hold now.
fn take(driver: &mut Driver) -> Snapshot {
    let plugins = driver
        .rekindle()
        .infos()
        .into_iter()
        .map(|p| Plugin {
            id: p.id,
            version: p.version,
            trust: p.trust.as_str().to_string(),
            status: match p.status {
                rekindle_plugin_host::runtime::Status::Active => "Active".to_string(),
                rekindle_plugin_host::runtime::Status::Faulted(why) => format!("Faulted: {why}"),
            },
            granted: p.granted.iter().map(|c| c.as_str().to_string()).collect(),
        })
        .collect();
    let chat_bar = driver
        .chat_bar_buttons()
        .into_iter()
        .map(|(plugin, b)| ChatBarButton {
            plugin,
            key: b.key,
            glyph: b.glyph,
            tooltip: b.tooltip,
        })
        .collect();
    let theme = driver
        .theme_overrides()
        .into_iter()
        .map(|(token, hsla)| {
            let c = rekindle_native::Rgba::from_hsla(hsla);
            let hex = [c.r, c.g, c.b, c.a]
                .iter()
                .fold(String::from("#"), |mut s, v| {
                    let _ = write!(s, "{:02x}", (v.clamp(0.0, 1.0) * 255.0).round() as u8);
                    s
                });
            (token, hex)
        })
        .collect();
    Snapshot {
        plugins,
        chat_bar,
        toasts: rekindle_ui_hook::live_toasts()
            .into_iter()
            .map(|t| t.text)
            .collect(),
        panels: rekindle_ui_hook::panels()
            .into_iter()
            .map(|p| format!("{}/{}", p.plugin, p.id))
            .collect(),
        theme,
        slash_commands: driver
            .slash_commands()
            .into_iter()
            .map(|(_, c)| c.name)
            .collect(),
        ..Snapshot::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishes_a_snapshot_for_an_empty_home() {
        let dir = std::env::temp_dir().join(format!("rekindle-embedded-{}", std::process::id()));
        let report = dir.join("runtime.json");
        assert!(start(Options {
            home: dir.join("home"),
            palettes: vec![("dark".into(), vec![("text-default".into(), 0xdbdee1)])],
            tweaks: Vec::new(),
            log: |_| {},
            report: Some(report.clone()),
        }));
        let deadline = Instant::now() + Duration::from_secs(30);
        let shot = loop {
            if let Some(s) = snapshot() {
                break s;
            }
            assert!(Instant::now() < deadline, "no snapshot");
            std::thread::sleep(TICK);
        };
        assert!(shot.plugins.is_empty());
        assert!(!shot.developer_mode);
        assert!(shot.theme.is_empty());
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
        assert_eq!(written["plugins"], serde_json::json!([]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
