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
//! modal goes over the screen. The plugins' tabs go in the client's emoji
//! picker ([`crate::picker`]), and a tab the user picks shows its plugin's
//! panel there.
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
use rekindle_plugin_host::api::{ChannelRef, CommandReply};
use rekindle_plugin_host::home::Home;
use rekindle_plugin_host::manager;
use rekindle_plugin_host::model::{
    SLOT_CHANNEL_MENU, SLOT_CHAT_BAR_BUTTON, SLOT_EXPRESSION_PICKER_TAB, SLOT_MESSAGE_MENU,
    SLOT_USER_MENU,
};
use rekindle_ui_hook::{Click, UiInput, Widget};
use rekindle_wire_inject::{Handler, Mount, PluginPress, Recolor, Write};

use super::log;
use crate::gateway::{Decision, Passed};
use crate::messages::MessageMap;
use crate::rows::{self, Look, Press};
use crate::wire::{MenuMap, SlotMap};

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
    /// The plugins' slash commands, by name.
    pub(crate) commands: BTreeSet<String>,
    /// The rules that put the plugins' values of the theme's tokens that
    /// aren't colours on the guest's nodes, and the ones a plugin's tab in
    /// the emoji picker makes while it is shown.
    pub(crate) rows: Vec<rekindle_wire_inject::RowRule>,
    /// What happens to presses on the guest's nodes: its own tabs in the
    /// emoji picker while a plugin's tab is shown.
    pub(crate) presses: Vec<rekindle_wire_inject::PressRule>,
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
    /// The user sent this slash command from the composer: run it, and
    /// send back its reply.
    Slash {
        text: String,
        done: mpsc::SyncSender<CommandReply>,
    },
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
        crate::picker::TAB,
        crate::picker::BODY,
        crate::picker::GUEST_TAB,
        SLOT_MESSAGE_MENU,
        SLOT_USER_MENU,
        SLOT_CHANNEL_MENU,
    ]
    .contains(&p.surface.as_str())
}

/// What the next press on a plugin's menu row is about, as the guest's
/// thread read it off the tree before handing the press over.
static MENU_TARGET: Mutex<Option<rekindle_ui_hook::MenuTargetRef>> = Mutex::new(None);

/// Set what the next press on a menu row is about.
pub(crate) fn set_menu_target(target: Option<rekindle_ui_hook::MenuTargetRef>) {
    *MENU_TARGET
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = target;
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

/// Run the slash command `text` names, and wait up to [`PAGE_WAIT`] for
/// its reply. `None` if the runtime didn't answer in time or isn't running.
pub(crate) fn slash(text: &str) -> Option<CommandReply> {
    let (done, answered) = mpsc::sync_channel(1);
    let sent = send(Request::Slash {
        text: text.to_string(),
        done,
    });
    sent.then(|| answered.recv_timeout(PAGE_WAIT).ok())
        .flatten()
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
    /// Where each menu is drawn, by slot, when the wire map says.
    menus: BTreeMap<String, MenuMap>,
    /// The rules for the theme's tokens that aren't colours.
    values: crate::wire::ValueMap,
    /// The client's emoji picker, when the wire map says where it is.
    picker: Option<crate::picker::PickerMap>,
    /// The plugin's tab shown in the picker, as (plugin, tab id).
    picker_shown: Option<(String, String)>,
    /// The picker as the guest's tree had it when last read, and the turns
    /// the guest had sent then.
    picker_colors: Option<crate::picker::Colors>,
    picker_read_at: Option<u64>,
    /// The body shown, and which block of ids it has.
    picker_body: Option<Mount>,
    picker_block: usize,
    /// The plugins' values of those tokens when last logged.
    values_seen: Vec<(String, rekindle_plugin_host::model::ThemeValue)>,
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
        let (chat_bar, messages, menus, values, picker) = match map {
            Some(mut m) => (
                m.slots.remove(SLOT_CHAT_BAR_BUTTON),
                m.messages,
                m.menus,
                m.values,
                m.expression_picker,
            ),
            None => (None, None, BTreeMap::new(), BTreeMap::new(), None),
        };
        let developer_mode = home.developer_mode().unwrap_or(false);
        Surfaces {
            look,
            chat_bar,
            messages,
            menus,
            values,
            picker,
            picker_shown: None,
            picker_colors: None,
            picker_read_at: None,
            picker_body: None,
            picker_block: 0,
            values_seen: Vec::new(),
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
        if p.surface == crate::picker::TAB {
            return Some(Click::Slot {
                plugin: p.plugin.clone(),
                slot: SLOT_EXPRESSION_PICKER_TAB.into(),
                item: p.element.clone(),
            });
        }
        if self.menus.contains_key(&p.surface) {
            let target = MENU_TARGET
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()?;
            return Some(Click::Menu {
                plugin: p.plugin.clone(),
                item: p.element.clone(),
                target,
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
        "runtime up in {} ms: pulley={} plugins=[{}] developer_mode={} section={} chat_bar={} picker={}",
        started.elapsed().as_millis(),
        engine.map_or_else(|e| crate::err(&e), |p| p.to_string()),
        plugins.join(" "),
        surfaces.developer_mode,
        surfaces.look.is_some(),
        surfaces.chat_bar.is_some(),
        surfaces.picker.is_some(),
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
            Ok(Request::Events(passed)) => deliver_events(&mut driver, passed, &mut received),
            Ok(Request::Slash { text, done }) => {
                // The command may have stopped waiting.
                let _ = done.send(run_slash(&mut driver, &text));
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

/// Hand the dispatches the guest received to the plugins as events. The
/// first [`EVENTS_LOGGED`] are logged, then one in that many.
fn deliver_events(driver: &mut Driver, passed: Vec<Passed>, received: &mut u64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    for p in passed {
        let n = driver.rekindle().gateway_events(&p.t, p.d(), now);
        *received += 1;
        if *received <= EVENTS_LOGGED || received.is_multiple_of(EVENTS_LOGGED) {
            log(&format!("gateway #{received}: {} -> {n} event(s)", p.t));
        }
    }
}

/// Run one press: queue the click it reports, let the Driver take it, and
/// hand the engine what changed.
fn serve(driver: &mut Driver, surfaces: &mut Surfaces, p: &PluginPress) {
    let at = Instant::now();
    let before = running(driver);
    match p.surface.as_str() {
        crate::picker::TAB => surfaces.picker_shown = Some((p.plugin.clone(), p.element.clone())),
        crate::picker::GUEST_TAB => surfaces.picker_shown = None,
        _ => {}
    }
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
        Some(Click::Menu {
            plugin,
            item,
            target,
        }) => format!("menu {plugin}/{item} on {target:?}"),
        Some(Click::Slot { plugin, slot, item }) => format!("{slot} {plugin}/{item}"),
        _ if p.surface == crate::picker::GUEST_TAB => "picker: the client's own tab".into(),
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

/// Run the slash command `text` names. A command no plugin has, or options
/// it can't read, reply as a message only the user sees.
fn run_slash(driver: &mut Driver, text: &str) -> CommandReply {
    let commands = driver.slash_commands();
    let Some((plugin, cmd)) = rekindle_extension::slash::find(&commands, text) else {
        return local(&format!("No plugin has {text:?}."));
    };
    // Which channel the composer is in isn't read off the wire yet.
    let channel = ChannelRef {
        id: 0,
        name: String::new(),
        guild: None,
        kind: 0,
    };
    match rekindle_extension::slash::invocation(cmd, text, channel) {
        Ok(inv) => driver.rekindle().slash_command(plugin, &inv),
        Err(e) => local(&e),
    }
}

/// A reply only the user sees, of plain text.
fn local(text: &str) -> CommandReply {
    CommandReply::Local(vec![rekindle_plugin_host::api::Span {
        text: text.to_string(),
        bold: false,
        italic: false,
        code: false,
        color: None,
        target: None,
    }])
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
        slash_commands: true,
        wire_menus: true,
        wire_picker_tabs: picker_map().is_some(),
        wire_values: true,
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

/// The emoji picker in the wire map, if it has one.
fn picker_map() -> Option<crate::picker::PickerMap> {
    let path = super::config_file(WIRE_MAP);
    path.exists()
        .then(|| crate::wire::read_wire_map(&path).ok())
        .flatten()?
        .expression_picker
}

/// The plugins' tabs in the emoji picker, with the one the user picked
/// shown while the picker is open. The guest's tree is read again only
/// after it sent a turn.
fn picker(driver: &mut Driver, surfaces: &mut Surfaces) -> crate::picker::Drawn {
    let Some(map) = surfaces.picker.clone() else {
        return crate::picker::Drawn::default();
    };
    let tabs: Vec<crate::picker::Tab> = driver
        .picker_tabs()
        .into_iter()
        .map(|t| crate::picker::Tab {
            plugin: t.plugin,
            id: t.id,
            title: t.title,
        })
        .collect();
    if tabs.is_empty() {
        surfaces.picker_shown = None;
        return crate::picker::Drawn::default();
    }
    let turns = super::TURNS.load(Ordering::Relaxed);
    if surfaces.picker_read_at != Some(turns) {
        surfaces.picker_read_at = Some(turns);
        let read =
            super::with_engine(|e| crate::picker::read(e.guest(), &map, |s| e.find(s))).flatten();
        // The guest animates its tabs' text from a worklet, and writes their
        // `color` cells only when it creates them, so the colours are the
        // ones read when the picker opened.
        match (read, &surfaces.picker_colors) {
            (None, Some(_)) => {
                log("picker: closed");
                surfaces.picker_colors = None;
            }
            (Some(read), None) => {
                log(&format!("picker: open {read:?}"));
                surfaces.picker_colors = Some(read);
            }
            _ => {}
        }
    }
    let Some(colors) = surfaces.picker_colors.clone() else {
        // The picker closing takes a plugin's tab down.
        surfaces.picker_shown = None;
        surfaces.picker_body = None;
        return crate::picker::Drawn::default();
    };
    let shown = surfaces
        .picker_shown
        .as_ref()
        .and_then(|(plugin, id)| tabs.iter().position(|t| &t.plugin == plugin && &t.id == id));
    if shown.is_none() {
        surfaces.picker_shown = None;
    }
    let panels = rekindle_ui_hook::panels();
    let panel = shown.and_then(|i| {
        panels
            .iter()
            .find(|p| p.plugin == tabs[i].plugin && p.id == tabs[i].id)
    });
    let draw = |block| crate::picker::draw(&map, &colors, &tabs, shown.map(|i| (i, panel)), block);
    let mut drawn = draw(surfaces.picker_block);
    // A body of another shape goes under the other block of ids, so the
    // old one is taken out before the new one is made.
    let body = |d: &crate::picker::Drawn| shown.and_then(|_| d.mounts.last().cloned());
    if let (Some(old), Some(new)) = (&surfaces.picker_body, body(&drawn))
        && !old.same_shape(&new)
    {
        surfaces.picker_block ^= 1;
        drawn = draw(surfaces.picker_block);
    }
    surfaces.picker_body = body(&drawn);
    drawn
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

/// The rules that put the plugins' values of the theme's tokens that aren't
/// colours on the guest's nodes. A change of values is logged.
fn value_rules(driver: &Driver, surfaces: &mut Surfaces) -> Vec<rekindle_wire_inject::RowRule> {
    let values = driver.theme_values();
    let rows = crate::wire::value_rules(&values, &surfaces.values);
    if values != surfaces.values_seen {
        log(&format!("theme values: {values:?} -> {} rules", rows.len()));
        surfaces.values_seen = values;
    }
    rows
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
            // A tab's body in the emoji picker isn't one of the plugin's
            // panels on its page.
            let tabs = driver.picker_tabs();
            let own: Vec<_> = panels
                .iter()
                .filter(|p| shown.as_deref() == Some(p.plugin.as_str()))
                .filter(|p| !tabs.iter().any(|t| t.plugin == p.plugin && t.id == p.id))
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
    let drawn = picker(driver, surfaces);
    mounts.extend(drawn.mounts);
    if surfaces.look.is_none() {
        surfaces.presses.clear();
    }
    surfaces.presses.extend(drawn.clicks);
    let rows = driver.menu_items();
    for (nth, (slot, map)) in (0u32..).zip(&surfaces.menus) {
        let kind = match slot.as_str() {
            SLOT_MESSAGE_MENU => rekindle_ui_hook::MenuKind::Message,
            SLOT_USER_MENU => rekindle_ui_hook::MenuKind::User,
            SLOT_CHANNEL_MENU => rekindle_ui_hook::MenuKind::Channel,
            _ => continue,
        };
        let items: Vec<_> = rows.iter().filter(|r| r.kind == kind).cloned().collect();
        mounts.extend(crate::wire::menu_mount(&items, slot, nth, map));
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
    let commands = driver
        .slash_commands()
        .into_iter()
        .map(|(_, c)| c.name)
        .collect();
    let mut rows = value_rules(driver, surfaces);
    rows.extend(drawn.rows);
    let state = State {
        recolors,
        running,
        mounts,
        back,
        cells,
        commands,
        rows,
        presses: drawn.presses,
    };
    super::set_plugins(state.clone());
    state
}
