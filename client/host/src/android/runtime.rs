//! The plugin runtime in the app: `rekindle-extension`'s `Driver` on a thread
//! of its own.
//!
//! The Driver starts from a profile with no addresses, so it installs no
//! detours. Its plugins come from the home in the app's own files
//! (`/data/data/<package>/files/rekindle`), installed and signed as on the
//! desktop, and run on Pulley (`rekindle_plugin_host::runtime::engine`).
//! Their theme reaches the screen as recolours of the guest's cells, from the
//! palette in the config dir (`palette.json`, token to Phoenix's colour).
//!
//! With a look in the config dir (`panel.json`, [`Look`]), rekindle's section
//! on the client's settings screen is rendered from the Manager's panel: a
//! switch per installed plugin and one for developer mode. The plugins'
//! chat-bar buttons are mounted where the wire map (`wire.json`) puts the
//! slot, and a plugin's modal goes over the screen.
//!
//! A press on any of them reaches the runtime as the click the same control
//! reports on the desktop: a row of the section as an interaction with the
//! Manager's panel, a chat-bar button as a click on it, a row of a modal as
//! an interaction with the plugin's panel. The Manager turns a plugin on or
//! off in the home and the runtime. [`press`] waits up to [`ANSWER_WAIT`] for
//! the runtime to finish, on the guest's thread inside its dispatch.
//! Whatever the runtime changes later is set on the engine at once and
//! flushed on the next dispatch.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rekindle_extension::theme::ThemeProfile;
use rekindle_extension::{Driver, ExtArgs, RecoveredProfile};
use rekindle_plugin_host::home::Home;
use rekindle_plugin_host::manager;
use rekindle_plugin_host::model::SLOT_CHAT_BAR_BUTTON;
use rekindle_ui_hook::{Click, UiInput};
use rekindle_wire_inject::{Mount, PluginPress, Recolor};

use super::log;
use crate::rows::{self, Look, Press};
use crate::wire::SlotMap;

/// The palette file in the config dir: Phoenix's colour for each token.
const PALETTE: &str = "palette.json";
/// The look of rekindle's rows, in the config dir.
const LOOK: &str = "panel.json";
/// The wire map, in the config dir: where the chat-bar slot is.
const WIRE_MAP: &str = "wire.json";
/// How long the runtime waits for a request between ticks.
const TICK: Duration = Duration::from_millis(50);
/// How many ticks the home's developer mode is read again after.
const DEVELOPER_MODE_EVERY: u32 = 20;
/// How long a press on one of rekindle's nodes waits for the runtime.
pub(crate) const ANSWER_WAIT: Duration = Duration::from_millis(50);

/// What the runtime last made of its plugins.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct State {
    /// The recolours for the plugins' theme.
    pub(crate) recolors: Vec<Recolor>,
    /// The plugins running, by id.
    pub(crate) running: BTreeSet<String>,
    /// rekindle's section, the plugins' chat-bar buttons and the modal
    /// shown, if any.
    pub(crate) mounts: Vec<Mount>,
}

/// One of rekindle's nodes was pressed: run the press, and say when done.
struct Request {
    press: PluginPress,
    done: mpsc::SyncSender<()>,
}

static REQUESTS: Mutex<Option<mpsc::Sender<Request>>> = Mutex::new(None);

/// Whether the runtime runs: the config dir holds a palette.
pub(crate) fn wanted() -> bool {
    super::config_file(PALETTE).exists()
}

/// Start the runtime's thread. It reports to logcat once it's up.
pub(crate) fn start() {
    let (tx, rx) = mpsc::channel();
    *requests() = Some(tx);
    let spawned = std::thread::Builder::new()
        .name("rekindle-runtime".into())
        .spawn(move || {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&rx))).is_err() {
                log("runtime: panicked");
            }
            *requests() = None;
        });
    if let Err(e) = spawned {
        log(&format!("runtime: no thread: {e}"));
        *requests() = None;
    }
}

fn requests() -> std::sync::MutexGuard<'static, Option<mpsc::Sender<Request>>> {
    REQUESTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether a press is the runtime's: on a row of rekindle's section or of a
/// modal, or on a chat-bar button.
pub(crate) fn handles(p: &PluginPress) -> bool {
    [
        rows::SECTION,
        rows::MODAL,
        rows::MODAL_SCRIM,
        rows::MODAL_CARD,
        SLOT_CHAT_BAR_BUTTON,
    ]
    .contains(&p.surface.as_str())
}

/// Hand a press to the runtime, and wait up to [`ANSWER_WAIT`] for it. True
/// if it answered in time; false if it didn't or isn't running.
pub(crate) fn press(p: &PluginPress) -> bool {
    let (done, answered) = mpsc::sync_channel(1);
    let sent = requests().as_ref().is_some_and(|tx| {
        tx.send(Request {
            press: p.clone(),
            done,
        })
        .is_ok()
    });
    sent && answered.recv_timeout(ANSWER_WAIT).is_ok()
}

/// The plugin home: `rekindle` in the app's own files dir.
fn home() -> anyhow::Result<PathBuf> {
    let cmdline = std::fs::read("/proc/self/cmdline")?;
    let package = cmdline.split(|b| *b == 0).next().unwrap_or_default();
    let package = std::str::from_utf8(package)?;
    // A process name such as `com.discord:service` belongs to the package
    // before the colon.
    let package = package.split(':').next().unwrap_or_default();
    if package.is_empty() || package.contains('/') {
        anyhow::bail!("no package name in /proc/self/cmdline");
    }
    Ok(PathBuf::from(format!(
        "/data/data/{package}/files/rekindle"
    )))
}

/// What the runtime draws with, and what it drew.
struct Surfaces {
    /// The look of rekindle's rows, when the config dir has one.
    look: Option<Look>,
    /// The chat-bar slot, when the wire map has one.
    chat_bar: Option<SlotMap>,
    home: Home,
    /// The home's developer mode, read every [`DEVELOPER_MODE_EVERY`]
    /// ticks and after a press.
    developer_mode: bool,
    ticks: u32,
    /// What a press on each of rekindle's rows reports, by handler surface
    /// and element, from the last render.
    presses: BTreeMap<(String, String), Press>,
}

impl Surfaces {
    fn load(home: Home) -> Surfaces {
        let look = std::fs::read_to_string(super::config_file(LOOK))
            .ok()
            .and_then(|json| {
                Look::from_json(&json)
                    .inspect_err(|e| log(&format!("runtime: {LOOK}: {}", crate::err(e))))
                    .ok()
            });
        let path = super::config_file(WIRE_MAP);
        let chat_bar = if path.exists() {
            crate::wire::read_wire_map(&path)
                .inspect_err(|e| log(&format!("runtime: {}", crate::err(e))))
                .ok()
                .and_then(|mut m| m.slots.remove(SLOT_CHAT_BAR_BUTTON))
        } else {
            None
        };
        let developer_mode = home.developer_mode().unwrap_or(false);
        Surfaces {
            look,
            chat_bar,
            home,
            developer_mode,
            ticks: 0,
            presses: BTreeMap::new(),
        }
    }

    /// The click a press reports, if it reports one. `running` is the
    /// plugins running.
    fn click(&self, p: &PluginPress, running: &BTreeSet<String>) -> Option<Click> {
        if p.surface == SLOT_CHAT_BAR_BUTTON {
            return Some(Click::Button {
                plugin: p.plugin.clone(),
                item: p.element.clone(),
            });
        }
        let Some(press) = self.presses.get(&(p.surface.clone(), p.element.clone())) else {
            // A plugin's switch in a fixed section (`work.json`) is the
            // Manager's own switch for it, flipped from whether it runs.
            let id = p.element.strip_prefix("on:")?;
            return (p.surface == rows::SECTION).then(|| Click::Ui {
                plugin: manager::OWNER.into(),
                panel: manager::PANEL.into(),
                element: p.element.clone(),
                value: UiInput::Toggled(!running.contains(id)),
            });
        };
        Some(match press.clone() {
            Press::Ui {
                plugin,
                panel,
                element,
                value,
            } => Click::Ui {
                plugin,
                panel,
                element,
                value,
            },
            Press::Close { plugin } => Click::Modal { plugin },
        })
    }
}

/// Bring the runtime up and serve it until the process ends.
fn run(rx: &mpsc::Receiver<Request>) {
    let started = Instant::now();
    let (mut driver, home) = match up() {
        Ok(d) => d,
        Err(e) => {
            log(&format!("runtime: not started: {}", crate::err(&e)));
            return;
        }
    };
    let mut surfaces = Surfaces::load(home);
    match surfaces.home.installed() {
        Ok(list) => log(&format!(
            "runtime: home {} has {} installed: {:?}",
            surfaces.home.root().display(),
            list.len(),
            list.iter().map(|i| format!("{}:{:?}", i.id, i.state)).collect::<Vec<_>>(),
        )),
        Err(e) => log(&format!("runtime: home installed() error: {}", crate::err(&e))),
    }
    let engine = rekindle_plugin_host::runtime::engine().map(|e| e.is_pulley());
    let plugins: Vec<String> = driver
        .rekindle()
        .infos()
        .iter()
        .map(|p| {
            let caps: Vec<&str> = p.granted.iter().map(|c| c.as_str()).collect();
            format!(
                "{}@{}({},{:?},caps={})",
                p.id,
                p.version,
                p.trust.as_str(),
                p.status,
                caps.join("+")
            )
        })
        .collect();
    log(&format!(
        "runtime up in {} ms: pulley={} plugins=[{}] developer_mode={} section={} chat_bar={}",
        started.elapsed().as_millis(),
        engine.map_or_else(|e| crate::err(&e), |p| p.to_string()),
        plugins.join(" "),
        surfaces.developer_mode,
        surfaces.look.is_some(),
        surfaces.chat_bar.is_some(),
    ));
    let mut logs = 0;
    publish(&mut driver, &mut surfaces);
    loop {
        match rx.recv_timeout(TICK) {
            Ok(r) => {
                serve(&mut driver, &mut surfaces, &r.press);
                // The press may have stopped waiting.
                let _ = r.done.send(());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        driver.tick();
        surfaces.ticks = surfaces.ticks.wrapping_add(1);
        if surfaces.ticks.is_multiple_of(DEVELOPER_MODE_EVERY) {
            surfaces.developer_mode = surfaces.home.developer_mode().unwrap_or(false);
        }
        publish(&mut driver, &mut surfaces);
        let (records, next) = driver.rekindle().logs_since(logs);
        logs = next;
        for r in records {
            log(&format!("plugin {} {:?}: {}", r.plugin, r.level, r.message));
        }
    }
}

/// Run one press: queue the click it reports, let the Driver take it, and
/// hand the engine what changed.
fn serve(driver: &mut Driver, surfaces: &mut Surfaces, p: &PluginPress) {
    let at = Instant::now();
    let before = running(driver);
    let click = surfaces.click(p, &before);
    let what = match &click {
        Some(Click::Ui { element, value, .. }) => format!("{element}={value:?}"),
        Some(Click::Button { plugin, item }) => format!("button {plugin}/{item}"),
        Some(Click::Modal { plugin }) => format!("close {plugin}"),
        _ => "nothing".into(),
    };
    if let Some(click) = click {
        rekindle_ui_hook::queue_click(click);
    }
    driver.pump();
    surfaces.developer_mode = surfaces.home.developer_mode().unwrap_or(false);
    let state = publish(driver, surfaces);
    let load = driver.rekindle().last_load();
    let started: Vec<&String> = state.running.difference(&before).collect();
    let stopped: Vec<&String> = before.difference(&state.running).collect();
    log(&format!(
        "runtime: {}/{}/{} -> {what} in {} ms: started={started:?} stopped={stopped:?} \
         (last load: compile {} ms, instantiate {} ms, activate {} ms) running={:?} \
         recolors={} mounts={} developer_mode={}",
        p.plugin,
        p.surface,
        p.element,
        at.elapsed().as_millis(),
        load.compile.as_millis(),
        load.instantiate.as_millis(),
        load.activate.as_millis(),
        state.running,
        state.recolors.len(),
        state.mounts.len(),
        surfaces.developer_mode,
    ));
}

/// The plugins running, by id.
fn running(driver: &mut Driver) -> BTreeSet<String> {
    driver
        .rekindle()
        .infos()
        .into_iter()
        .map(|p| p.id)
        .collect()
}

/// Start the Driver on the app's home, with the palette's tokens as the
/// client's, and its theme and chat-bar buttons on the wire.
fn up() -> anyhow::Result<(Driver, Home)> {
    let home = home()?;
    std::fs::create_dir_all(&home)?;
    let palettes = crate::layout::read_palette(&super::config_file(PALETTE));
    if palettes.is_empty() {
        anyhow::bail!("no palette in {}", super::config_file(PALETTE).display());
    }
    // A token's place in the client's theme value. With no theme function
    // to detour, only the names count: they are the tokens plugins can set.
    let tokens: BTreeSet<&String> = palettes
        .iter()
        .flat_map(|(_, t)| t.iter().map(|(name, _)| name))
        .collect();
    let profile = RecoveredProfile {
        theme: ThemeProfile {
            addr: None,
            return_size: tokens.len() * 16,
            offsets: tokens
                .iter()
                .enumerate()
                .map(|(i, t)| ((*t).clone(), i * 16))
                .collect(),
        },
        registry: None,
        compose: None,
        framework: None,
        surfaces: Vec::new(),
    };
    let args = ExtArgs {
        dev_paths: Vec::new(),
        load_installed: true,
        memory_storage: false,
        allow_network: false,
        wait_for_client: false,
        home: Some(home.clone()),
    };
    // SAFETY: the profile has no addresses, so the Driver installs nothing.
    let mut driver = unsafe { Driver::start(args, profile) }.map_err(|e| anyhow::anyhow!(e))?;
    driver.use_framework_theme(palettes);
    driver.use_wire_theme();
    driver.use_wire_chat_bar();
    Ok((driver, Home::new(home)))
}

/// Hand the engine what the plugins make now, if it changed.
fn publish(driver: &mut Driver, surfaces: &mut Surfaces) -> State {
    let running = running(driver);
    let mut mounts = Vec::new();
    if let Some(look) = &surfaces.look {
        let widgets = rekindle_ui_hook::panels()
            .into_iter()
            .find(|p| p.plugin == manager::OWNER)
            .map(|p| p.widgets)
            .unwrap_or_default();
        let items = rows::section_items(&widgets, surfaces.developer_mode);
        let modal = rekindle_ui_hook::modal_plugin()
            .zip(rekindle_ui_hook::modal_panel())
            .map(|(plugin, panel)| rows::modal(look, &plugin, &panel));
        let rendered = rows::join(std::iter::once(rows::section(look, &items)).chain(modal));
        mounts = rendered.mounts;
        surfaces.presses = rendered.presses;
    }
    if let Some(slot) = &surfaces.chat_bar {
        mounts.extend(crate::wire::button_mounts(
            &driver.chat_bar_buttons(),
            slot,
        ));
    }
    let palettes = driver.recolors();
    // Reading which palette the guest shows walks its whole tree under the
    // engine's lock, which the guest's emit waits on. One palette needs no
    // reading.
    let shown = if palettes.len() > 1 {
        super::with_engine(|e| crate::wire::shown_palette(e.guest(), &palettes)).flatten()
    } else {
        None
    };
    let recolors = crate::wire::palette_recolors(&palettes, shown);
    let state = State {
        recolors,
        running,
        mounts,
    };
    super::set_plugins(state.clone());
    state
}
