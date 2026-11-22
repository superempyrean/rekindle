//! The startup gate: the client waits for the host before it initialises.
//!
//! The host comes up on a worker thread, after the loader lock is released,
//! so the client's `main` runs alongside it. Left alone, the client builds its
//! registries (rekindle joins them by detouring their constructor) and paints
//! its first frame before the host's hooks exist, and rekindle misses both.
//! The client's `ole32!OleInitialize` import is pointed at a stand-in that
//! waits until the host reports ready (or has no host, or 20 s pass) and then
//! calls the real one. gpui calls it while creating its platform, on the main
//! thread, after the loader lock is released and before any of the client's
//! own setup.

use std::ffi::c_void;
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::{Hresult, STARTED, host, iat, sys, trace};

/// How long the client waits for the host at most.
const TIMEOUT: Duration = Duration::from_secs(20);

/// The client's own `OleInitialize`, from its import slot.
static ORIG_OLE_INITIALIZE: AtomicUsize = AtomicUsize::new(0);
static WAITED: Once = Once::new();

/// Point the client's `OleInitialize` import at the gate. Returns how many
/// import slots now point at it; a client that doesn't import it runs
/// ungated.
pub(crate) fn install() -> usize {
    let Some(base) = iat::exe_base() else {
        return 0;
    };
    // SAFETY: `base` is the client exe's image, mapped by the loader.
    let slots = unsafe { iat::slots(base, "ole32.dll", "OleInitialize") };
    let Some(&first) = slots.first() else {
        return 0;
    };
    // SAFETY: `first` is one of the image's import slots, which the loader
    // filled before running this library's initializer.
    ORIG_OLE_INITIALIZE.store(unsafe { first.read() }, Ordering::Release);
    slots
        .iter()
        .filter(|&&slot| {
            // SAFETY: `slot` is one of the image's import slots.
            unsafe { iat::write(slot, ole_initialize as *const () as usize) }.is_ok()
        })
        .count()
}

/// The client's `OleInitialize`, once the host is ready.
extern "system" fn ole_initialize(reserved: *mut c_void) -> Hresult {
    wait_for_host();
    // SAFETY: `install` stored the client's own `OleInitialize` before
    // pointing any slot here, and its signature is this function's.
    let real: extern "system" fn(*mut c_void) -> Hresult =
        unsafe { std::mem::transmute(ORIG_OLE_INITIALIZE.load(Ordering::Acquire)) };
    real(reserved)
}

/// Block until the host reports ready, has turned out not to load, or
/// [`TIMEOUT`] passes. Only the first call waits.
fn wait_for_host() {
    WAITED.call_once(|| {
        trace("gate: waiting for host");
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match STARTED.load(Ordering::Acquire) {
                1 => return trace("gate: no host"),
                2 => match host_ready() {
                    None => return trace("gate: host has no ready export"),
                    Some(true) => return trace("gate: host ready"),
                    Some(false) => {}
                },
                _ => {}
            }
            if Instant::now() >= deadline {
                return trace("gate: timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
}

/// What the loaded host's `rekindle_host_ready` export says, or `None` if
/// it has none.
fn host_ready() -> Option<bool> {
    let host = host::module();
    // SAFETY: `host` is the host's module handle, and the name is
    // NUL-terminated.
    let ready = unsafe { sys::GetProcAddress(host, c"rekindle_host_ready".as_ptr().cast()) };
    if ready.is_null() {
        return None;
    }
    // SAFETY: the host exports `rekindle_host_ready` as
    // `extern "system" fn() -> u32`.
    let ready: extern "system" fn() -> u32 = unsafe { std::mem::transmute(ready) };
    Some(ready() != 0)
}
