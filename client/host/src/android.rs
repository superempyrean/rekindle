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
//! Each turn's op stream goes to [`crate::app`], which runs rekindle's wire
//! engine and the plugin runtime on it. Host calls always pass through.
//!
//! Events reach the guest through one method too: the event method of the
//! runtime handle's vtable, which the JNI dispatch and `ldh_dispatch` both
//! call. rekindle swaps that slot (anchor [`EVENT_SLOT`]) the same way, so
//! every UI event and host frame passes through [`crate::app::dispatch`]
//! first ([`hooked_dispatch`]). A turn of rekindle's own goes out through
//! the original emit ([`flush`]).
//!
//! The report and the running counts go to logcat (tag `rekindle`), since an
//! app has no console.

pub(crate) mod apk;

use std::ffi::{CString, c_char, c_void};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::app;
use crate::hooks::Hooks;

/// The anchor of the vtable slot that holds the transport's emit method.
pub(crate) const EMIT_SLOT: &str = "wire_transport_emit_slot";
/// The anchor of the vtable slot that holds the runtime handle's event
/// method.
pub(crate) const EVENT_SLOT: &str = "wire_event_dispatch_slot";
/// The dev config dir, seeded over adb: the `DT_NEEDED` loader can't set env,
/// so a build pushed with `adb push` leaves its files here. Used when no
/// bundled config is in the APK. Override the whole choice with
/// `REKINDLE_HOST_DIR`.
const CONFIG_DIR: &str = "/data/local/tmp/rekindle";
/// The app's own files, where a one-tap install keeps its config and plugin
/// home: `<this>/rekindle`. Writable by the app (the host runs as the app's
/// uid), unlike [`CONFIG_DIR`], so the host creates `known-slots.json` and
/// the handshake here instead of relying on the shell pre-creating them.
const APP_FILES: &str = "/data/data";
/// How long [`run_worker`] waits for the client's library to be mapped.
const IMAGE_WAIT: Duration = Duration::from_secs(60);

/// A file in rekindle's config dir. See [`config_dir`].
pub(crate) fn config_file(name: &str) -> PathBuf {
    config_dir().join(name)
}

/// rekindle's config dir, resolved once.
///
/// The order is:
/// 1. `REKINDLE_HOST_DIR`, when a test run or a dev launch sets it.
/// 2. The app's own `files/rekindle`, when the package is known. On the first
///    run it is seeded from the config baked into the APK
///    (`assets/rekindle/`, [`apk::seed_config`]), so a one-tap install carries
///    `anchors.json`, the first-party plugins and the themes with no adb push.
///    This is also the plugin home ([`runtime`]), so plugins and config share
///    one dir.
/// 3. The adb-seeded [`CONFIG_DIR`], when that holds an `anchors.json` and the
///    app dir doesn't (a dev build with no bundled config).
///
/// With no package and no env, [`CONFIG_DIR`] is the last resort.
pub(crate) fn config_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(resolve_config_dir).clone()
}

fn resolve_config_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("REKINDLE_HOST_DIR") {
        return p.into();
    }
    let tmp = PathBuf::from(CONFIG_DIR);
    let Some(app) = app_config_dir() else {
        return tmp;
    };
    if !app.join("anchors.json").exists() {
        if let Err(e) = std::fs::create_dir_all(&app) {
            log(&format!("config: can't create {}: {e}", app.display()));
            return tmp;
        }
        match apk::seed_config(&app) {
            Ok(0) => {}
            Ok(n) => log(&format!(
                "config: seeded {n} files from the APK into {}",
                app.display()
            )),
            Err(e) => log(&format!("config: no bundled config seeded: {e}")),
        }
    }
    if app.join("anchors.json").exists() {
        return app;
    }
    // A dev build seeded tmp over adb and bundled nothing in the APK.
    if tmp.join("anchors.json").exists() {
        return tmp;
    }
    // Nothing anywhere yet: the app dir is writable, so prefer it.
    app
}

/// `/data/data/<package>/files/rekindle`, from the process name, or `None`
/// off-device or when the package can't be read.
pub(crate) fn app_config_dir() -> Option<PathBuf> {
    let package = package()?;
    Some(PathBuf::from(format!(
        "{APP_FILES}/{package}/files/rekindle"
    )))
}

/// The plugin home: the app's own `files/rekindle`, the dir the config
/// resolves to ([`config_dir`]), so plugins and config live together. A dev
/// launch that points `REKINDLE_HOST_DIR` elsewhere puts the home there too.
fn plugin_home() -> PathBuf {
    if std::env::var_os("REKINDLE_HOST_DIR").is_some() {
        return config_dir();
    }
    app_config_dir().unwrap_or_else(config_dir)
}

/// The app's package name from `/proc/self/cmdline`, dropping any `:process`
/// suffix. `None` if it can't be read or isn't a plausible package.
pub(crate) fn package() -> Option<String> {
    let cmdline = std::fs::read("/proc/self/cmdline").ok()?;
    let first = cmdline.split(|b| *b == 0).next().unwrap_or_default();
    let name = std::str::from_utf8(first).ok()?.split(':').next()?;
    if name.is_empty() || name.contains('/') {
        return None;
    }
    Some(name.to_string())
}

/// What emit returns: a 24-byte value, which both AArch64 and x86-64 return
/// through a pointer the caller passes (`x8`, or a hidden first argument).
#[repr(C)]
pub(crate) struct EmitResult([u64; 3]);

impl EmitResult {
    /// `Ok(None)`: the call made, with no REPLY.
    const NONE: EmitResult = EmitResult([u64::MAX, 0, 0]);

    /// The REPLY to a host call: `Ok(Some(Vec<u8>))`, laid out as
    /// `{cap, ptr, len}`. `Ok(None)` is a `cap` of `-1` and an error `-2`
    /// (the emit at `0x5ae4d2c` in the `x86_64` library of 356207).
    fn reply(&self) -> Option<&[u8]> {
        let [cap, ptr, len] = self.0;
        if cap >= (1 << 63) || ptr == 0 || len > cap || len > (1 << 28) {
            return None;
        }
        // SAFETY: an `Ok(Some(_))` result holds a live `Vec<u8>` of `len`
        // bytes at `ptr`, which the caller owns until it drops it.
        Some(unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize) })
    }
}

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
/// Turns of rekindle's own sent through [`flush`].
static FLUSHED: AtomicU64 = AtomicU64::new(0);
/// The transport the guest last emitted through, for [`flush`].
static TRANSPORT: AtomicUsize = AtomicUsize::new(0);

/// The Android worker: build the engine, wait for the client's library,
/// swap the emit slot early if the build is known, then resolve the anchors
/// and check the swap against them. Returns the one-line report, which the
/// caller also writes to logcat.
pub(crate) fn run_worker() -> String {
    let mut out = crate::report_head();
    app::set_platform(app::Platform {
        config_dir: config_dir(),
        home: plugin_home(),
        log,
        emit: flush,
        call: host_call,
    });
    // Before the swap: the guest names its palette in its first host calls,
    // and the first turn the hook sees has to reach the engine.
    out.push_str(&app::start());
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
                app::turns()
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

/// The op stream `buf` carries, or `None` for a host call: a turn is led by
/// a `u32` 0.
fn op_stream(buf: &[u8]) -> Option<&[u8]> {
    let (head, ops) = buf.split_first_chunk::<4>()?;
    (u32::from_le_bytes(*head) == 0).then_some(ops)
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
        // A panic can't unwind into the guest's frame.
        let call = std::panic::catch_unwind(|| app::host_call(buf)).unwrap_or(app::Call::Pass);
        let result = match call {
            app::Call::Drop => return EmitResult::NONE,
            // SAFETY: the guest's transport, with a call the host reads
            // within the call.
            app::Call::Replace(new) => unsafe { original(transport, new.as_ptr(), new.len()) },
            // SAFETY: the guest's own arguments.
            app::Call::Pass => unsafe { original(transport, bytes, len) },
        };
        if let Some(reply) = result.reply() {
            let _ = std::panic::catch_unwind(|| app::host_reply(reply));
        }
        return result;
    };
    match std::panic::catch_unwind(|| app::turn(ops)).ok().flatten() {
        Some(rewritten) => {
            let mut framed = Vec::with_capacity(4 + rewritten.len());
            framed.extend_from_slice(&0u32.to_le_bytes());
            framed.extend_from_slice(&rewritten);
            // SAFETY: the guest's transport, with the rewritten turn, which
            // the host reads within the call.
            unsafe { original(transport, framed.as_ptr(), framed.len()) }
        }
        // SAFETY: the guest's own arguments.
        None => unsafe { original(transport, bytes, len) },
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
    // SAFETY: the host passes `len` readable bytes.
    let buf = unsafe { std::slice::from_raw_parts(bytes, len) };
    app::dispatch(buf, |b| match b {
        // SAFETY: the host's own handle, with its event or one the guest
        // decodes as it decodes the host's, read within the call.
        Some(b) => unsafe { original(data, b.as_ptr(), b.len()) },
        // Success, and the guest never saw the event.
        None => std::ptr::null_mut(),
    })
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

/// Make a host call of rekindle's own through the transport the guest last
/// emitted through, by the original emit, and return the REPLY. On the
/// guest's thread.
fn host_call(frame: &[u8]) -> Option<Vec<u8>> {
    let transport = TRANSPORT.load(Ordering::Relaxed);
    let emit = EMIT.original.load(Ordering::SeqCst);
    if transport == 0 || emit == 0 || op_stream(frame).is_some() {
        return None;
    }
    // SAFETY: the original emit, with the transport the guest passed it and
    // a CALL frame the host reads within the call.
    let result = unsafe {
        std::mem::transmute::<usize, EmitFn>(emit)(
            transport as *mut c_void,
            frame.as_ptr(),
            frame.len(),
        )
    };
    let reply = result.reply().map(<[u8]>::to_vec);
    if reply.is_some() && result.0[0] != 0 {
        // SAFETY: the reply is rekindle's to drop, and the library
        // allocates with the system allocator, which is `malloc`.
        unsafe { libc::free(result.0[1] as *mut c_void) };
    }
    reply
}

fn gettid() -> i64 {
    // SAFETY: `gettid` takes no arguments and always succeeds.
    i64::from(unsafe { libc::gettid() })
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
