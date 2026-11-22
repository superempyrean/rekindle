//! `compose` plugins on the client's send path: a detour of its send
//! method hands each message to rekindle's worker and sends what comes
//! back.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use rekindle_hook::{Detour, InlineDetour};

use crate::Hook;

/// Recovered description of the client's send method: `fn(&mut self, text:
/// String) -> _`, and where `self` keeps the current channel.
#[derive(Clone, Copy, Debug)]
pub struct ComposeLayout {
    pub send_addr: usize,
    pub off_channel: usize,
}

/// A pending send handed to rekindle's worker: (request id, channel, text),
/// and its answer: the messages to send instead, in order.
struct ComposeSlot {
    next: u64,
    request: Option<(u64, u64, String)>,
    answer: Option<(u64, Option<Vec<String>>)>,
}

static COMPOSE: Mutex<ComposeSlot> = Mutex::new(ComposeSlot {
    next: 1,
    request: None,
    answer: None,
});
static COMPOSE_CV: std::sync::Condvar = std::sync::Condvar::new();

/// Ask the worker to rewrite `text` (UI-thread side): the messages to send
/// instead, in order (one for a rewrite, several for a split). `None`:
/// unchanged, or no answer within `timeout`.
fn request_compose(
    channel: u64,
    text: String,
    timeout: std::time::Duration,
) -> Option<Vec<String>> {
    let mut slot = COMPOSE.lock().unwrap();
    let id = slot.next;
    slot.next += 1;
    slot.request = Some((id, channel, text));
    slot.answer = None;
    COMPOSE_CV.notify_all();
    // While it waits, the UI thread answers the reads the compose plugins
    // make (they run on the worker, and the client is parked here).
    let deadline = std::time::Instant::now() + timeout;
    let answered = |s: &ComposeSlot| matches!(&s.answer, Some((a, _)) if *a == id);
    while !answered(&slot) {
        let now = std::time::Instant::now();
        if now >= deadline {
            break;
        }
        drop(slot);
        crate::client::service(std::time::Duration::ZERO);
        slot = COMPOSE.lock().unwrap();
        let wait = (deadline - now).min(std::time::Duration::from_millis(2));
        slot = COMPOSE_CV
            .wait_timeout_while(slot, wait, |s| !answered(s))
            .unwrap()
            .0;
    }
    slot.request = None; // a late answer is for a request nobody is waiting on
    match slot.answer.take() {
        Some((a, text)) if a == id => text,
        _ => None,
    }
}

/// Worker side: wait up to `wait` for a send, and answer it with `rewrite`
/// (which returns the messages to send instead, or `None` to leave it).
/// Doubles as the worker's tick sleep. Returns whether a send was served.
pub fn serve_compose(
    wait: std::time::Duration,
    rewrite: impl FnOnce(u64, &str) -> Option<Vec<String>>,
) -> bool {
    let slot = COMPOSE.lock().unwrap();
    let (mut slot, _) = COMPOSE_CV
        .wait_timeout_while(slot, wait, |s| s.request.is_none())
        .unwrap();
    let Some((id, channel, text)) = slot.request.take() else {
        return false;
    };
    drop(slot); // run the plugins without holding the lock
    let out = rewrite(channel, &text);
    let mut slot = COMPOSE.lock().unwrap();
    slot.answer = Some((id, out));
    COMPOSE_CV.notify_all();
    true
}

static TRAMP_SEND: AtomicUsize = AtomicUsize::new(0);
static SEND_CHANNEL_OFF: AtomicUsize = AtomicUsize::new(0);
static SEND_TIMEOUT_MS: AtomicUsize = AtomicUsize::new(0);

/// Install the send detour. Each send blocks the client's UI thread for up to
/// `timeout` while rekindle's worker answers with [`serve_compose`]; with no
/// answer in time the message goes out unchanged.
///
/// # Safety
///
/// `layout.send_addr` is the client's send method, as [`hooked_send`]
/// describes it, and `layout.off_channel` the offset of the channel id (a
/// `u64`) in its `self`.
pub unsafe fn install_compose(layout: ComposeLayout, timeout: std::time::Duration) -> Result<Hook> {
    SEND_CHANNEL_OFF.store(layout.off_channel, Ordering::SeqCst);
    SEND_TIMEOUT_MS.store(
        usize::try_from(timeout.as_millis()).unwrap_or(usize::MAX),
        Ordering::SeqCst,
    );
    // SAFETY: the caller's contract: `send_addr` has the signature
    // `hooked_send` has.
    let d = unsafe { InlineDetour.install(layout.send_addr, hooked_send as *const () as usize)? };
    TRAMP_SEND.store(d.trampoline(), Ordering::SeqCst);
    Ok(Hook::new(vec![d]))
}

/// `fn(&mut self, text: String) -> Option<ClientEvent>`: sret, self, and
/// the `String` by reference to the caller's copy (the callee drops it).
/// rekindle replaces that `String` before the client's own code runs.
///
/// # Safety
///
/// Called only by the client, in place of its send method, with its
/// arguments.
unsafe extern "C" fn hooked_send(ret: *mut u8, this: *mut u8, text: *mut String) -> *mut u8 {
    // SAFETY: `this` is the client's model, which holds the channel id at
    // the offset `install_compose` was given.
    let channel = unsafe {
        std::ptr::read_unaligned(
            this.add(SEND_CHANNEL_OFF.load(Ordering::SeqCst))
                .cast::<u64>(),
        )
    };
    let timeout = std::time::Duration::from_millis(SEND_TIMEOUT_MS.load(Ordering::SeqCst) as u64);
    // SAFETY: TRAMP_SEND holds the original, stored before the detour went
    // live, and this is its signature.
    let original: unsafe extern "C" fn(*mut u8, *mut u8, *mut String) -> *mut u8 =
        unsafe { std::mem::transmute(TRAMP_SEND.load(Ordering::SeqCst)) };
    // SAFETY: `text` is the caller's live `String` (same compiler and std).
    let current = unsafe { (*text).clone() };
    if let Some(mut parts) = request_compose(channel, current, timeout)
        && let Some(last) = parts.pop()
    {
        // A split: the parts before the last go through the client's own
        // send first, in order, each a String it takes and drops. What each
        // returns is the client's event for that send; rekindle has no drop
        // for it, so it is left unread (a small leak per part).
        for part in parts {
            let mut part = std::mem::ManuallyDrop::new(part);
            let mut event = [0u64; 64];
            // SAFETY: the client's send, with its own `self` and a `String`
            // it takes ownership of.
            unsafe { original(event.as_mut_ptr().cast(), this, &raw mut *part) };
        }
        // SAFETY: as above. The client's String is dropped here, with the
        // process allocator both sides use.
        unsafe { *text = last };
    }
    // SAFETY: the client's own arguments, with the text rekindle left.
    unsafe { original(ret, this, text) }
}
