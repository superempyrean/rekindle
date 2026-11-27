//! The plugin runtime for a client that has no wire to put plugins'
//! contributions on, run from hooks that live in a crate of their own.
//!
//! [`start`] runs `rekindle-extension`'s `Driver` on a thread of its own,
//! from a profile with no addresses, so it installs no detours. Its plugins
//! come from the plugin home in [`Options`], installed and signed as for any
//! other client. The tokens plugins can set are the ones in the palettes.
//!
//! After every tick the runtime publishes a [`Snapshot`]: the plugins
//! loaded and the installed ones that aren't, their chat-bar buttons, the toasts up, their panels, the theme
//! overrides and their slash commands. [`snapshot`] returns the latest one,
//! and `Options::report` names a file it is written to as JSON whenever it
//! changes. [`menu`] returns the Manager's menu, and [`generation`] counts
//! the times either changed. The
//! platform's hooks draw from it, and hand a click on one of them back with
//! `rekindle_ui_hook::queue_click`, which the next tick runs.
//!
//! The profile reads no client state. Plugins' `get-selection` is answered
//! by `Options::selection` when the platform gives one, and is empty
//! otherwise.
//!
//! A tweak named in `Options::tweaks` is one the platform carries out
//! itself. Its hooks read the value with [`tweaks::on`].
//!
//! [`stop`] ends the runtime: its thread returns, the plugins are dropped
//! and the process's wasmtime engine with them. A platform that unloads the
//! library holding the runtime calls [`unloadable`] before [`start`], and
//! [`stop`] before the library goes.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub use rekindle_extension::LiveSelection;
use rekindle_extension::theme::ThemeProfile;
pub use rekindle_extension::tweaks;
use rekindle_extension::{Driver, ExtArgs, RecoveredProfile};
use rekindle_plugin_host::manager;
use rekindle_ui_hook::Widget;
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
    /// Reads what the client shows as selected, for plugins'
    /// `get-selection`, and `get-channel` and `get-guild` of the selection.
    /// Without it, the selection is empty.
    pub selection: Option<fn() -> LiveSelection>,
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

/// A plugin loaded or installed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Plugin {
    pub id: String,
    pub version: String,
    pub trust: String,
    /// `Active` or `Faulted: <reason>` for a loaded plugin. An installed
    /// one that isn't loaded is `Off` when the user turned it off, `Not
    /// loaded` when it is on but didn't load, and otherwise `Off: <why>`.
    pub status: String,
    pub granted: Vec<String>,
}

/// A plugin's chat-bar button. A click on it is `Click::Button { plugin,
/// item }`, `item` the part of `key` after the plugin and `\u{1f}`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ChatBarButton {
    pub plugin: String,
    pub key: String,
    pub glyph: String,
    pub tooltip: String,
}

static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);
static MENU: Mutex<Vec<Widget>> = Mutex::new(Vec::new());
static GENERATION: AtomicU64 = AtomicU64::new(0);
static STOP: AtomicBool = AtomicBool::new(false);
static THREAD: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);

/// The snapshot the runtime published last, or `None` before it is up.
pub fn snapshot() -> Option<Snapshot> {
    SNAPSHOT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// The Manager's menu (`Manager::menu`): a card for each list, with how
/// many of its plugins are installed and on, and the developer-mode switch.
/// It is read when the runtime starts, and again when the Manager's panel,
/// developer mode or the installed plugins change, since reading it reads
/// every installed plugin. Empty before the runtime is up.
///
/// A platform that draws rekindle's settings as rows passes it to
/// `rekindle_ui_hook::panel::rows::shown` with the Manager's panel.
pub fn menu() -> Vec<Widget> {
    MENU.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// How many times the runtime has published a snapshot or a new menu, 0
/// before the first. A platform that polls compares it with the last one
/// it drew from and calls [`snapshot`] and [`menu`] only when it has moved.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

/// The palettes in a palette file: `{"themes": {name: {token: "#rrggbb"}}}`.
pub fn read_palette(path: &Path) -> Vec<(String, Vec<(String, u32)>)> {
    crate::layout::read_palette(path)
}

/// Start the runtime's thread. False if the thread didn't start; whether
/// the runtime came up is logged.
pub fn start(options: Options) -> bool {
    STOP.store(false, Ordering::Relaxed);
    let thread = std::thread::Builder::new()
        .name("rekindle-runtime".into())
        .spawn(move || {
            let log = options.log;
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&options))).is_err() {
                log("runtime: panicked");
            }
        });
    match thread {
        Ok(t) => {
            *THREAD
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(t);
            true
        }
        Err(_) => false,
    }
}

/// Build the wasmtime engine without its process-wide trap handler, so the
/// library holding the runtime can be unloaded once [`stop`] returns. Call
/// it before [`start`].
pub fn unloadable() {
    rekindle_plugin_host::runtime::unloadable();
}

/// End the runtime started last: its thread finishes the tick it is in and
/// returns, its plugins are dropped, and so is the process's wasmtime
/// engine. The snapshot and the menu are cleared.
pub fn stop() {
    STOP.store(true, Ordering::Relaxed);
    let thread = THREAD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(t) = thread {
        let _ = t.join();
    }
    rekindle_plugin_host::runtime::shutdown_engine();
    *SNAPSHOT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    MENU.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    GENERATION.fetch_add(1, Ordering::Release);
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
    // The installed plugins that aren't loaded, read from the home again
    // only when the loaded set changes.
    let mut loaded_ids = None;
    let mut not_loaded = Vec::new();
    // What the menu was last read for.
    let mut menu_for = None;
    while !STOP.load(Ordering::Relaxed) {
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
        let mut now = Snapshot {
            started_ms,
            developer_mode: home.developer_mode().unwrap_or(false),
            log: lines.clone(),
            ..take(&mut driver)
        };
        let ids: Vec<String> = now.plugins.iter().map(|p| p.id.clone()).collect();
        if loaded_ids.as_ref() != Some(&ids) {
            not_loaded = not_loaded_plugins(&home, &ids);
            loaded_ids = Some(ids);
        }
        now.plugins.extend(not_loaded.iter().cloned());
        let manager_panel = rekindle_ui_hook::panels()
            .into_iter()
            .find(|p| p.plugin == manager::OWNER)
            .map(|p| p.widgets);
        let menu_now = (manager_panel, now.developer_mode, now.plugins.clone());
        let mut moved = false;
        if menu_for.as_ref() != Some(&menu_now) {
            let read = driver.rekindle().menu();
            let mut menu = MENU
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            moved = *menu != read;
            *menu = read;
            menu_for = Some(menu_now);
        }
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
            moved = true;
        }
        if moved {
            GENERATION.fetch_add(1, Ordering::Release);
        }
        std::thread::sleep(TICK);
    }
}

/// The Driver on the plugin home, with the palettes' tokens as the client's.
fn up(o: &Options) -> anyhow::Result<Driver> {
    std::fs::create_dir_all(&o.home)?;
    if let Some(read) = o.selection {
        rekindle_extension::read_selection(read);
    }
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

/// The plugins installed in `home` whose ids aren't in `loaded`, with the
/// status `Off` or `Off: <why>`.
fn not_loaded_plugins(home: &rekindle_plugin_host::home::Home, loaded: &[String]) -> Vec<Plugin> {
    use rekindle_plugin_host::home::InstallState;
    home.installed()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !loaded.contains(&i.id))
        .map(|i| Plugin {
            version: i
                .manifest
                .as_ref()
                .map_or_else(String::new, |m| m.version.clone()),
            trust: i
                .trust
                .as_ref()
                .map_or_else(String::new, |t| t.trust.as_str().to_string()),
            status: match &i.state {
                InstallState::Off => "Off".to_string(),
                InstallState::Ready => "Not loaded".to_string(),
                other => format!("Off: {}", other.describe()),
            },
            granted: Vec::new(),
            id: i.id,
        })
        .collect()
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
    fn publishes_a_snapshot_and_the_menu_for_an_empty_home() {
        let dir = std::env::temp_dir().join(format!("rekindle-embedded-{}", std::process::id()));
        let report = dir.join("runtime.json");
        assert!(start(Options {
            home: dir.join("home"),
            palettes: vec![("dark".into(), vec![("text-default".into(), 0xdbdee1)])],
            tweaks: Vec::new(),
            log: |_| {},
            report: Some(report.clone()),
            selection: None,
        }));
        let deadline = Instant::now() + Duration::from_secs(30);
        let shot = loop {
            if let Some(s) = snapshot() {
                break s;
            }
            assert!(Instant::now() < deadline, "no snapshot");
            std::thread::sleep(TICK);
        };
        assert!(generation() >= 1);
        assert!(shot.plugins.is_empty());
        assert!(!shot.developer_mode);
        assert!(shot.theme.is_empty());
        // The menu is read before the first snapshot is published.
        let lists: Vec<(String, String)> = match menu().first() {
            Some(Widget::Cards(cards)) => cards
                .iter()
                .map(|c| (c.id.clone(), c.meta.clone()))
                .collect(),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            lists,
            [
                ("list:plugins".to_string(), "0 installed, 0 on".to_string()),
                ("list:themes".to_string(), "0 installed, 0 on".to_string()),
            ]
        );
        assert!(matches!(
            menu().get(1),
            Some(Widget::Toggle { id, value: false, .. }) if id == "developer-mode"
        ));
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
        assert_eq!(written["plugins"], serde_json::json!([]));
        let before = generation();
        stop();
        assert!(snapshot().is_none());
        assert!(menu().is_empty());
        assert!(generation() > before);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
