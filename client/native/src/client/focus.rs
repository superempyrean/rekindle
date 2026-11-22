//! Focus: handles rekindle holds, and the window's focused element.

use std::sync::Mutex;

use anyhow::{Context, Result};

use super::{layout, layout_or_err, read_checked};

/// A gpui `FocusHandle`, held by rekindle for the life of the process (never
/// dropped, so copies of its bytes stay valid). Elements track a clone made
/// by the client's own `FocusHandle::clone`. Its first word is the focus id.
#[derive(Clone, Copy)]
#[repr(C, align(8))]
pub struct Focus([u8; 32]);

impl Focus {
    /// Its focus id.
    pub(crate) fn id(&self) -> u64 {
        u64::from_ne_bytes(*self.0.first_chunk().expect("a 32-byte handle"))
    }

    /// Its bytes, as gpui's `&FocusHandle`.
    pub(crate) fn as_ptr(&self) -> *const u8 {
        self.0.as_ptr()
    }
}

impl std::fmt::Debug for Focus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Focus({:x})", self.id())
    }
}

impl PartialEq for Focus {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
    }
}

/// A new focus handle (`App::focus_handle`).
///
/// # Safety
/// `cx` must be the client's live `App`, on its UI thread.
pub unsafe fn new_focus(cx: *mut u8) -> Option<Focus> {
    let l = layout()?;
    if l.c.focus_handle_size != 32 {
        crate::fail("FocusHandle is not 32 bytes");
        return None;
    }
    let mut out = Focus([0; 32]);
    // SAFETY: the client's `App::focus_handle` (or the `FocusHandle::new` it
    // calls, on the app's focus map), with the caller's live app, writing a
    // 32-byte handle into `out`.
    unsafe {
        if l.fns.app_focus_handle != 0 {
            let f: unsafe fn(*mut u8, *mut u8) -> *mut u8 =
                std::mem::transmute(l.fns.app_focus_handle);
            f(out.0.as_mut_ptr(), cx);
        } else {
            let f: unsafe fn(*mut u8, usize) -> *mut u8 =
                std::mem::transmute(l.fns.focus_handle_new);
            f(out.0.as_mut_ptr(), cx.addr() + l.c.focus_handles);
        }
    }
    Some(out)
}

/// The window's focused element's focus id (`Window.focus`), if any.
///
/// # Safety
/// `window` must be the client's live `Window`, on its UI thread.
pub(super) unsafe fn focused_id(window: *const u8) -> Option<u64> {
    let l = layout()?;
    // SAFETY: the caller's contract; `focus` is the window's
    // `Option<FocusId>`, a slotmap key.
    let id = unsafe { read_checked::<u64>(window.addr() + l.c.focus, "the window's focus")? };
    // `None` is 0 in its first four bytes, the key's niche.
    (id as u32 != 0).then_some(id)
}

/// Whether `focus` is the window's focused handle.
///
/// # Safety
/// As [`focused_id`].
pub unsafe fn is_focused(window: *const u8, focus: &Focus) -> bool {
    // SAFETY: the caller's contract.
    unsafe { focused_id(window) }.is_some_and(|id| id == focus.id())
}

/// Focus the element with focus id `id` (from `focused_input`) through
/// gpui's `Window::focus`, which reads only the handle's id. The handle
/// passed is a copy of one rekindle owns with the id swapped, never dropped.
///
/// # Safety
/// `window` and `cx` must be the client's live `Window` and `App`, on its
/// UI thread, outside any draw; `id` must be a live focus id of this
/// window's app.
pub unsafe fn focus_id(window: *mut u8, cx: *mut u8, id: u64) -> Result<()> {
    static TEMPLATE: Mutex<Option<Focus>> = Mutex::new(None);
    let l = layout_or_err()?;
    let mut template = TEMPLATE.lock().unwrap();
    let mut handle = match *template {
        Some(h) => h,
        // SAFETY: the caller's contract.
        None => *template.insert(unsafe { new_focus(cx) }.context("couldn't make a focus handle")?),
    };
    handle.0[..8].copy_from_slice(&id.to_ne_bytes());
    // SAFETY: the client's `Window::focus`, with the caller's live window
    // and app and a handle whose bytes stay valid (rekindle never drops the
    // one they're copied from).
    unsafe {
        let f: unsafe fn(*mut u8, *const u8, *mut u8) = std::mem::transmute(l.fns.window_focus);
        f(window, handle.as_ptr(), cx);
    }
    Ok(())
}
