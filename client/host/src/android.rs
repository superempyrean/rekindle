//! The hook points of a client that runs as an Android app.
//!
//! Render ops and host calls leave the guest through one method: `emit` on
//! the guest's `Box<dyn Transport>`, the first method of its vtable. rekindle
//! swaps the pointer in that vtable slot (anchor [`EMIT_SLOT`]), so every
//! call reaches [`hooked_emit`] first and no code is patched. A buffer led by
//! a `u32` 0 is a turn of render ops, and anything else is a host call.
//!
//! The host is loaded into the app before the client's own library, and
//! the client emits its first scene soon after that library is loaded. The
//! engine recolours only nodes whose `Create` it mirrored, so it has to see
//! those turns. [`run_worker`] swaps the slot in two steps:
//!
//! - As soon as the library is loaded, the slot's address comes from
//!   [`KNOWN_SLOTS`], keyed by the library's GNU build ID. The swap happens
//!   only if the slot holds the emit method the file records for that
//!   build. Nothing else is read and no anchor is resolved.
//! - Then the anchors resolve against the copy of the library. If they
//!   agree, the early swap stays. If they put the slot elsewhere, the hook
//!   moves there. Either way the build's slot and emit method are recorded
//!   in [`KNOWN_SLOTS`] for the next launch.
//!
//! A build that isn't in the file gets the hook only after the anchors
//! resolve, a few seconds in, and its first turns pass unseen.
//!
//! Each turn's op stream passes through rekindle's wire engine, the same
//! `rekindle_wire_inject::Engine` the desktop detour runs, configured by a
//! `Work` a test run names in [`WORK_ENV`]; without one the hook counts turns
//! and host calls and forwards them unchanged. Host calls always pass
//! through.
//!
//! Events reach the guest through one method too: the event method of the
//! runtime handle's vtable, which the JNI dispatch and `ldh_dispatch` both
//! call. rekindle swaps that slot (anchor [`EVENT_SLOT`]) the same way, so
//! every UI event and host frame passes `Engine::event` first
//! ([`hooked_dispatch`]). A press on one of rekindle's own nodes is consumed
//! there. If it flips one of the work's [`Switches`], the host sets the new
//! work and emits the engine's flush through the original emit before the
//! dispatch returns. A consumed event makes the guest render nothing, and an
//! idle guest sends no turns, so nothing else would carry the change.
//!
//! With a palette in the config dir, the plugin runtime runs in the app too
//! ([`runtime`]). The theme its plugins set becomes the engine's recolours
//! for the palette the guest names ([`theme`]), and rekindle's section on
//! the settings screen, the plugins' chat-bar buttons and a plugin's modal
//! become its mounts. A press on any of them goes to the runtime.
//!
//! The report and the running counts go to logcat (tag `rekindle`), since an
//! app has no console.

mod runtime;
mod theme;

use crate::gateway;
use std::ffi::{CString, c_char, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rekindle_wire_inject::{Engine, Handler, PluginPress, Switches, Verdict, Work};

use crate::hooks::Hooks;

/// The anchor of the vtable slot that holds the transport's emit method.
pub(crate) const EMIT_SLOT: &str = "wire_transport_emit_slot";
/// The anchor of the vtable slot that holds the runtime handle's event
/// method.
pub(crate) const EVENT_SLOT: &str = "wire_event_dispatch_slot";
/// A test run names a JSON file of `Work` the engine runs on the turns.
const WORK_ENV: &str = "REKINDLE_HOST_WIRE_WORK";
/// Where the loader leaves rekindle's files when no env var points at them:
/// the `DT_NEEDED` loader can't set env, so the host reads this dir. Override
/// with `REKINDLE_HOST_DIR`.
const CONFIG_DIR: &str = "/data/local/tmp/rekindle";
/// How long [`run_worker`] waits for the client's library to be mapped.
const IMAGE_WAIT: Duration = Duration::from_secs(60);

/// A file in rekindle's config dir ([`CONFIG_DIR`], or `REKINDLE_HOST_DIR`).
pub(crate) fn config_file(name: &str) -> std::path::PathBuf {
    let dir = std::env::var_os("REKINDLE_HOST_DIR")
        .map_or_else(|| std::path::PathBuf::from(CONFIG_DIR), Into::into);
    dir.join(name)
}

/// What emit returns: a 24-byte value, which both AArch64 and x86-64 return
/// through a pointer the caller passes (`x8`, or a hidden first argument).
#[repr(C)]
pub(crate) struct EmitResult([u64; 3]);

type EmitFn = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> EmitResult;
/// The event method: the handle's data, the event's bytes and length. Null
/// is success, anything else the guest's error.
type EventFn = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> *mut c_void;

/// A vtable slot rekindle swaps.
struct Slot {
    what: &'static str,
    /// The method the slot held. Stored before the swap, so the hook never
    /// sees 0.
    original: AtomicUsize,
    /// The swap in place, kept so a swap the anchors disagree with can be
    /// put back.
    swap: Mutex<Option<rekindle_hook::SlotSwap>>,
}

impl Slot {
    const fn new(what: &'static str) -> Slot {
        Slot {
            what,
            original: AtomicUsize::new(0),
            swap: Mutex::new(None),
        }
    }
}

static EMIT: Slot = Slot::new("emit");
static EVENT: Slot = Slot::new("event");
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
/// The transport the guest last emitted through, for [`flush`].
static TRANSPORT: AtomicUsize = AtomicUsize::new(0);

/// The engine, when a test run gave it `Work`. `None` leaves every turn
/// unchanged.
static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

/// The Android worker: build the engine, wait for the client's library,
/// swap the emit slot early if the build is known, then resolve the anchors
/// and check the swap against them. Returns the one-line report, which the
/// caller also writes to logcat.
pub(crate) fn run_worker() -> String {
    let mut out = crate::report_head();
    // Before the swap: the guest names its palette in its first host calls.
    if let Some(b) = theme::load() {
        crate::field(&mut out, "theme_binding", b.tokens.len());
    }
    // Before the swap, so the first turn the hook sees reaches the engine.
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
    let known = KnownSlots::load();
    let Some((loaded, early, early_event)) = wait_and_swap(&known) else {
        crate::field(&mut out, "error", "image_not_mapped");
        return out;
    };
    let build = loaded.build_id.as_deref().map(crate::hex);
    crate::field(&mut out, "build_id", build.as_deref().unwrap_or("none"));
    match &early {
        Early::Swapped(slot) => {
            log(&format!(
                "early swap: slot={slot:#x} turns_seen={}",
                TURNS.load(Ordering::Relaxed)
            ));
            crate::field(&mut out, "early_swap", format_args!("{slot:#x}"));
        }
        Early::Unknown => crate::field(&mut out, "early_swap", "skipped(unknown_build)"),
        Early::Mismatch(why) => {
            crate::field(&mut out, "early_swap", format_args!("skipped({why})"));
        }
    }
    let anchors_path = std::env::var_os(rekindle_install::env::ANCHORS)
        .map_or_else(|| config_file("anchors.json"), Into::into);
    let anchors_path = anchors_path.to_string_lossy().into_owned();
    let hooks = match Hooks::resolve(&anchors_path) {
        Ok(h) => h,
        Err(e) => {
            crate::field(
                &mut out,
                "error",
                format_args!("anchors:{}", crate::err(&e)),
            );
            return out;
        }
    };
    crate::report_anchors(&hooks, &mut out);
    match (&early, hooks.addr(EMIT_SLOT)) {
        (Early::Swapped(slot), Ok(r)) if *slot == r => crate::field(&mut out, "verify", "agreed"),
        (Early::Swapped(slot), Ok(_)) => {
            restore(&EMIT);
            crate::field(&mut out, "verify", format_args!("moved_from({slot:#x})"));
            report_install(&hooks, &mut out);
        }
        // The early swap checked the slot's method, so it stays.
        (Early::Swapped(_), Err(e)) => {
            crate::field(
                &mut out,
                "verify",
                format_args!("unresolved({})", crate::err(&e)),
            );
        }
        _ => report_install(&hooks, &mut out),
    }
    report_event_hook(&hooks, early_event, &mut out);
    if let (Some(id), Ok(slot), Ok(emit)) = (
        build,
        hooks.addr(EMIT_SLOT),
        hooks.addr("wire_transport_emit"),
    ) {
        let event = match (hooks.addr(EVENT_SLOT), hooks.addr("wire_event_dispatch")) {
            (Ok(s), Ok(m)) => Some((s - loaded.bias, m - loaded.bias)),
            _ => None,
        };
        if let Err(e) = known.learn(id, slot - loaded.bias, emit - loaded.bias, event) {
            crate::field(&mut out, "known_slots", crate::err(&e));
        }
    }
    out
}

/// Check the early event swap against the anchors, or swap the slot they
/// resolve, and add the result to the report.
fn report_event_hook(hooks: &Hooks, early: Option<usize>, out: &mut String) {
    match (early, hooks.addr(EVENT_SLOT)) {
        (Some(slot), Ok(r)) if slot == r => {
            crate::field(out, "event_hook", format_args!("{slot:#x}(early,agreed)"));
        }
        // The early swap checked the slot's method, so it stays.
        (Some(slot), Err(e)) => crate::field(
            out,
            "event_hook",
            format_args!("{slot:#x}(early,unresolved:{})", crate::err(&e)),
        ),
        (early, Ok(r)) => {
            if early.is_some() {
                restore(&EVENT);
            }
            match swap_in(&EVENT, r, hooked_dispatch as *const () as usize) {
                Ok(()) => crate::field(out, "event_hook", format_args!("{r:#x}")),
                Err(e) => crate::field(out, "event_hook", crate::err(&e)),
            }
        }
        (None, Err(e)) => crate::field(out, "event_hook", crate::err(&e)),
    }
}

/// Swap the slot the anchors resolve, and add the result to the report.
fn report_install(hooks: &Hooks, out: &mut String) {
    match install(hooks) {
        Ok(slot) => crate::field(out, "transport_hook", format_args!("{slot:#x}")),
        Err(e) => crate::field(out, "transport_hook", crate::err(&e)),
    }
}

/// The file in the config dir that records, per build of the client's
/// library, where its emit slot is.
const KNOWN_SLOTS: &str = "known-slots.json";

/// One build's emit slot and the emit method it holds, and its event slot
/// and event method, as virtual addresses in the library.
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct KnownSlot {
    slot: u64,
    emit: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    event_slot: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    event: Option<u64>,
}

/// [`KNOWN_SLOTS`], by build ID in hex.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct KnownSlots(std::collections::BTreeMap<String, KnownSlot>);

impl KnownSlots {
    fn load() -> Self {
        std::fs::read(config_file(KNOWN_SLOTS))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Record build `id`'s slots and the methods they hold, unless the file
    /// already holds them.
    fn learn(
        mut self,
        id: String,
        slot: usize,
        emit: usize,
        event: Option<(usize, usize)>,
    ) -> anyhow::Result<()> {
        let k = KnownSlot {
            slot: slot as u64,
            emit: emit as u64,
            event_slot: event.map(|(s, _)| s as u64),
            event: event.map(|(_, m)| m as u64),
        };
        if self.0.get(&id) == Some(&k) {
            return Ok(());
        }
        self.0.insert(id, k);
        // Written in place: SELinux lets the app write a file the shell
        // created in `/data/local/tmp`, but not create one there. A torn
        // write reads back as no entries, so the next launch swaps late and
        // writes it again.
        std::fs::write(config_file(KNOWN_SLOTS), serde_json::to_vec_pretty(&self)?)?;
        Ok(())
    }
}

/// What the early swap did.
enum Early {
    /// The slot at this address now holds [`hooked_emit`].
    Swapped(usize),
    /// [`KNOWN_SLOTS`] has no entry for the build, or it has no build ID.
    Unknown,
    /// The slot didn't hold the recorded emit method, or couldn't be
    /// swapped.
    Mismatch(String),
}

/// Wait for the client's library to be loaded, then swap the emit slot, and
/// the event slot when it is recorded, if [`KNOWN_SLOTS`] knows the build.
/// The event slot's address comes back too, if it was swapped.
///
/// The wait polls `dl_iterate_phdr` every millisecond. A poll made while the
/// linker loads the library blocks on the linker's lock and returns when the
/// load is done, so the swap follows the load closely. A hook on the
/// runtime's `android_dlopen_ext` would fire no earlier, since the slot
/// holds nothing until the linker has relocated it.
fn wait_and_swap(known: &KnownSlots) -> Option<(crate::image::Loaded, Early, Option<usize>)> {
    let deadline = Instant::now() + IMAGE_WAIT;
    let loaded = loop {
        if let Some(l) = crate::image::loaded_image() {
            break l;
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let entry = loaded
        .build_id
        .as_deref()
        .and_then(|id| known.0.get(&crate::hex(id)));
    let early = match entry {
        None => Early::Unknown,
        Some(k) => early_swap(
            &EMIT,
            loaded.bias,
            (k.slot, k.emit),
            hooked_emit as *const () as usize,
        ),
    };
    let event = entry
        .and_then(|k| Some((k.event_slot?, k.event?)))
        .and_then(|recorded| {
            match early_swap(
                &EVENT,
                loaded.bias,
                recorded,
                hooked_dispatch as *const () as usize,
            ) {
                Early::Swapped(at) => Some(at),
                _ => None,
            }
        });
    Some((loaded, early, event))
}

/// Swap the slot `recorded` names to `hook`, if it holds the method
/// `recorded` names. Both are virtual addresses in the library.
fn early_swap(which: &'static Slot, bias: usize, recorded: (u64, u64), hook: usize) -> Early {
    let (Some(slot), Some(method)) = (
        rekindle_hook::elf::slot_address(bias, recorded.0),
        rekindle_hook::elf::slot_address(bias, recorded.1),
    ) else {
        return Early::Mismatch("overflow".into());
    };
    // SAFETY: the recorded slot is in the library's `PT_GNU_RELRO` data,
    // which the linker has mapped and relocated (see `loaded_image`).
    let held = unsafe { std::ptr::read_volatile(slot as *const usize) };
    if held != method {
        return Early::Mismatch(format!("slot_holds_{held:#x}"));
    }
    match swap_in(which, slot, hook) {
        Ok(()) => Early::Swapped(slot),
        Err(e) => Early::Mismatch(crate::err(&e)),
    }
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
    let engine = Engine::new(work).map_err(|e| anyhow::anyhow!("{e}"))?;
    *lock() = Some(engine);
    *panel_lock() = Some(panel);
    Ok(true)
}

/// The file of [`Switches`] in the work's mounts, beside `work.json`.
const SWITCHES: &str = "switches.json";
/// Which switches are on, by element. Written in place like
/// [`KNOWN_SLOTS`], so the shell creates it.
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
        log(&format!(
            "plugins: running={:?} recolors={}",
            state.running,
            state.recolors.len()
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

/// Swap the emit slot the anchors resolve to [`hooked_emit`]. Returns the
/// slot's address.
pub(crate) fn install(hooks: &Hooks) -> anyhow::Result<usize> {
    let slot = hooks.addr(EMIT_SLOT)?;
    swap_in(&EMIT, slot, hooked_emit as *const () as usize)?;
    Ok(slot)
}

/// Swap the slot at `slot` to `hook`, which has the signature of the method
/// the slot holds: [`hooked_emit`] for [`EMIT`], [`hooked_dispatch`] for
/// [`EVENT`].
fn swap_in(which: &'static Slot, slot: usize, hook: usize) -> anyhow::Result<()> {
    let what = which.what;
    // SAFETY: the caller found the slot in this process's image: a mapped,
    // aligned pointer in the vtable's read-only data.
    let original = unsafe { std::ptr::read_volatile(slot as *const usize) };
    if original == 0 {
        anyhow::bail!("the {what} slot at {slot:#x} is empty");
    }
    which.original.store(original, Ordering::SeqCst);
    // SAFETY: the slot holds the method `hook` stands in for, which has the
    // same signature.
    let swap = unsafe { rekindle_hook::SlotSwap::install(slot, hook)? };
    if swap.original() != original {
        anyhow::bail!("the {what} slot at {slot:#x} changed while it was swapped");
    }
    // The hook stays for the life of the process, unless `restore` takes it
    // out.
    *which
        .swap
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(swap);
    Ok(())
}

/// Put back the method the swap in place replaced. A thread already inside
/// the hook still calls the original it read.
fn restore(which: &'static Slot) {
    let swap = which
        .swap
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(mut swap) = swap
        && let Err(e) = swap.restore()
    {
        log(&format!("restore: {}", crate::err(&e)));
    }
}

/// The report's counts: turns, turns rewritten, turns the engine failed on,
/// host calls passed through, events dispatched, events consumed, turns of
/// rekindle's own flushed, and the gateway's dispatches the guest received,
/// the plugins dropped and the plugins replaced.
pub(crate) fn counts() -> String {
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

/// The op stream `buf` carries, or `None` for a host call: a turn is led by
/// a `u32` 0.
fn op_stream(buf: &[u8]) -> Option<&[u8]> {
    let (head, ops) = buf.split_first_chunk::<4>()?;
    (u32::from_le_bytes(*head) == 0).then_some(ops)
}

/// Run `f` on the engine, catching a panic so it can't unwind into the
/// guest's frame. `None` when there is no engine or it panicked.
fn with_engine<T>(f: impl FnOnce(&mut Engine) -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lock().as_mut().map(f)))
        .ok()
        .flatten()
}

/// Called by the guest in place of its transport's emit method.
unsafe extern "C" fn hooked_emit(
    transport: *mut c_void,
    bytes: *const u8,
    len: usize,
) -> EmitResult {
    // SAFETY: the original was stored before the swap went live, and has
    // this signature.
    let original =
        unsafe { std::mem::transmute::<usize, EmitFn>(EMIT.original.load(Ordering::SeqCst)) };
    TRANSPORT.store(transport as usize, Ordering::Relaxed);
    // SAFETY: the guest passes `len` readable bytes.
    let buf = unsafe { std::slice::from_raw_parts(bytes, len) };
    let Some(ops) = op_stream(buf) else {
        let n = CALLS.fetch_add(1, Ordering::Relaxed);
        if n < 8 {
            log(&format!("host call #{n} len={len} head={}", head_hex(buf)));
        }
        if theme::on_call(buf) {
            retheme();
        }
        // SAFETY: the guest's own arguments.
        return unsafe { original(transport, bytes, len) };
    };
    let n = TURNS.fetch_add(1, Ordering::Relaxed);
    if n < 8 || n.is_multiple_of(256) {
        log(&format!("turn #{n} len={len} head={}", head_hex(buf)));
    }
    capture(n, ops);
    match with_engine(|e| e.filter(ops)) {
        Some(Ok(Some(rewritten))) => {
            REWRITTEN.fetch_add(1, Ordering::Relaxed);
            let mut framed = Vec::with_capacity(4 + rewritten.len());
            framed.extend_from_slice(&0u32.to_le_bytes());
            framed.extend_from_slice(&rewritten);
            // SAFETY: the guest's transport, with the rewritten turn, which
            // the host reads within the call.
            unsafe { original(transport, framed.as_ptr(), framed.len()) }
        }
        r => {
            if !matches!(r, Some(Ok(None)) | None) {
                FAILED.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: the guest's own arguments.
            unsafe { original(transport, bytes, len) }
        }
    }
}

/// Called by the host in place of the runtime handle's event method.
unsafe extern "C" fn hooked_dispatch(
    data: *mut c_void,
    bytes: *const u8,
    len: usize,
) -> *mut c_void {
    // SAFETY: the original was stored before the swap went live, and has
    // this signature.
    let original =
        unsafe { std::mem::transmute::<usize, EventFn>(EVENT.original.load(Ordering::SeqCst)) };
    let n = DISPATCHES.fetch_add(1, Ordering::Relaxed);
    if n < 4 {
        log(&format!("dispatch #{n} len={len} tid={}", gettid()));
    }
    // A test run's press goes through the engine as the host's would.
    let _ = std::panic::catch_unwind(dispatch_test_press);
    // A test run's dispatches reach the guest ahead of this event, as the
    // platform's own would.
    for buf in std::panic::catch_unwind(take_test_dispatches).unwrap_or_default() {
        // SAFETY: the host's handle, with an event the guest decodes as it
        // decodes the platform's, read within the call.
        let _ = unsafe { deliver(data, original, &buf) };
    }
    // SAFETY: the host passes `len` readable bytes.
    let buf = unsafe { std::slice::from_raw_parts(bytes, len) };
    // SAFETY: the host's own handle and event.
    let r = unsafe { deliver(data, original, buf) };
    // What the runtime changed outside a turn, after the guest's own turns
    // for this event. A panic can't unwind into the host's frame.
    let _ = std::panic::catch_unwind(flush_pending);
    r
}

/// Hand `buf` to the guest's event method: past the plugins' event filters,
/// then the engine. The dispatches the guest received go to the runtime
/// for their events.
///
/// # Safety
///
/// `data` is the runtime handle's data and `original` its event method.
unsafe fn deliver(data: *mut c_void, original: EventFn, buf: &[u8]) -> *mut c_void {
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
            return std::ptr::null_mut();
        }
    };
    let r = match with_engine(|e| e.event(buf)) {
        Some(Ok(Verdict::Consume(_))) => {
            CONSUMED.fetch_add(1, Ordering::Relaxed);
            // A panic can't unwind into the host's frame.
            let _ = std::panic::catch_unwind(deliver_presses);
            // Success, and the guest never saw the event.
            std::ptr::null_mut()
        }
        Some(Ok(Verdict::Replace(ev))) => {
            let b = ev.to_bytes();
            // SAFETY: the host's handle, with an event the guest decodes as
            // it decodes the host's, read within the call.
            unsafe { original(data, b.as_ptr(), b.len()) }
        }
        Some(Ok(Verdict::Batch(b))) => {
            // A panic can't unwind into the host's frame.
            let _ = std::panic::catch_unwind(deliver_presses);
            // SAFETY: as above.
            unsafe { original(data, b.as_ptr(), b.len()) }
        }
        // SAFETY: the host's own handle, with its event or the one the
        // filters left, read within the call.
        _ => unsafe { original(data, buf.as_ptr(), buf.len()) },
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
fn deliver_presses() {
    let presses = with_engine(Engine::take_presses).unwrap_or_default();
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
}

/// Flush the engine's work if it changed outside a turn. The length of the
/// turn flushed, 0 for none.
fn flush_pending() -> usize {
    if !DIRTY.swap(false, Ordering::SeqCst) {
        return 0;
    }
    match with_engine(Engine::flush).flatten() {
        Some(ops) => {
            flush(&ops);
            ops.len()
        }
        None => 0,
    }
}

/// Send a turn of rekindle's own through the transport the guest last
/// emitted through, by the original emit.
fn flush(ops: &[u8]) {
    let transport = TRANSPORT.load(Ordering::Relaxed);
    let emit = EMIT.original.load(Ordering::SeqCst);
    if transport == 0 || emit == 0 {
        log("flush: no transport yet");
        return;
    }
    let mut framed = Vec::with_capacity(4 + ops.len());
    framed.extend_from_slice(&0u32.to_le_bytes());
    framed.extend_from_slice(ops);
    let n = FLUSHED.fetch_add(1, Ordering::Relaxed);
    // SAFETY: the original emit, with the transport the guest passed it and
    // a turn the host reads within the call.
    let r = unsafe {
        std::mem::transmute::<usize, EmitFn>(emit)(
            transport as *mut c_void,
            framed.as_ptr(),
            framed.len(),
        )
    };
    if n < 8 {
        log(&format!(
            "flush #{n} len={} tid={} result={:#x},{:#x},{:#x}",
            framed.len(),
            gettid(),
            r.0[0],
            r.0[1],
            r.0[2]
        ));
    }
}

fn gettid() -> i64 {
    // SAFETY: `gettid` takes no arguments and always succeeds.
    i64::from(unsafe { libc::gettid() })
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
pub(crate) fn poll_test_press() {
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
            deliver_presses();
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
pub(crate) fn poll_test_gateway() {
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

/// Write one line to logcat under the `rekindle` tag. Android apps have no
/// console, so the report and the counts go here.
pub(crate) fn log(msg: &str) {
    const ANDROID_LOG_INFO: i32 = 4;
    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(prio: i32, tag: *const c_char, text: *const c_char) -> i32;
    }
    let (Ok(tag), Ok(text)) = (CString::new("rekindle"), CString::new(msg)) else {
        return;
    };
    // SAFETY: both pointers are valid, NUL-terminated C strings for the call.
    unsafe {
        __android_log_write(ANDROID_LOG_INFO, tag.as_ptr(), text.as_ptr());
    }
}
