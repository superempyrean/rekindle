//! The wire engine and the plugin runtime in a client that runs as a phone
//! app, whatever hooks reach them.
//!
//! The platform's hooks hand this module every turn the guest emits
//! ([`turn`]), every host call ([`host_call`]) and every event on its way
//! to the guest ([`dispatch`]). Each turn's op stream passes through
//! `rekindle_wire_inject::Engine`, configured by a `Work`: the file a test
//! run names in [`WORK_ENV`], or `work.json` in the config dir, with what
//! the switches and the plugin runtime add. Host calls always pass through.
//!
//! Every UI event and host frame passes `Engine::event` before the guest
//! receives it. A press on one of rekindle's own nodes is consumed there.
//! If it flips one of the work's [`Switches`], the new work is set and the
//! engine's flush goes out at once through [`Platform::emit`], before the
//! dispatch returns. A consumed event makes the guest render nothing, and
//! an idle guest sends no turns, so nothing else would carry the change.
//!
//! With a palette in the config dir, the plugin runtime runs too
//! ([`runtime`]). The theme its plugins set becomes the engine's recolours
//! for the palette the guest names ([`theme`]), and rekindle's section on
//! the settings screen, the plugins' chat-bar buttons and a plugin's modal
//! become its mounts. A press on any of them goes to the runtime.
//!
//! Text a plugin puts in the composer with `insert-text` is appended to the
//! guest's text field the user last focused or typed in, as a `forceValue`
//! in the next turn rekindle flushes (`Engine::insert_text`). With no such
//! field on screen, the field `wire.json` names as `composer` takes it.
//!
//! A plugin's slash command, typed in the composer and sent with the button
//! `wire.json` names as `send` or the field's own submit, never reaches the
//! guest. The runtime runs it, and its reply replaces the composer's text.
//! A reply only the user sees goes up as a toast.
//!
//! The plugins' context-menu rows go into the guest's own menu, as one
//! group where `wire.json`'s `menus` puts it. A press on one reaches the
//! plugin with the menu's target: the id under the list row whose event
//! opened the menu. The guest then receives a `dismiss` on the menu's sheet
//! in place of the press, and the menu closes.
//!
//! Android's hooks are `crate::android`. Another platform's hooks link this
//! crate as a library, call [`set_platform`] and then [`start`], and hand
//! over what the guest sends and receives.

mod runtime;
mod theme;

use crate::gateway;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use rekindle_wire_inject::{
    Composer, Engine, Handler, PluginPress, Switches, Typed, Verdict, Work,
};

/// A test run names a JSON file of `Work` the engine runs on the turns.
const WORK_ENV: &str = "REKINDLE_HOST_WIRE_WORK";

/// What the platform's hooks give this module.
pub struct Platform {
    /// rekindle's config dir: `palette.json`, `panel.json`, `wire.json`,
    /// `work.json` and a test run's files.
    pub config_dir: PathBuf,
    /// The plugin home the runtime installs and loads plugins from.
    pub home: PathBuf,
    /// Write one line to the platform's log.
    pub log: fn(&str),
    /// Send a turn of rekindle's own, an op stream led by the wire version
    /// byte, through the guest's transport. Called on the guest's thread,
    /// inside its dispatch.
    pub emit: fn(&[u8]),
}

static PLATFORM: OnceLock<Platform> = OnceLock::new();

/// Set the platform, once. False if it was already set.
pub fn set_platform(p: Platform) -> bool {
    PLATFORM.set(p).is_ok()
}

fn platform() -> Option<&'static Platform> {
    PLATFORM.get()
}

/// A file in the config dir.
pub(crate) fn config_file(name: &str) -> PathBuf {
    platform().map_or_else(|| PathBuf::from(name), |p| p.config_dir.join(name))
}

/// The plugin home.
fn home() -> PathBuf {
    platform().map_or_else(PathBuf::new, |p| p.home.clone())
}

/// Write one line to the platform's log. Nothing is written before
/// [`set_platform`].
pub fn log(msg: &str) {
    if let Some(p) = platform() {
        (p.log)(msg);
    }
}

static TURNS: AtomicU64 = AtomicU64::new(0);
static REWRITTEN: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);
static DISPATCHES: AtomicU64 = AtomicU64::new(0);
static CONSUMED: AtomicU64 = AtomicU64::new(0);
static FLUSHED: AtomicU64 = AtomicU64::new(0);
/// Set when the engine's work changed outside a turn, so the next dispatch
/// flushes it.
static DIRTY: AtomicBool = AtomicBool::new(false);

/// The engine, once [`start`] built one. `None` leaves every turn
/// unchanged.
static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

/// Build the engine and start the runtime, before the platform's hooks go
/// live: the guest names its palette in its first host calls, and the
/// engine has to see the first turn. Returns the report's fields, each
/// ` key=value`.
pub fn start() -> String {
    let mut out = String::new();
    if let Some(b) = theme::load() {
        crate::field(&mut out, "theme_binding", b.tokens.len());
    }
    match build_engine() {
        Ok(true) => {
            crate::field(&mut out, "wire_engine", "on");
            if runtime::wanted() {
                runtime::start();
                crate::field(&mut out, "runtime", "starting");
            }
        }
        Ok(false) => crate::field(&mut out, "wire_engine", "off"),
        Err(e) => crate::field(&mut out, "wire_engine", crate::err(&e)),
    }
    if start_capture() {
        crate::field(&mut out, "capture", CAPTURE_FILE);
    }
    out
}

/// A turn the guest emitted: its op stream, led by the wire version byte.
/// Returns the turn to send instead, or `None` to send it unchanged. On the
/// guest's thread.
pub fn turn(ops: &[u8]) -> Option<Vec<u8>> {
    let n = TURNS.fetch_add(1, Ordering::Relaxed);
    if n < 8 || n.is_multiple_of(256) {
        log(&format!(
            "turn #{n} len={} head={}",
            ops.len(),
            head_hex(ops)
        ));
    }
    capture(n, ops);
    match with_engine(|e| e.filter(ops)) {
        Some(Ok(Some(rewritten))) => {
            REWRITTEN.fetch_add(1, Ordering::Relaxed);
            Some(rewritten)
        }
        Some(Ok(None)) | None => None,
        Some(Err(_)) => {
            FAILED.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

/// A host call the guest made. If it names a palette, the plugins' theme is
/// recoloured for it from the guest's next turn. On the guest's thread.
pub fn host_call(buf: &[u8]) {
    let n = CALLS.fetch_add(1, Ordering::Relaxed);
    if n < 8 {
        log(&format!(
            "host call #{n} len={} head={}",
            buf.len(),
            head_hex(buf)
        ));
    }
    if theme::on_call(buf) {
        retheme();
    }
}

/// An event on its way to the guest. `original(Some(bytes))` hands the
/// guest an event and returns what the guest returns; `original(None)`
/// returns what the guest returns for success, with nothing handed over.
/// On the thread the platform calls the guest on.
pub fn dispatch<R>(buf: &[u8], mut original: impl FnMut(Option<&[u8]>) -> R) -> R {
    let n = DISPATCHES.fetch_add(1, Ordering::Relaxed);
    if n < 4 {
        log(&format!("dispatch #{n} len={}", buf.len()));
    }
    // A test run's press goes through the engine as the host's would.
    let _ = std::panic::catch_unwind(dispatch_test_press);
    // A test run's dispatches reach the guest ahead of this event, as the
    // platform's own would.
    for queued in std::panic::catch_unwind(take_test_dispatches).unwrap_or_default() {
        let _ = deliver(&queued, &mut original);
    }
    let r = deliver(buf, &mut original);
    // What the runtime changed outside a turn, after the guest's own turns
    // for this event. A panic can't unwind into the host's frame.
    let _ = std::panic::catch_unwind(flush_pending);
    r
}

/// Read a test run's files: a press, and gateway dispatches. The platform
/// calls it every second.
pub fn poll() {
    poll_test_press();
    poll_test_gateway();
    poll_tree_dump();
    poll_test_compose();
    poll_pick();
}

/// A picker asks for the next press by leaving `press` in this file in the
/// config dir, or for one node by its number, and `cancel` stops waiting.
/// The host clears the file, and writes the pick to [`PICK_ANSWER`] as JSON
/// (`rekindle_wire_inject::Pick`) once the press arrives. The press never
/// reaches the guest.
const PICK_REQUEST: &str = "pick.req";
const PICK_ANSWER: &str = "pick.json";

fn poll_pick() {
    let path = config_file(PICK_REQUEST);
    if let Ok(req) = std::fs::read_to_string(&path)
        && !req.trim().is_empty()
    {
        // Written in place, as the app can't create files in the config dir.
        let _ = std::fs::write(&path, "");
        let _ = std::fs::write(config_file(PICK_ANSWER), "");
        let req = req.trim();
        if req == "cancel" {
            with_engine(Engine::cancel_pick);
            log("pick: cancelled");
        } else if let Ok(node) = req.parse::<u32>() {
            let picked = with_engine(|e| e.pick_node(node)).flatten();
            write_pick(
                picked.as_ref(),
                &format!("no node {node} in the guest's tree"),
            );
        } else if with_engine(Engine::pick).is_some() {
            log("pick: waiting for a press");
        } else {
            write_pick(None, "no engine");
        }
    }
    if let Some(p) = with_engine(Engine::take_pick).flatten() {
        write_pick(Some(&p), "");
    }
}

fn write_pick(p: Option<&rekindle_wire_inject::Pick>, error: &str) {
    let json = match p {
        Some(p) => {
            log(&format!(
                "pick: #{} {} ({:?})",
                p.node, p.name, p.best.basis
            ));
            serde_json::to_string_pretty(p).unwrap_or_default()
        }
        None => {
            log(&format!("pick: {error}"));
            serde_json::json!({ "error": error }).to_string()
        }
    };
    let _ = std::fs::write(config_file(PICK_ANSWER), json);
}

/// A test run sets the text of the field `insert-text` would use by
/// writing it to this file in the config dir. The host reads it every
/// second and clears it, and the text goes out with the next dispatch.
const TEST_COMPOSE: &str = "compose.txt";

fn poll_test_compose() {
    let path = config_file(TEST_COMPOSE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let text = text.trim_end_matches(['\r', '\n']);
    if text.is_empty() {
        return;
    }
    let _ = std::fs::write(&path, "");
    let set = with_engine(|e| {
        let (field, _) = e.text_field()?;
        e.set_text(field, text);
        Some(field)
    })
    .flatten();
    match set {
        Some(field) => {
            DIRTY.store(true, Ordering::SeqCst);
            log(&format!("test compose {text:?} into #{field}"));
        }
        None => log(&format!("test compose {text:?}: no field")),
    }
}

/// A test run asks for the trees by leaving this file in the config dir:
/// the guest's tree goes to `guest.capture` and the host's, as rekindle's
/// turns left it, to `host.capture`, each one turn that creates the whole
/// tree, in `rekindle_wire::capture`'s format. Then the file is cleared.
const TREE_DUMP: &str = "tree.req";

fn poll_tree_dump() {
    let path = config_file(TREE_DUMP);
    if !std::fs::metadata(&path).is_ok_and(|m| m.len() > 0) {
        return;
    }
    let _ = std::fs::write(&path, "");
    let turns = with_engine(|e| (whole(e.guest()), whole(e.mirror())));
    let Some((guest, host)) = turns else {
        log("tree dump: no engine");
        return;
    };
    for (name, turn) in [("guest.capture", guest), ("host.capture", host)] {
        let written = rekindle_wire::capture::Writer::create(config_file(name))
            .and_then(|mut w| w.record(0, &turn.to_bytes()));
        log(&format!(
            "tree dump: {name} {} ops {}",
            turn.ops.len(),
            written.map_or_else(|e| e.to_string(), |()| "written".into())
        ));
    }
}

/// One turn that creates `tree` as it stands.
fn whole(tree: &rekindle_wire::Tree) -> rekindle_wire::Turn {
    use rekindle_wire::NodeOp;
    let mut turn = rekindle_wire::Turn::new();
    for n in tree.nodes() {
        turn.push(NodeOp::Create {
            node: n.id,
            kind: n.kind.clone(),
            props: rekindle_wire::turn::create_props(
                tree.props()
                    .node_props(n.id)
                    .map(|(p, v)| (p, v.clone()))
                    .collect(),
            ),
        });
    }
    for n in tree.nodes().filter(|n| !n.children.is_empty()) {
        turn.push(NodeOp::Children {
            node: n.id,
            children: n.children.clone(),
        });
    }
    if let Some(node) = tree.root() {
        turn.push(NodeOp::Root { node });
    }
    if let Some(node) = tree.focus() {
        turn.push(NodeOp::Focus { node: Some(node) });
    }
    turn
}

/// Build the engine from the `Work` a test run names, if it names one, or
/// for the plugin runtime. True when an engine is in place.
fn build_engine() -> anyhow::Result<bool> {
    let path: std::path::PathBuf =
        std::env::var_os(WORK_ENV).map_or_else(|| config_file("work.json"), Into::into);
    let base = if path.exists() {
        let json = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("can't read {}: {e}", path.display()))?;
        Work::from_json(&json).map_err(|e| anyhow::anyhow!("{e}"))?
    } else {
        Work::default()
    };
    let panel = Panel::load(base)?;
    let work = panel.work();
    if work.is_empty() && !runtime::wanted() {
        return Ok(false);
    }
    let mut engine = Engine::new(work).map_err(|e| anyhow::anyhow!("{e}"))?;
    engine.set_composer(composer(std::collections::BTreeSet::new()));
    *lock() = Some(engine);
    *panel_lock() = Some(panel);
    Ok(true)
}

/// The file of [`Switches`] in the work's mounts, beside `work.json`.
const SWITCHES: &str = "switches.json";
/// Which switches are on, by element. Written in place, so on Android the
/// shell creates it.
const SWITCH_STATE: &str = "switch-state.json";

/// The work as `work.json` gives it, the switches in it with their state,
/// and what the plugin runtime adds.
struct Panel {
    base: Work,
    switches: Switches,
    on: std::collections::BTreeMap<String, bool>,
    /// What the runtime last made of its plugins, once it runs.
    plugins: Option<runtime::State>,
}

impl Panel {
    fn load(base: Work) -> anyhow::Result<Panel> {
        let switches = match std::fs::read_to_string(config_file(SWITCHES)) {
            Ok(json) => Switches::from_json(&json).map_err(|e| anyhow::anyhow!("{e}"))?,
            Err(_) => Switches::default(),
        };
        let on = std::fs::read(config_file(SWITCH_STATE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Ok(Panel {
            base,
            switches,
            on,
            plugins: None,
        })
    }

    /// The work for the switches' state, with the plugins' recolours after
    /// the base work's. A plugin's switch (`on:<id>`) is on while the
    /// plugin runs.
    fn work(&self) -> Work {
        let mut on = self.on.clone();
        if let Some(p) = &self.plugins {
            for s in &self.switches.switches {
                if let Some(id) = plugin_switch(&s.element) {
                    on.insert(s.element.clone(), p.running.contains(id));
                }
            }
        }
        let mut work = self.switches.apply(&self.base, &on);
        if let Some(p) = &self.plugins {
            work.recolors.extend(p.recolors.iter().copied());
            work.cells.extend(p.cells.iter().cloned());
            work.rows.extend(p.rows.iter().cloned());
            work.mounts.extend(p.mounts.iter().cloned());
            work.back.clone_from(&p.back);
        }
        work
    }

    /// Flip the switch a press is for, and keep its state for the next
    /// launch. The new work, or `None` when the press isn't for a switch.
    fn press(&mut self, press: &PluginPress) -> Option<Work> {
        let s = self.switches.get(&press.element)?;
        let now = !self.on.get(&s.element).copied().unwrap_or(s.on);
        self.on.insert(s.element.clone(), now);
        log(&format!(
            "switch {} {}",
            s.element,
            if now { "on" } else { "off" }
        ));
        if let Ok(json) = serde_json::to_vec_pretty(&self.on)
            && let Err(e) = std::fs::write(config_file(SWITCH_STATE), json)
        {
            log(&format!("switch state not kept: {e}"));
        }
        Some(self.work())
    }
}

static PANEL: Mutex<Option<Panel>> = Mutex::new(None);

/// The plugin a switch element turns on and off: `on:<id>`, as the Manager
/// names its own switch for the plugin.
fn plugin_switch(element: &str) -> Option<&str> {
    element.strip_prefix("on:")
}

/// The runtime's new state: set the work it makes on the engine, to be
/// flushed on the next dispatch. Called on the runtime's thread.
fn set_plugins(state: runtime::State) {
    let work = {
        let mut panel = panel_lock();
        let Some(panel) = panel.as_mut() else {
            return;
        };
        if panel.plugins.as_ref() == Some(&state) {
            return;
        }
        if panel.plugins.as_ref().map(|p| &p.commands) != Some(&state.commands) {
            log(&format!("slash commands: {:?}", state.commands));
            let commands = state.commands.clone();
            with_engine(|e| e.set_composer(composer(commands)));
        }
        log(&format!(
            "plugins: running={:?} recolors={} value rules={}",
            state.running,
            state.recolors.len(),
            state.rows.len()
        ));
        panel.plugins = Some(state);
        panel.work()
    };
    set_work(work);
}

/// The guest named a palette: recolour the plugins' theme for it, from
/// the guest's next turn. On the guest's thread, within its call.
fn retheme() {
    let work = {
        let mut panel = panel_lock();
        let Some(panel) = panel.as_mut() else {
            return;
        };
        let Some(plugins) = panel.plugins.as_mut() else {
            return;
        };
        let recolors = theme::recolors();
        if plugins.recolors == recolors {
            return;
        }
        log(&format!(
            "theme: {} recolours for the new palette",
            recolors.len()
        ));
        plugins.recolors = recolors;
        panel.work()
    };
    set_work(work);
}

/// Turns the guest has emitted.
pub(crate) fn turns() -> u64 {
    TURNS.load(Ordering::Relaxed)
}

/// Set `work` on the engine, for the next dispatch to flush.
fn set_work(work: Work) {
    match with_engine(|e| e.set_work(work)) {
        Some(Ok(())) => DIRTY.store(true, Ordering::SeqCst),
        Some(Err(e)) => log(&format!("work refused: {e}")),
        None => {}
    }
}

fn panel_lock() -> std::sync::MutexGuard<'static, Option<Panel>> {
    PANEL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock() -> std::sync::MutexGuard<'static, Option<Engine>> {
    ENGINE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The report's counts: turns, turns rewritten, turns the engine failed on,
/// host calls passed through, events dispatched, events consumed, turns of
/// rekindle's own flushed, and the gateway's dispatches the guest received,
/// the plugins dropped and the plugins replaced.
pub fn counts() -> String {
    format!(
        "transport_turns={} transport_rewritten={} transport_failed={} transport_calls={} \
         dispatches={} consumed={} flushed={} gateway={} gateway_dropped={} gateway_replaced={}",
        TURNS.load(Ordering::Relaxed),
        REWRITTEN.load(Ordering::Relaxed),
        FAILED.load(Ordering::Relaxed),
        CALLS.load(Ordering::Relaxed),
        DISPATCHES.load(Ordering::Relaxed),
        CONSUMED.load(Ordering::Relaxed),
        FLUSHED.load(Ordering::Relaxed),
        GATEWAY.load(Ordering::Relaxed),
        GATEWAY_DROPPED.load(Ordering::Relaxed),
        GATEWAY_REPLACED.load(Ordering::Relaxed),
    )
}

/// Run `f` on the engine, catching a panic so it can't unwind into the
/// guest's frame. `None` when there is no engine or it panicked.
fn with_engine<T>(f: impl FnOnce(&mut Engine) -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lock().as_mut().map(f)))
        .ok()
        .flatten()
}

/// Hand `buf` to the guest: past the plugins' event filters, then the
/// engine. The dispatches the guest received go to the runtime for their
/// events.
fn deliver<R>(buf: &[u8], original: &mut impl FnMut(Option<&[u8]>) -> R) -> R {
    // A panic can't unwind into the host's frame.
    let filtered = std::panic::catch_unwind(|| gateway_filter(buf)).unwrap_or_default();
    let owned;
    let buf = match filtered.change {
        gateway::Change::Keep => buf,
        gateway::Change::Bytes(b) => {
            owned = b;
            owned.as_slice()
        }
        gateway::Change::Drop => {
            runtime::received(filtered.passed);
            // Success, and the guest never saw the event.
            return original(None);
        }
    };
    let r = match with_engine(|e| e.event(buf)) {
        Some(Ok(Verdict::Consume(_))) => {
            CONSUMED.fetch_add(1, Ordering::Relaxed);
            // A panic can't unwind into the host's frame.
            match std::panic::catch_unwind(deliver_presses).ok().flatten() {
                // A menu's row closes the menu.
                Some(instead) => original(Some(&instead)),
                // Success, and the guest never saw the event.
                None => original(None),
            }
        }
        Some(Ok(Verdict::Replace(ev))) => original(Some(&ev.to_bytes())),
        Some(Ok(Verdict::Batch(b))) => {
            // A panic can't unwind into the host's frame.
            let _ = std::panic::catch_unwind(deliver_presses);
            original(Some(&b))
        }
        // The host's own event, or the one the filters left.
        _ => original(Some(buf)),
    };
    runtime::received(filtered.passed);
    r
}

/// The gateway's dispatches in `buf`, past the plugins' event filters. While
/// the runtime isn't up, nothing is read.
fn gateway_filter(buf: &[u8]) -> gateway::Filtered {
    if !runtime::is_up() {
        return gateway::Filtered::default();
    }
    let f = gateway::filter(buf, &runtime::intercepts, &mut runtime::filter);
    let seen = GATEWAY.fetch_add(f.passed.len() as u64, Ordering::Relaxed);
    if seen < 8 {
        for p in &f.passed {
            log(&format!("gateway: {} ({} bytes)", p.t, p.message.len()));
        }
    }
    for (event, len) in &f.unread {
        if UNREAD.fetch_add(1, Ordering::Relaxed) < 8 {
            log(&format!(
                "gateway: a {event} socket message of {len} bytes isn't JSON"
            ));
        }
    }
    if !f.dropped.is_empty() {
        GATEWAY_DROPPED.fetch_add(f.dropped.len() as u64, Ordering::Relaxed);
        log(&format!("gateway: dropped {:?}", f.dropped));
    }
    GATEWAY_REPLACED.fetch_add(f.replaced as u64, Ordering::Relaxed);
    if let Some(s) = &f.socket {
        *lock_socket() = Some(s.clone());
    }
    f
}

static GATEWAY: AtomicU64 = AtomicU64::new(0);
static GATEWAY_DROPPED: AtomicU64 = AtomicU64::new(0);
static GATEWAY_REPLACED: AtomicU64 = AtomicU64::new(0);
static UNREAD: AtomicU64 = AtomicU64::new(0);
/// The socket the gateway's dispatches last arrived on.
static SOCKET: Mutex<Option<gateway::Socket>> = Mutex::new(None);

fn lock_socket() -> std::sync::MutexGuard<'static, Option<gateway::Socket>> {
    SOCKET
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Hand the presses the engine kept to the switches and the runtime, and
/// flush the work they make. On the guest's thread, inside its dispatch,
/// where the guest itself emits.
fn deliver_presses() -> Option<Vec<u8>> {
    for typed in with_engine(Engine::take_typed).unwrap_or_default() {
        run_typed(&typed);
    }
    let presses = with_engine(Engine::take_presses).unwrap_or_default();
    let mut instead = None;
    for p in &presses {
        let at = Instant::now();
        // A switch of `switches.json` flips here, unless it is a plugin's.
        let work = if plugin_switch(&p.element).is_none() {
            panel_lock().as_mut().and_then(|panel| panel.press(p))
        } else {
            None
        };
        if let Some(work) = work {
            set_work(work);
            flush_pending();
            continue;
        }
        if !runtime::handles(p) {
            log(&format!("press {}/{}/{}", p.plugin, p.surface, p.element));
            continue;
        }
        let up = panel_lock()
            .as_ref()
            .is_some_and(|panel| panel.plugins.is_some());
        if !up {
            log(&format!("press {}: the runtime isn't up", p.element));
            continue;
        }
        let menu = wire_map().and_then(|m| m.menus.get(&p.surface));
        if let Some(map) = menu {
            let target = with_engine(|e| menu_target(e, &p.surface, map)).flatten();
            log(&format!("menu {}: target {target:?}", p.surface));
            runtime::set_menu_target(target);
            instead = with_engine(|e| dismiss(e, map)).flatten();
        }
        // No lock is held while the runtime works, since it sets the
        // plugins' work itself.
        let answered = runtime::press(p);
        let waited = at.elapsed();
        let flushed = flush_pending();
        log(&format!(
            "press {}/{}/{}: answered={answered} wait={} us flushed={flushed} bytes total={} us",
            p.plugin,
            p.surface,
            p.element,
            waited.as_micros(),
            at.elapsed().as_micros(),
        ));
    }
    instead
}

/// What a menu of `slot` is about: the row of the list whose event opened
/// it, and the id the `accessibilityId` under that row carries.
fn menu_target(
    e: &Engine,
    slot: &str,
    map: &crate::wire::MenuMap,
) -> Option<rekindle_ui_hook::MenuTargetRef> {
    use rekindle_plugin_host::model::{SLOT_CHANNEL_MENU, SLOT_MESSAGE_MENU, SLOT_USER_MENU};
    use rekindle_ui_hook::MenuTargetRef;
    let ev = e.last_event(rekindle_wire::PropId::from_name(&map.opened_by)?)?;
    let tree = e.guest();
    let number = |v: Option<&rekindle_wire::Value>| match v? {
        rekindle_wire::Value::I32(n) => Some(i64::from(*n)),
        rekindle_wire::Value::I64(n) => Some(*n),
        rekindle_wire::Value::F64(f) => Some(*f as i64),
        _ => None,
    };
    let field = |name: &str| match &ev.value {
        rekindle_wire::Value::Object(props) => {
            let id = rekindle_wire::PropId::from_name(name)?;
            props.iter().find(|(p, _)| *p == id).map(|(_, v)| v)
        }
        rekindle_wire::Value::Map(entries) => {
            entries.iter().find(|(k, _)| k == name).map(|(_, v)| v)
        }
        _ => None,
    };
    let index = number(field("index"))?;
    let first = number(
        tree.props()
            .get(ev.node, rekindle_wire::PropId::from_name("first")?),
    )
    .unwrap_or(0);
    let row = *tree
        .node(ev.node)?
        .children
        .get(usize::try_from(index - first).ok()?)?;
    let id = id_under(tree, row, &map.target_prefix)?;
    match slot {
        SLOT_MESSAGE_MENU => Some(MenuTargetRef::Message {
            id,
            // Which channel and author aren't read off the wire yet.
            channel: 0,
            author: 0,
        }),
        SLOT_USER_MENU => Some(MenuTargetRef::User { id }),
        SLOT_CHANNEL_MENU => Some(MenuTargetRef::Channel { id }),
        _ => None,
    }
}

/// The id after `prefix` in an `accessibilityId` at or under `node`.
fn id_under(tree: &rekindle_wire::Tree, node: rekindle_wire::NodeId, prefix: &str) -> Option<u64> {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if let Some(rekindle_wire::Value::Str(aid)) =
            tree.props().get(n, rekindle_wire::a11y::ids::ID)
            && let Some(id) = aid.strip_prefix(prefix).and_then(|i| i.parse().ok())
        {
            return Some(id);
        }
        stack.extend(
            tree.node(n)
                .map(|x| x.children.iter().copied())
                .into_iter()
                .flatten(),
        );
    }
    None
}

/// The `dismiss` the guest receives on the menu's sheet, in place of the
/// press on a plugin's row, so the menu closes as when the user swipes it
/// away.
fn dismiss(e: &Engine, map: &crate::wire::MenuMap) -> Option<Vec<u8>> {
    let node = e.find(&map.sheet)?;
    Some(
        rekindle_wire::event::Event {
            node,
            name: rekindle_wire::PropId::from_name("dismiss")?,
            value: rekindle_wire::Value::Null,
        }
        .to_bytes(),
    )
}

/// Run a slash command the user sent from a field, and put what it replies
/// in the field: the text to insert or send, or nothing. A reply only the
/// user sees goes up as a toast, and the command stays in the field.
fn run_typed(typed: &Typed) {
    use rekindle_plugin_host::api::CommandReply;
    let at = Instant::now();
    let Some(reply) = runtime::slash(&typed.text) else {
        log(&format!(
            "slash {:?}: the runtime didn't answer",
            typed.text
        ));
        return;
    };
    let (text, what) = match &reply {
        CommandReply::None => (Some(String::new()), "none".to_string()),
        CommandReply::Insert(t) => (Some(t.clone()), format!("insert {t:?}")),
        // Sending goes through the guest's own send, which nothing here
        // reaches yet: the text waits in the composer for the user's send.
        CommandReply::Send(t) => (Some(t.clone()), format!("send {t:?}, left in the composer")),
        CommandReply::Local(spans) => {
            let text: String = spans.iter().map(|s| s.text.as_str()).collect();
            rekindle_ui_hook::push_toast(&text, rekindle_ui_hook::ToastStyle::Info);
            (None, format!("local {text:?}"))
        }
    };
    if let Some(text) = text {
        with_engine(|e| e.set_text(typed.field, &text));
        DIRTY.store(true, Ordering::SeqCst);
    }
    let flushed = flush_pending();
    log(&format!(
        "slash {:?} in #{} -> {what} in {} us, flushed={flushed} bytes",
        typed.text,
        typed.field,
        at.elapsed().as_micros()
    ));
}

/// Flush the engine's work if it changed outside a turn, with the text the
/// plugins put in the composer since. The length of the turn flushed, 0 for
/// none.
fn flush_pending() -> usize {
    insert_queued();
    if !DIRTY.swap(false, Ordering::SeqCst) {
        return 0;
    }
    match with_engine(Engine::flush).flatten() {
        Some(ops) => {
            match platform() {
                Some(p) => {
                    FLUSHED.fetch_add(1, Ordering::Relaxed);
                    (p.emit)(&ops);
                }
                None => log("flush: no platform"),
            }
            ops.len()
        }
        None => 0,
    }
}

/// The wire map in the config dir, read once: the composer, its send
/// button and the menus.
fn wire_map() -> Option<&'static crate::wire::WireMap> {
    static MAP: OnceLock<Option<crate::wire::WireMap>> = OnceLock::new();
    MAP.get_or_init(|| {
        let path = config_file("wire.json");
        path.exists()
            .then(|| crate::wire::read_wire_map(&path).ok())
            .flatten()
    })
    .as_ref()
}

/// The composer for the engine, with the slash commands the plugins run.
fn composer(commands: std::collections::BTreeSet<String>) -> Composer {
    Composer {
        field: wire_map().and_then(|m| m.composer.clone()),
        send: wire_map().and_then(|m| m.send.clone()),
        commands,
    }
}

/// Put the text plugins queued with `insert-text` into the guest's text
/// field, as a `forceValue` in the next turn rekindle flushes.
fn insert_queued() {
    let text = rekindle_ui_hook::take_inserts().concat();
    if text.is_empty() {
        return;
    }
    let done = with_engine(|e| {
        if let Some(i) = e.insert_text(&text) {
            return Ok(format!(
                "compose: {text:?} into input #{} ({:?}) at {}, {} chars now, seq {:#x} eventCount {:?}",
                i.node,
                i.found,
                i.force.start,
                i.force.text.chars().count(),
                i.force.seq,
                i.force.event_count
            ));
        }
        let a11y = rekindle_wire::a11y::ids::ID;
        let fields: Vec<String> = e
            .fields()
            .keys()
            .map(|n| format!("#{n} {:?}", e.guest().props().get(*n, a11y)))
            .collect();
        Err(format!(
            "compose: no field for {text:?}; inputs: {}",
            fields.join(", ")
        ))
    });
    match done {
        Some(Ok(line)) => {
            DIRTY.store(true, Ordering::SeqCst);
            log(&line);
        }
        Some(Err(line)) => {
            rekindle_ui_hook::push_toast(
                "There's no text field to put that in.",
                rekindle_ui_hook::ToastStyle::Warning,
            );
            log(&line);
        }
        None => log("compose: no engine"),
    }
}

/// A test run presses one of rekindle's own nodes by writing the element of
/// its handler to this file in the config dir, which the shell creates, or
/// raises the system back by writing [`TEST_BACK`]. The host reads it every
/// second and clears it. The press reaches the engine in
/// the next dispatch, on the guest's thread, as a press the host raised on
/// that node would, so a run can check the press path without input on the
/// device.
const TEST_PRESS: &str = "press.txt";
/// What [`TEST_PRESS`] holds to raise the system back.
const TEST_BACK: &str = "@back";

/// The test run's press, as event bytes, until a dispatch takes it.
static PRESS: Mutex<Option<Vec<u8>>> = Mutex::new(None);

/// Take the test run's press from [`TEST_PRESS`], if it holds one. Called
/// by the worker every second.
fn poll_test_press() {
    let path = config_file(TEST_PRESS);
    let Ok(element) = std::fs::read_to_string(&path) else {
        return;
    };
    let element = element.trim();
    if element.is_empty() {
        return;
    }
    // Written in place, as the app can't create files in the config dir.
    let _ = std::fs::write(&path, "");
    if element == TEST_BACK {
        log("test back");
        let mut back = rekindle_wire::event::UI.to_le_bytes().to_vec();
        back.extend([
            rekindle_wire::event::VERSION,
            rekindle_wire::event::ops::BACK,
        ]);
        *lock_press() = Some(back);
        return;
    }
    let node = with_engine(|e| {
        e.work()
            .mounts
            .iter()
            .flat_map(|m| &m.nodes)
            .find(|n| {
                matches!(&n.on_press, Some(Handler::Plugin { element: el, .. }) if el == element)
            })
            .map(|n| n.id)
    })
    .flatten();
    match node {
        Some(node) => {
            log(&format!("test press {element} on node {node:#x}"));
            *lock_press() = Some(rekindle_wire::event::Event::press(node).to_bytes());
        }
        None => log(&format!("test press {element}: no node has that handler")),
    }
}

fn lock_press() -> std::sync::MutexGuard<'static, Option<Vec<u8>>> {
    PRESS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Run the test run's press through the engine, if one waits.
fn dispatch_test_press() {
    let Some(bytes) = lock_press().take() else {
        return;
    };
    match with_engine(|e| e.event(&bytes)) {
        Some(Ok(Verdict::Consume(_))) => {
            CONSUMED.fetch_add(1, Ordering::Relaxed);
            if deliver_presses().is_some() {
                log("test press: a menu would close now");
            }
        }
        other => log(&format!("test press passed to nobody: {other:?}")),
    }
}

/// A test run hands the guest gateway dispatches by writing them to this
/// file in the config dir, one JSON object `{"t": ..., "d": ...}` a line.
/// The host reads it every second and clears it. Each reaches the guest in
/// the next dispatch, on the socket and with the sequence number of the
/// gateway's last dispatch, through the plugins' event filters as the
/// gateway's own would. A run can check the event and intercept path with
/// nothing sent to Discord.
const TEST_GATEWAY: &str = "gateway.txt";

/// The test run's dispatches, until a dispatch takes them.
static TEST_DISPATCHES: Mutex<Vec<(String, serde_json::Value)>> = Mutex::new(Vec::new());

/// Take the test run's dispatches from [`TEST_GATEWAY`]. Called by the
/// worker every second.
fn poll_test_gateway() {
    #[derive(serde::Deserialize)]
    struct Line {
        t: String,
        d: serde_json::Value,
    }
    let path = config_file(TEST_GATEWAY);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    if text.trim().is_empty() {
        return;
    }
    // Written in place, as the app can't create files in the config dir.
    let _ = std::fs::write(&path, "");
    let mut queued = TEST_DISPATCHES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<Line>(line) {
            Ok(l) => {
                log(&format!("test dispatch {} queued", l.t));
                queued.push((l.t, l.d));
            }
            Err(e) => log(&format!("test dispatch unreadable: {e}")),
        }
    }
}

/// The test run's dispatches as the platform's `@ws` events, once the
/// gateway's socket is known.
fn take_test_dispatches() -> Vec<Vec<u8>> {
    let mut queued = TEST_DISPATCHES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if queued.is_empty() {
        return Vec::new();
    }
    let Some(socket) = lock_socket().clone() else {
        log("test dispatches wait for the gateway's socket");
        return Vec::new();
    };
    queued
        .drain(..)
        .map(|(t, d)| {
            log(&format!("test dispatch {t} on socket {:?}", socket.id));
            gateway::socket_event(&socket, &gateway::dispatch_message(&t, &d, socket.seq))
        })
        .collect()
}

/// How many of the first turns [`CAPTURE`] records.
const CAPTURE_TURNS: u64 = 512;

/// The capture the first turns are recorded in, when a test run asks for
/// one by leaving a file named [`CAPTURE_FILE`] in the config dir. It is
/// `rekindle_wire::capture`'s format, which the wire inspector reads.
static CAPTURE: Mutex<Option<rekindle_wire::capture::Writer<std::fs::File>>> = Mutex::new(None);
const CAPTURE_FILE: &str = "wire.capture";

/// Start the capture if the config dir asks for one. True if it started.
fn start_capture() -> bool {
    let path = config_file(CAPTURE_FILE);
    if !path.exists() {
        return false;
    }
    let Ok(w) = rekindle_wire::capture::Writer::create(&path) else {
        return false;
    };
    *CAPTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(w);
    true
}

/// Record turn `n`'s op stream in the capture, if one is running.
fn capture(n: u64, ops: &[u8]) {
    if n >= CAPTURE_TURNS {
        return;
    }
    let mut c = CAPTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(w) = c.as_mut()
        && w.record(n, ops).is_err()
    {
        *c = None;
    }
}

/// The first bytes of a buffer, as hex, for a log line.
fn head_hex(buf: &[u8]) -> String {
    crate::hex(&buf[..buf.len().min(24)])
}
