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
//! A press on a plugin's switch in rekindle's section (element
//! `on:<plugin id>`, the Manager's own switch) reaches the Manager as the
//! toggle it would get from its panel. The Manager turns the plugin on or off
//! in the home and the runtime. [`toggle`] waits up to [`ANSWER_WAIT`] for
//! that to finish, on the guest's thread inside its dispatch. Whatever the
//! runtime changes later is set on the engine at once and flushed on the next
//! dispatch.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use rekindle_extension::theme::ThemeProfile;
use rekindle_extension::{Driver, ExtArgs, RecoveredProfile};
use rekindle_plugin_host::manager;
use rekindle_ui_hook::{Click, UiInput};
use rekindle_wire_inject::Recolor;

use super::log;

/// The palette file in the config dir: Phoenix's colour for each token.
const PALETTE: &str = "palette.json";
/// How long the runtime waits for a request between ticks.
const TICK: Duration = Duration::from_millis(50);
/// How long a press on a plugin's switch waits for the runtime.
pub(crate) const ANSWER_WAIT: Duration = Duration::from_millis(50);

/// What the runtime last made of its plugins.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct State {
    /// The recolours for the plugins' theme.
    pub(crate) recolors: Vec<Recolor>,
    /// The plugins running, by id.
    pub(crate) running: BTreeSet<String>,
}

/// A plugin's switch was pressed: turn `plugin` on or off, and say when done.
struct Toggle {
    plugin: String,
    on: bool,
    done: mpsc::SyncSender<()>,
}

static REQUESTS: Mutex<Option<mpsc::Sender<Toggle>>> = Mutex::new(None);

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

fn requests() -> std::sync::MutexGuard<'static, Option<mpsc::Sender<Toggle>>> {
    REQUESTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Ask the runtime to turn `plugin` on or off, and wait up to
/// [`ANSWER_WAIT`] for it. True if it answered in time; false if it didn't
/// or isn't running.
pub(crate) fn toggle(plugin: &str, on: bool) -> bool {
    let (done, answered) = mpsc::sync_channel(1);
    let sent = requests().as_ref().is_some_and(|tx| {
        tx.send(Toggle {
            plugin: plugin.into(),
            on,
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

/// Bring the runtime up and serve it until the process ends.
fn run(rx: &mpsc::Receiver<Toggle>) {
    let started = Instant::now();
    let mut driver = match up() {
        Ok(d) => d,
        Err(e) => {
            log(&format!("runtime: not started: {}", crate::err(&e)));
            return;
        }
    };
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
        "runtime up in {} ms: pulley={} plugins=[{}]",
        started.elapsed().as_millis(),
        engine.map_or_else(|e| crate::err(&e), |p| p.to_string()),
        plugins.join(" ")
    ));
    let mut logs = 0;
    publish(&mut driver);
    loop {
        match rx.recv_timeout(TICK) {
            Ok(t) => {
                let at = Instant::now();
                // The Manager's own switch for the plugin, as its panel sends it.
                rekindle_ui_hook::queue_click(Click::Ui {
                    plugin: manager::OWNER.into(),
                    panel: manager::PANEL.into(),
                    element: format!("on:{}", t.plugin),
                    value: UiInput::Toggled(t.on),
                });
                driver.pump();
                let state = publish(&mut driver);
                let load = driver.rekindle().last_load();
                log(&format!(
                    "runtime: {} {} in {} ms (load: compile {} ms, instantiate {} ms, activate {} ms) running={} recolors={}",
                    t.plugin,
                    if t.on { "on" } else { "off" },
                    at.elapsed().as_millis(),
                    load.compile.as_millis(),
                    load.instantiate.as_millis(),
                    load.activate.as_millis(),
                    state.running.len(),
                    state.recolors.len(),
                ));
                // The press may have stopped waiting.
                let _ = t.done.send(());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        driver.tick();
        publish(&mut driver);
        let (records, next) = driver.rekindle().logs_since(logs);
        logs = next;
        for r in records {
            log(&format!("plugin {} {:?}: {}", r.plugin, r.level, r.message));
        }
    }
}

/// Start the Driver on the app's home, with the palette's tokens as the
/// client's, and its theme on the wire.
fn up() -> anyhow::Result<Driver> {
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
        home: Some(home),
    };
    // SAFETY: the profile has no addresses, so the Driver installs nothing.
    let mut driver = unsafe { Driver::start(args, profile) }.map_err(|e| anyhow::anyhow!(e))?;
    driver.use_framework_theme(palettes);
    driver.use_wire_theme();
    Ok(driver)
}

/// Hand the engine what the plugins make now, if it changed.
fn publish(driver: &mut Driver) -> State {
    let running = driver
        .rekindle()
        .infos()
        .into_iter()
        .map(|p| p.id)
        .collect();
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
    let state = State { recolors, running };
    super::set_plugins(state.clone());
    state
}
