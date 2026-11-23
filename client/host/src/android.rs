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
//! The report and the running counts go to logcat (tag `rekindle`), since an
//! app has no console.

use std::ffi::{CString, c_char, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rekindle_wire_inject::{Engine, Work};

use crate::hooks::Hooks;

/// The anchor of the vtable slot that holds the transport's emit method.
pub(crate) const EMIT_SLOT: &str = "wire_transport_emit_slot";
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

/// The emit method the slot held. Stored before the swap, so the hook never
/// sees 0.
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static TURNS: AtomicU64 = AtomicU64::new(0);
static REWRITTEN: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);

/// The engine, when a test run gave it `Work`. `None` leaves every turn
/// unchanged.
static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

/// The Android worker: build the engine, wait for the client's library,
/// swap the emit slot early if the build is known, then resolve the anchors
/// and check the swap against them. Returns the one-line report, which the
/// caller also writes to logcat.
pub(crate) fn run_worker() -> String {
    let mut out = crate::report_head();
    // Before the swap, so the first turn the hook sees reaches the engine.
    match build_engine() {
        Ok(true) => crate::field(&mut out, "wire_engine", "on"),
        Ok(false) => crate::field(&mut out, "wire_engine", "off"),
        Err(e) => crate::field(&mut out, "wire_engine", crate::err(&e)),
    }
    if start_capture() {
        crate::field(&mut out, "capture", CAPTURE_FILE);
    }
    let known = KnownSlots::load();
    let Some((loaded, early)) = wait_and_swap(&known) else {
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
            restore();
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
    if let (Some(id), Ok(slot), Ok(emit)) = (
        build,
        hooks.addr(EMIT_SLOT),
        hooks.addr("wire_transport_emit"),
    ) && let Err(e) = known.learn(id, slot - loaded.bias, emit - loaded.bias)
    {
        crate::field(&mut out, "known_slots", crate::err(&e));
    }
    out
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

/// One build's emit slot and the emit method it holds, as virtual addresses
/// in the library.
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct KnownSlot {
    slot: u64,
    emit: u64,
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

    /// Record build `id`'s slot and emit method, unless the file already
    /// holds them.
    fn learn(mut self, id: String, slot: usize, emit: usize) -> anyhow::Result<()> {
        let k = KnownSlot {
            slot: slot as u64,
            emit: emit as u64,
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

/// Wait for the client's library to be loaded, then swap the emit slot if
/// [`KNOWN_SLOTS`] knows the build.
///
/// The wait polls `dl_iterate_phdr` every millisecond. A poll made while the
/// linker loads the library blocks on the linker's lock and returns when the
/// load is done, so the swap follows the load closely. A hook on the
/// runtime's `android_dlopen_ext` would fire no earlier, since the slot
/// holds nothing until the linker has relocated it.
fn wait_and_swap(known: &KnownSlots) -> Option<(crate::image::Loaded, Early)> {
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
        Some(k) => early_swap(loaded.bias, *k),
    };
    Some((loaded, early))
}

/// Swap the slot `k` records, if it holds the emit method `k` records.
fn early_swap(bias: usize, k: KnownSlot) -> Early {
    let (Some(slot), Some(emit)) = (
        rekindle_hook::elf::slot_address(bias, k.slot),
        rekindle_hook::elf::slot_address(bias, k.emit),
    ) else {
        return Early::Mismatch("overflow".into());
    };
    // SAFETY: the recorded slot is in the library's `PT_GNU_RELRO` data,
    // which the linker has mapped and relocated (see `loaded_image`).
    let held = unsafe { std::ptr::read_volatile(slot as *const usize) };
    if held != emit {
        return Early::Mismatch(format!("slot_holds_{held:#x}"));
    }
    match swap_in(slot) {
        Ok(()) => Early::Swapped(slot),
        Err(e) => Early::Mismatch(crate::err(&e)),
    }
}

/// Build the engine from the `Work` a test run names, if it names one. True
/// when an engine is in place.
fn build_engine() -> anyhow::Result<bool> {
    let path = std::env::var_os(WORK_ENV).map_or_else(|| config_file("work.json"), Into::into);
    if !path.exists() {
        return Ok(false);
    }
    let json = std::fs::read_to_string(&path).map_err(|e| {
        anyhow::anyhow!("can't read {}: {e}", std::path::Path::new(&path).display())
    })?;
    let work = Work::from_json(&json).map_err(|e| anyhow::anyhow!("{e}"))?;
    if work.is_empty() {
        return Ok(false);
    }
    let engine = Engine::new(work).map_err(|e| anyhow::anyhow!("{e}"))?;
    *lock() = Some(engine);
    Ok(true)
}

fn lock() -> std::sync::MutexGuard<'static, Option<Engine>> {
    ENGINE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The swap in place, kept so a swap the anchors disagree with can be put
/// back.
static SWAP: Mutex<Option<rekindle_hook::SlotSwap>> = Mutex::new(None);

/// Swap the emit slot the anchors resolve to [`hooked_emit`]. Returns the
/// slot's address.
pub(crate) fn install(hooks: &Hooks) -> anyhow::Result<usize> {
    let slot = hooks.addr(EMIT_SLOT)?;
    swap_in(slot)?;
    Ok(slot)
}

/// Swap the emit slot at `slot` to [`hooked_emit`].
fn swap_in(slot: usize) -> anyhow::Result<()> {
    // SAFETY: the caller found the slot in this process's image: a mapped,
    // aligned pointer in the vtable's read-only data.
    let original = unsafe { std::ptr::read_volatile(slot as *const usize) };
    if original == 0 {
        anyhow::bail!("the emit slot at {slot:#x} is empty");
    }
    ORIGINAL.store(original, Ordering::SeqCst);
    // SAFETY: the slot holds the emit method, whose signature is `EmitFn`,
    // as `hooked_emit`'s is.
    let swap =
        unsafe { rekindle_hook::SlotSwap::install(slot, hooked_emit as *const () as usize)? };
    if swap.original() != original {
        anyhow::bail!("the emit slot at {slot:#x} changed while it was swapped");
    }
    // The hook stays for the life of the process, unless `restore` takes it
    // out.
    *SWAP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(swap);
    Ok(())
}

/// Put back the method the swap in place replaced. A thread already inside
/// [`hooked_emit`] still calls the original it read.
fn restore() {
    let swap = SWAP
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
/// and host calls passed through.
pub(crate) fn counts() -> String {
    format!(
        "transport_turns={} transport_rewritten={} transport_failed={} transport_calls={}",
        TURNS.load(Ordering::Relaxed),
        REWRITTEN.load(Ordering::Relaxed),
        FAILED.load(Ordering::Relaxed),
        CALLS.load(Ordering::Relaxed),
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
    // SAFETY: ORIGINAL was stored before the swap went live, and has this
    // signature.
    let original = unsafe { std::mem::transmute::<usize, EmitFn>(ORIGINAL.load(Ordering::SeqCst)) };
    // SAFETY: the guest passes `len` readable bytes.
    let buf = unsafe { std::slice::from_raw_parts(bytes, len) };
    let Some(ops) = op_stream(buf) else {
        let n = CALLS.fetch_add(1, Ordering::Relaxed);
        if n < 8 {
            log(&format!("host call #{n} len={len} head={}", head_hex(buf)));
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
