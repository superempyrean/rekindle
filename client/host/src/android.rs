//! The hook points of a client that runs as an Android app.
//!
//! Render ops and host calls leave the guest through one method: `emit` on
//! the guest's `Box<dyn Transport>`, the first method of its vtable. rekindle
//! swaps the pointer in that vtable slot (anchor [`EMIT_SLOT`]), so every
//! call reaches [`hooked_emit`] first and no code is patched. A buffer led by
//! a `u32` 0 is a turn of render ops, and anything else is a host call.
//!
//! The host is loaded into the app before the client's own library, so
//! [`run_worker`] waits for that library to be mapped (its anchors resolve
//! against it) before it swaps the slot. Each turn's op stream then passes
//! through rekindle's wire engine, the same `rekindle_wire_inject::Engine`
//! the desktop detour runs, configured by a `Work` a test run names in
//! [`WORK_ENV`]; without one the hook counts turns and host calls and
//! forwards them unchanged. Host calls always pass through.
//!
//! The report and the running counts go to logcat (tag `rekindle`), since an
//! app has no console.

use std::ffi::{CString, c_void};
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
/// the DT_NEEDED loader can't set env, so the host reads this dir. Override
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

/// The Android worker: wait for the client's library, resolve the anchors
/// against it, swap the emit slot and run the engine on the turns. Returns
/// the one-line report, which the caller also writes to logcat.
pub(crate) fn run_worker() -> String {
    let mut out = crate::report_head();
    if !wait_for_image() {
        crate::field(&mut out, "error", "image_not_mapped");
        return out;
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
    match build_engine() {
        Ok(true) => crate::field(&mut out, "wire_engine", "on"),
        Ok(false) => crate::field(&mut out, "wire_engine", "off"),
        Err(e) => crate::field(&mut out, "wire_engine", crate::err(&e)),
    }
    match install(&hooks) {
        Ok(slot) => crate::field(&mut out, "transport_hook", format_args!("{slot:#x}")),
        Err(e) => crate::field(&mut out, "transport_hook", crate::err(&e)),
    }
    out
}

/// Wait until the client's image shows up in this process, so its anchors
/// resolve and the slot swap writes a mapped page. True once it is mapped.
fn wait_for_image() -> bool {
    let deadline = Instant::now() + IMAGE_WAIT;
    while Instant::now() < deadline {
        if crate::image::image_mapped() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    crate::image::image_mapped()
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

/// Swap the emit slot to [`hooked_emit`]. Returns the slot's address.
pub(crate) fn install(hooks: &Hooks) -> anyhow::Result<usize> {
    let slot = hooks.addr(EMIT_SLOT)?;
    // SAFETY: the anchor resolved against this process's image to the slot,
    // a mapped, aligned pointer in the vtable's read-only data.
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
    // The hook stays for the life of the process.
    std::mem::forget(swap);
    Ok(slot)
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
        fn __android_log_write(prio: i32, tag: *const i8, text: *const i8) -> i32;
    }
    let (Ok(tag), Ok(text)) = (CString::new("rekindle"), CString::new(msg)) else {
        return;
    };
    // SAFETY: both pointers are valid, NUL-terminated C strings for the call.
    unsafe {
        __android_log_write(ANDROID_LOG_INFO, tag.as_ptr(), text.as_ptr());
    }
}
