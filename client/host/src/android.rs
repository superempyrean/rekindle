//! The hook points of a client that runs as an Android app.
//!
//! Render ops and host calls leave the guest through one method: `emit` on
//! the guest's `Box<dyn Transport>`, the first method of its vtable. rekindle
//! swaps the pointer in that vtable slot (anchor [`EMIT_SLOT`]), so every
//! call reaches [`hooked_emit`] first and no code is patched. A buffer led by
//! a `u32` 0 is a turn of render ops, and anything else is a host call.
//!
//! For now the hook counts what passes and forwards it unchanged. Running
//! the wire engine on the turns, and the event side (`ld_event`,
//! `ldh_dispatch`, `ldh_reply`), come with loading the host into the app.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::hooks::Hooks;

/// The anchor of the vtable slot that holds the transport's emit method.
pub(crate) const EMIT_SLOT: &str = "wire_transport_emit_slot";

/// What emit returns: a 24-byte value, which both AArch64 and x86-64 return
/// through a pointer the caller passes (`x8`, or a hidden first argument).
#[repr(C)]
pub(crate) struct EmitResult([u64; 3]);

type EmitFn = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> EmitResult;

/// The emit method the slot held. Stored before the swap, so the hook never
/// sees 0.
static ORIGINAL: AtomicUsize = AtomicUsize::new(0);
static TURNS: AtomicU64 = AtomicU64::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);

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

/// The report's counts: turns and host calls passed through.
pub(crate) fn counts() -> String {
    format!(
        "transport_turns={} transport_calls={}",
        TURNS.load(Ordering::Relaxed),
        CALLS.load(Ordering::Relaxed)
    )
}

/// Whether `buf` is a turn of render ops: led by a `u32` 0.
fn is_turn(buf: &[u8]) -> bool {
    buf.first_chunk::<4>() == Some(&[0; 4])
}

/// Called by the guest in place of its transport's emit method.
unsafe extern "C" fn hooked_emit(
    transport: *mut c_void,
    bytes: *const u8,
    len: usize,
) -> EmitResult {
    // SAFETY: the guest passes `len` readable bytes.
    let buf = unsafe { std::slice::from_raw_parts(bytes, len) };
    let counter = if is_turn(buf) { &TURNS } else { &CALLS };
    counter.fetch_add(1, Ordering::Relaxed);
    // SAFETY: ORIGINAL was stored before the swap went live, and has this
    // signature.
    let original = unsafe { std::mem::transmute::<usize, EmitFn>(ORIGINAL.load(Ordering::SeqCst)) };
    // SAFETY: the guest's own arguments.
    unsafe { original(transport, bytes, len) }
}
