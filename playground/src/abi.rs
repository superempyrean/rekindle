//! The functions the page calls on the wasm module.
//!
//! Bytes go in through a buffer the page asks for with `pg_alloc` and
//! fills. A call that returns text (a tree, an error) leaves it in the
//! output buffer, read with `pg_out_ptr` and `pg_out_len` before the next
//! call. A call that can fail returns 1, or 0 with the error in the
//! output buffer. Sessions are numbered from 1.

use std::cell::RefCell;

use crate::Session;

thread_local! {
    static SESSIONS: RefCell<Vec<Option<Session>>> = const { RefCell::new(Vec::new()) };
    static OUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn out(s: String) {
    OUT.with(|o| *o.borrow_mut() = s.into_bytes());
}

fn done(r: Result<(), String>) -> u32 {
    match r {
        Ok(()) => 1,
        Err(e) => {
            out(e);
            0
        }
    }
}

/// The `len` bytes at `ptr`, which the page filled after `pg_alloc`.
fn input(ptr: *mut u8, len: usize) -> Vec<u8> {
    // SAFETY: `ptr` came from `pg_alloc(len)`, which leaked a boxed slice
    // of `len` bytes; ownership comes back here once.
    unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)) }.into_vec()
}

fn with<T>(h: u32, f: impl FnOnce(&mut Session) -> T) -> Result<T, String> {
    SESSIONS.with(|s| {
        let mut s = s.borrow_mut();
        let slot = (h as usize)
            .checked_sub(1)
            .and_then(|i| s.get_mut(i))
            .and_then(Option::as_mut)
            .ok_or_else(|| format!("no session {h}"))?;
        Ok(f(slot))
    })
}

/// A buffer of `len` bytes for the next call's input. The call that takes
/// it frees it.
#[unsafe(no_mangle)]
pub extern "C" fn pg_alloc(len: usize) -> *mut u8 {
    Box::into_raw(vec![0u8; len].into_boxed_slice()).cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn pg_out_ptr() -> *const u8 {
    OUT.with(|o| o.borrow().as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn pg_out_len() -> usize {
    OUT.with(|o| o.borrow().len())
}

/// A session over the capture in the input buffer, every turn applied.
/// 0 on error.
#[unsafe(no_mangle)]
pub extern "C" fn pg_open(ptr: *mut u8, len: usize) -> u32 {
    match Session::new(&input(ptr, len)) {
        Ok(session) => SESSIONS.with(|s| {
            let mut s = s.borrow_mut();
            s.push(Some(session));
            s.len() as u32
        }),
        Err(e) => {
            out(e);
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pg_close(h: u32) {
    SESSIONS.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(slot) = (h as usize).checked_sub(1).and_then(|i| s.get_mut(i)) {
            *slot = None;
        }
    });
}

/// The number of turns in the capture.
#[unsafe(no_mangle)]
pub extern "C" fn pg_turns(h: u32) -> u32 {
    with(h, |s| s.turns() as u32).unwrap_or(0)
}

/// Replay the first `n` turns.
#[unsafe(no_mangle)]
pub extern "C" fn pg_replay(h: u32, n: u32) -> u32 {
    done(with(h, |s| s.replay(n as usize)))
}

/// Set the engine's work from the JSON in the input buffer.
#[unsafe(no_mangle)]
pub extern "C" fn pg_set_work(h: u32, ptr: *mut u8, len: usize) -> u32 {
    let json = String::from_utf8_lossy(&input(ptr, len)).into_owned();
    done(with(h, |s| s.set_work_json(&json)).and_then(|r| r))
}

/// Apply the theme in the input buffer (see [`crate::theme`]), with its
/// report in the output buffer.
#[unsafe(no_mangle)]
pub extern "C" fn pg_set_theme(h: u32, ptr: *mut u8, len: usize) -> u32 {
    let json = String::from_utf8_lossy(&input(ptr, len)).into_owned();
    done(with(h, |s| s.set_theme_json(&json).map(out)).and_then(|r| r))
}

/// The host's tree (`mirror` 1) or the guest's (0), as JSON in the output
/// buffer.
#[unsafe(no_mangle)]
pub extern "C" fn pg_tree(h: u32, mirror: u32) -> u32 {
    done(with(h, |s| out(s.tree_json(mirror != 0))))
}

/// The turns the engine refused, as JSON `[[turn, "why"], ...]`, in the
/// output buffer.
#[unsafe(no_mangle)]
pub extern "C" fn pg_errors(h: u32) -> u32 {
    done(with(h, |s| {
        let e: Vec<serde_json::Value> = s
            .errors()
            .iter()
            .map(|(t, why)| serde_json::json!([t, why]))
            .collect();
        out(serde_json::Value::Array(e).to_string());
    }))
}
