//! The plugin runtime in the app: `rekindle-extension`'s `Driver` on a thread
//! of its own.
//!
//! The Driver starts from a profile with no addresses, so it installs no
//! detours. Its plugins come from the platform's home ([`super::Platform`]),
//! installed and signed as on the desktop, and run on Pulley
//! (`rekindle_plugin_host::runtime::engine`).
//! Their theme reaches the screen as recolours of the guest's cells, from the
//! palette in the config dir (`palette.json`, token to Phoenix's colour).
//!
//! With a look in the config dir (`panel.json`, [`Look`]), rekindle's menu
//! on the client's settings screen is rendered from the Manager: a row for
//! the plugins, one for the themes, and the developer-mode switch. Any other
//! view of the Manager's panel is a page over the screen, which the system
//! back closes as its back button does. The plugins' chat-bar buttons are
//! mounted where the wire map (`wire.json`) puts the slot, and a plugin's
//! modal goes over the screen.
//!
//! A press on any of them reaches the runtime as the click the same control
//! reports on the desktop: a row of the menu or a page as an interaction
//! with the Manager's panel, a row of a plugin's own panel on its page as an
//! interaction with that panel, a chat-bar button as a click on it, a row of
//! a modal as an interaction with the plugin's panel. The Manager turns a
//! plugin on or off in the home and the runtime. [`press`] waits up to
//! [`ANSWER_WAIT`] for the runtime to finish, on the guest's thread inside
//! its dispatch. Whatever the runtime changes later is set on the engine at
//! once and flushed on the next dispatch.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use std::sync::atomic::{AtomicU64, Ordering};

use rekindle_extension::theme::ThemeProfile;
use rekindle_extension::{Driver, ExtArgs, RecoveredProfile};
use rekindle_plugin_host::home::Home;
use rekindle_plugin_host::manager;
use rekindle_plugin_host::model::SLOT_CHAT_BAR_BUTTON;
use rekindle_ui_hook::{Click, UiInput, Widget};
use rekindle_wire_inject::{Handler, Mount, PluginPress, Recolor, Write};

use super::log;
use crate::gateway::{Decision, Passed};
use crate::messages::MessageMap;
use crate::rows::{self, Look, Press};
use crate::wire::SlotMap;

/// The palette file in the config dir: the build's theme binding, or
/// Phoenix's colour for each token.
pub(crate) const PALETTE: &str = "palette.json";
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
/// How long a press on rekindle's menu or a page waits. The Manager reads
/// every installed plugin's file and checks its signature to draw a list,
/// which took 170 to 490 ms for 38 plugins on the emulator, and an idle
/// client can go seven seconds without a dispatch to flush a late answer.
pub(crate) const PAGE_WAIT: Duration = Duration::from_millis(600);
/// How long a dispatch waits for the plugins' event filters before the
/// guest receives it unchanged. Filters run on the per-render budget, so
/// only a runtime busy with something else makes one wait this long.
const FILTER_WAIT: Duration = Duration::from_millis(250);
/// How many dispatches' events are logged by name, then one in this many.
const EVENTS_LOGGED: u64 = 64;

/// What the runtime last made of its plugins.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct State {
    /// The recolours for the plugins' theme.
    pub(crate) recolors: Vec<Recolor>,
    /// The plugins running, by id.
    pub(crate) running: BTreeSet<String>,
    /// rekindle's menu, the page and the modal shown, if any, and the
    /// plugins' chat-bar buttons.
    pub(crate) mounts: Vec<Mount>,
    /// What the system back does: close the modal or the page on top.
    pub(crate) back: Option<Handler>,
    /// The cells the plugins' message renderers write on the messages
    /// shown.
    pub(crate) cells: Vec<Write>,
}

/// What the guest's thread hands the runtime.
enum Request {
    /// One of rekindle's nodes was pressed: run the press, and say when
    /// done.
    Press {
        press: PluginPress,
        done: mpsc::SyncSender<()>,
    },
    /// Run a dispatch past the plugins' event filters, and send back its
    /// `d`, or `None` if a plugin dropped it.
    Filter {
        t: String,
        d: serde_json::Value,
        done: mpsc::SyncSender<Option<serde_json::Value>>,
    },
    /// The guest received these dispatches: deliver their events.
    Events(Vec<Passed>),
}

/// The event kinds some plugin's event filter sees, as bits by
/// `EventKind as u8`. Read on the guest's thread for every dispatch.
static INTERCEPTED: AtomicU64 = AtomicU64::new(0);

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
        rows::PAGE,
        rows::PAGE_PANEL,
        rows::MODAL,
        rows::MODAL_SCRIM,
        rows::MODAL_CARD,
        SLOT_CHAT_BAR_BUTTON,
    ]
    .contains(&p.surface.as_str())
}

/// Hand a press to the runtime, and wait up to [`ANSWER_WAIT`] for it, or
/// [`PAGE_WAIT`] on rekindle's menu and pages. True
/// if it answered in time; false if it didn't or isn't running.
pub(crate) fn press(p: &PluginPress) -> bool {
    let (done, answered) = mpsc::sync_channel(1);
    let sent = send(Request::Press {
        press: p.clone(),
        done,
    });
    let wait = if [rows::SECTION, rows::PAGE].contains(&p.surface.as_str()) {
        PAGE_WAIT
    } else {
        ANSWER_WAIT
    };
    sent && answered.recv_timeout(wait).is_ok()
}

/// Whether the runtime's thread is up.
pub(crate) fn is_up() -> bool {
    requests().is_some()
}

fn send(r: Request) -> bool {
    requests().as_ref().is_some_and(|tx| tx.send(r).is_ok())
}

/// Whether some plugin's event filter sees the events dispatch `t` causes.
pub(crate) fn intercepts(t: &str) -> bool {
    rekindle_plugin_host::backend::gateway::dispatch_kind(t)
        .is_some_and(|k| INTERCEPTED.load(Ordering::Relaxed) & (1 << (k as u8)) != 0)
}

/// Run dispatch `t` past the plugins' event filters, and wait up to
/// [`FILTER_WAIT`] for them. On the guest's thread, before it receives the
/// dispatch.
pub(crate) fn filter(t: &str, d: &serde_json::Value) -> Decision {
    let (done, answer) = mpsc::sync_channel(1);
    let at = Instant::now();
    if !send(Request::Filter {
        t: t.to_string(),
        d: d.clone(),
        done,
    }) {
        return Decision::Pass;
    }
    let decision = match answer.recv_timeout(FILTER_WAIT) {
        Ok(None) => Decision::Drop,
        Ok(Some(new)) if new != *d => Decision::Replace(new),
        Ok(Some(_)) => Decision::Pass,
        Err(_) => {
            log(&format!(
                "gateway: {t} passed unfiltered after {} ms",
                at.elapsed().as_millis()
            ));
            return Decision::Pass;
        }
    };
    log(&format!(
        "gateway: filter {t} -> {} in {} us",
        match &decision {
            Decision::Pass => "pass",
            Decision::Drop => "drop",
            Decision::Replace(_) => "replace",
        },
        at.elapsed().as_micros()
    ));
    decision
}

/// Hand the dispatches the guest received to the runtime, for their events.
pub(crate) fn received(passed: Vec<Passed>) {
    if !passed.is_empty() {
        send(Request::Events(passed));
    }
}

/// What the runtime draws with, and what it drew.
struct Surfaces {
    /// The look of rekindle's rows, when the config dir has one.
    look: Option<Look>,
    /// The chat-bar slot, when the wire map has one.
    chat_bar: Option<SlotMap>,
    /// Where the messages are, when the wire map says.
    messages: Option<MessageMap>,
    /// The turns the guest had sent when the messages were last read, and
    /// the cells the renderers made of them.
    read_at: Option<u64>,
    message_cells: Vec<Write>,
    home: Home,
    /// The home's developer mode, read every [`DEVELOPER_MODE_EVERY`]
    /// ticks and after a press.
    developer_mode: bool,
    ticks: u32,
    /// What a press on each of rekindle's rows reports, by handler surface
    /// and element, from the last render.
    presses: BTreeMap<(String, String), Press>,
    /// The Manager's menu. Reading it reads every installed plugin, so it
    /// is read again only after a press and when developer mode changes.
    menu: Vec<Widget>,
    /// The page shown, and which block of ids it has.
    page: Option<Mount>,
    block: usize,
    /// The toasts shown, by id, and how many sets of them were shown.
    toasts: Vec<u64>,
    toast_sets: u32,
    /// The palettes the guest had named when plugins' reads of the
    /// client's colours were last set.
    seeded: Option<u64>,
    /// The theme report last logged.
    theme_report: String,
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
        let map = if path.exists() {
            crate::wire::read_wire_map(&path)
                .inspect_err(|e| log(&format!("runtime: {}", crate::err(e))))
                .ok()
        } else {
            None
        };
        let (chat_bar, messages) = match map {
            Some(mut m) => (m.slots.remove(SLOT_CHAT_BAR_BUTTON), m.messages),
            None => (None, None),
        };
        let developer_mode = home.developer_mode().unwrap_or(false);
        Surfaces {
            look,
            chat_bar,
            messages,
            read_at: None,
            message_cells: Vec::new(),
            home,
            developer_mode,
            ticks: 0,
            presses: BTreeMap::new(),
            menu: Vec::new(),
            page: None,
            block: 0,
            toasts: Vec::new(),
            toast_sets: 0,
            seeded: None,
            theme_report: String::new(),
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
            list.iter()
                .map(|i| format!("{}:{:?}", i.id, i.state))
                .collect::<Vec<_>>(),
        )),
        Err(e) => log(&format!(
            "runtime: home installed() error: {}",
            crate::err(&e)
        )),
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
    surfaces.menu = driver.rekindle().menu();
    publish(&mut driver, &mut surfaces);
    let mut received = 0u64;
    loop {
        match rx.recv_timeout(TICK) {
            Ok(Request::Press { press, done }) => {
                serve(&mut driver, &mut surfaces, &press);
                // The press may have stopped waiting.
                let _ = done.send(());
                // The menu's counts, after the press is answered: reading
                // them reads every installed plugin.
                surfaces.menu = driver.rekindle().menu();
            }
            Ok(Request::Filter { t, d, done }) => {
                // The dispatch may have stopped waiting.
                let _ = done.send(driver.rekindle().gateway_filter(&t, d));
            }
            Ok(Request::Events(passed)) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as u64);
                for p in passed {
                    let n = driver.rekindle().gateway_events(&p.t, p.d(), now);
                    received += 1;
                    if received <= EVENTS_LOGGED || received.is_multiple_of(EVENTS_LOGGED) {
                        log(&format!("gateway #{received}: {} -> {n} event(s)", p.t));
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        driver.tick();
        let kinds = driver.rekindle().intercepted_kinds();
        let bits = kinds.iter().fold(0u64, |bits, k| bits | (1 << (*k as u8)));
        if INTERCEPTED.swap(bits, Ordering::Relaxed) != bits {
            log(&format!("gateway: intercepted kinds now {kinds:?}"));
        }
        surfaces.ticks = surfaces.ticks.wrapping_add(1);
        if surfaces.ticks.is_multiple_of(DEVELOPER_MODE_EVERY) {
            let now = surfaces.home.developer_mode().unwrap_or(false);
            if now != surfaces.developer_mode {
                surfaces.developer_mode = now;
                surfaces.menu = driver.rekindle().menu();
            }
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
        Some(Click::Ui {
            plugin,
            element,
            value,
            ..
        }) => format!("{plugin}:{element}={value:?}"),
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
         recolors={} mounts={} page={} developer_mode={}",
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
        page_title(&state),
        surfaces.developer_mode,
    ));
}

/// The title of the page shown, for the log: the text of the page bar's
/// title, `-` for none.
fn page_title(state: &State) -> String {
    state
        .mounts
        .iter()
        .filter(|m| {
            m.nodes.iter().any(|n| {
                n.cells.iter().any(|c| {
                    c.prop == rekindle_wire::a11y::ids::ID
                        && c.value == rekindle_wire::Value::Str("rekindle.page".into())
                })
            })
        })
        .flat_map(|m| &m.nodes)
        // The bar's title is the page's last text.
        .rfind(|n| n.kind == rekindle_wire::ElementKind::TEXT)
        .and_then(|n| {
            n.cells
                .iter()
                .find(|c| c.prop == rekindle_wire_inject::TEXT)
        })
        .map_or_else(|| "-".into(), |c| format!("{:?}", c.value))
}

/// How many ticks the messages are read again after, with no new turn:
/// a renderer's settings may have changed.
const MESSAGES_EVERY: u32 = 20;

/// The cells the plugins' message renderers write on the messages the
/// guest shows. The guest's tree is read only after it sent a turn, or
/// every [`MESSAGES_EVERY`] ticks, and only while a plugin renders
/// messages.
fn message_cells(driver: &mut Driver, surfaces: &mut Surfaces) -> Vec<Write> {
    let Some(map) = surfaces.messages.clone() else {
        return Vec::new();
    };
    let renders = driver
        .rekindle()
        .infos()
        .iter()
        .any(|p| p.granted.iter().any(|c| c.as_str() == "render-messages"));
    if !renders {
        surfaces.read_at = None;
        surfaces.message_cells.clear();
        return Vec::new();
    }
    let turns = super::TURNS.load(Ordering::Relaxed);
    if surfaces.read_at == Some(turns) && !surfaces.ticks.is_multiple_of(MESSAGES_EVERY) {
        return surfaces.message_cells.clone();
    }
    surfaces.read_at = Some(turns);
    let shown = super::with_engine(|e| crate::messages::shown(e.guest(), &map)).unwrap_or_default();
    let ids: Vec<(u64, String)> = shown.iter().map(|m| (m.id, m.content.clone())).collect();
    let looks = driver.render_messages(&ids);
    let cells: Vec<Write> = shown
        .iter()
        .zip(&looks)
        .flat_map(|(m, look)| crate::messages::writes(&map, m, look))
        .collect();
    if cells != surfaces.message_cells {
        log(&format!(
            "messages: {} shown, {} with a look",
            shown.len(),
            looks.iter().filter(|l| !l.is_empty()).count()
        ));
    }
    surfaces.message_cells.clone_from(&cells);
    cells
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
    let home = super::home();
    std::fs::create_dir_all(&home)?;
    // A binding's tokens take their colours from the palette the guest
    // names. An older palette file lists the colours itself.
    let palettes = match super::theme::load() {
        Some(_) => vec![live_palette()],
        None => crate::layout::read_palette(&super::config_file(PALETTE)),
    };
    let names: BTreeSet<String> = match super::theme::load() {
        Some(b) => b.names().map(str::to_string).collect(),
        None => palettes
            .iter()
            .flat_map(|(_, t)| t.iter().map(|(name, _)| name.clone()))
            .collect(),
    };
    if names.is_empty() {
        anyhow::bail!("no palette in {}", super::config_file(PALETTE).display());
    }
    // A token's place in the client's theme value. With no theme function
    // to detour, only the names count: they are the tokens plugins can set.
    let tokens: Vec<&String> = names.iter().collect();
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

/// The client's colour for each token, as a palette for the Driver: the
/// palette the guest named last, by its theme.
fn live_palette() -> (String, Vec<(String, u32)>) {
    let values = super::theme::values()
        .into_iter()
        .map(|b| (b.token, b.value))
        .collect();
    (super::theme::shown(), values)
}

/// How often, in ticks, the theme report is made.
const THEME_REPORT_EVERY: u32 = 40;

/// The recolours for the plugins' theme in the palette the guest named
/// last. Plugins' reads of a token follow that palette too.
fn live_recolors(driver: &mut Driver, surfaces: &mut Surfaces) -> Vec<Recolor> {
    let named = super::theme::named();
    if surfaces.seeded != Some(named) {
        surfaces.seeded = Some(named);
        driver.use_framework_theme(vec![live_palette()]);
    }
    super::theme::set_overrides(
        driver
            .theme_overrides()
            .into_iter()
            .map(|(t, hsla)| (t, crate::wire::rgba(hsla)))
            .collect(),
    );
    let recolors = super::theme::recolors();
    // The report walks the guest's tree under the engine's lock.
    if !recolors.is_empty()
        && surfaces.ticks.is_multiple_of(THEME_REPORT_EVERY)
        && let Some(report) = super::with_engine(|e| super::theme::report(e))
        && report != surfaces.theme_report
    {
        log(&report);
        surfaces.theme_report = report;
    }
    recolors
}

/// Hand the engine what the plugins make now, if it changed.
fn publish(driver: &mut Driver, surfaces: &mut Surfaces) -> State {
    let running = running(driver);
    let mut mounts = Vec::new();
    let mut back = None;
    if let Some(look) = &surfaces.look {
        let panels = rekindle_ui_hook::panels();
        let widgets = panels
            .iter()
            .find(|p| p.plugin == manager::OWNER)
            .map(|p| p.widgets.clone())
            .unwrap_or_default();
        let menu = rows::section(look, &rows::menu_items(&surfaces.menu));
        // Any view but the grid is a page, and a plugin's page shows the
        // plugin's own panels under the Manager's rows.
        let page = (!rows::is_grid(&widgets)).then(|| {
            let shown = driver.rekindle().detail_of();
            let own: Vec<_> = panels
                .iter()
                .filter(|p| shown.as_deref() == Some(p.plugin.as_str()))
                .cloned()
                .collect();
            let mut page = rows::page(look, &widgets, &own, surfaces.block);
            // A page of another shape goes under the other block of ids,
            // so the old one is taken out before the new one is made.
            if let (Some(old), Some(new)) = (&surfaces.page, page.mounts.first())
                && !old.same_shape(new)
            {
                surfaces.block ^= 1;
                page = rows::page(look, &widgets, &own, surfaces.block);
            }
            page
        });
        surfaces.page = page.as_ref().and_then(|p| p.mounts.first().cloned());
        let modal = rekindle_ui_hook::modal_plugin().zip(rekindle_ui_hook::modal_panel());
        back = match (&modal, &surfaces.page) {
            (Some((plugin, _)), _) => Some(rows::modal_back(plugin)),
            (None, Some(_)) => Some(rows::page_back()),
            (None, None) => None,
        };
        let modal = modal.map(|(plugin, panel)| rows::modal(look, &plugin, &panel));
        let up = rekindle_ui_hook::live_toasts();
        let ids: Vec<u64> = up.iter().map(|t| t.id).collect();
        if ids != surfaces.toasts {
            surfaces.toasts = ids;
            surfaces.toast_sets = surfaces.toast_sets.wrapping_add(1);
            log(&format!(
                "toasts: {} up (set {})",
                up.len(),
                surfaces.toast_sets
            ));
        }
        let toasts = rows::toasts(look, &up, surfaces.toast_sets);
        let rendered = rows::join(
            std::iter::once(menu)
                .chain(page)
                .chain(modal)
                .chain(std::iter::once(toasts)),
        );
        mounts = rendered.mounts;
        surfaces.presses = rendered.presses;
    }
    if let Some(slot) = &surfaces.chat_bar {
        mounts.extend(crate::wire::button_mounts(&driver.chat_bar_buttons(), slot));
    }
    let recolors = if super::theme::binding().is_some() {
        live_recolors(driver, surfaces)
    } else {
        let palettes = driver.recolors();
        // Reading which palette the guest shows walks its whole tree under
        // the engine's lock, which the guest's emit waits on. One palette
        // needs no reading.
        let shown = if palettes.len() > 1 {
            super::with_engine(|e| crate::wire::shown_palette(e.guest(), &palettes)).flatten()
        } else {
            None
        };
        crate::wire::palette_recolors(&palettes, shown)
    };
    let cells = message_cells(driver, surfaces);
    let state = State {
        recolors,
        running,
        mounts,
        back,
        cells,
    };
    super::set_plugins(state.clone());
    state
}
