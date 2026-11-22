//! Plugin clipboard writes, queued for the client's UI thread, and the
//! system clipboard as plain text (`CF_UNICODETEXT`), which is what gpui's
//! Windows backend reads and writes for a string `ClipboardItem`.

use std::sync::Mutex;

use crate::drive;

static CLIPBOARD: Mutex<Option<String>> = Mutex::new(None);

/// Queue `text` for the clipboard. It's written on the client's UI thread, by
/// the framework layer (or the overlay layer) on its next frame, as the
/// client's own copy actions are, so this needs one of them installed.
pub fn queue_clipboard(text: String) {
    *CLIPBOARD.lock().unwrap() = Some(text);
}

/// Drop the queued clipboard write, returning it.
pub fn take_clipboard() -> Option<String> {
    CLIPBOARD.lock().unwrap().take()
}

/// The clipboard write still waiting for a frame, if any (for self-tests).
pub fn pending_clipboard() -> Option<String> {
    CLIPBOARD.lock().unwrap().clone()
}

/// Write the queued text, if any (UI thread). While a drive runs, the drive
/// gets it instead, so a test never touches the user's clipboard.
pub(crate) fn deliver_queued() {
    let Some(text) = take_clipboard() else {
        return;
    };
    if drive::active() {
        drive::copied(text);
    } else {
        write(&text);
    }
}

/// Put `text` on the system clipboard. False if it couldn't be.
pub(crate) fn write(text: &str) -> bool {
    sys::write(text)
}

/// The system clipboard's text, if it holds any.
pub(crate) fn read() -> Option<String> {
    sys::read()
}

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;

    const CF_UNICODETEXT: u32 = 13;
    const GMEM_MOVEABLE: u32 = 0x0002;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn OpenClipboard(owner: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn SetClipboardData(format: u32, mem: *mut c_void) -> *mut c_void;
        fn GetClipboardData(format: u32) -> *mut c_void;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalAlloc(flags: u32, bytes: usize) -> *mut c_void;
        fn GlobalLock(mem: *mut c_void) -> *mut c_void;
        fn GlobalUnlock(mem: *mut c_void) -> i32;
        fn GlobalFree(mem: *mut c_void) -> *mut c_void;
    }

    pub(super) fn write(text: &str) -> bool {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: no owner window; a successful open is closed below.
        if unsafe { OpenClipboard(std::ptr::null_mut()) } == 0 {
            return false;
        }
        // SAFETY: the clipboard is open on this thread. `mem` is a fresh
        // allocation of `wide.len()` units, locked while it's written, and
        // freed unless the system took it.
        let ok = unsafe {
            EmptyClipboard();
            let mem = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
            let p = if mem.is_null() {
                std::ptr::null_mut()
            } else {
                GlobalLock(mem).cast::<u16>()
            };
            if p.is_null() {
                if !mem.is_null() {
                    GlobalFree(mem);
                }
                false
            } else {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
                GlobalUnlock(mem);
                // The system owns `mem` once SetClipboardData succeeds.
                let set = !SetClipboardData(CF_UNICODETEXT, mem).is_null();
                if !set {
                    GlobalFree(mem);
                }
                set
            }
        };
        // SAFETY: opened above.
        unsafe { CloseClipboard() };
        ok
    }

    pub(super) fn read() -> Option<String> {
        // SAFETY: no owner window; a successful open is closed below.
        if unsafe { OpenClipboard(std::ptr::null_mut()) } == 0 {
            return None;
        }
        // SAFETY: the clipboard is open on this thread, and the data it
        // returns for CF_UNICODETEXT is a null-terminated UTF-16 string,
        // read while it's locked.
        let text = unsafe {
            let mem = GetClipboardData(CF_UNICODETEXT);
            let p: *const u16 = if mem.is_null() {
                std::ptr::null()
            } else {
                GlobalLock(mem).cast()
            };
            if p.is_null() {
                None
            } else {
                let mut n = 0;
                while *p.add(n) != 0 {
                    n += 1;
                }
                let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
                GlobalUnlock(mem);
                Some(s)
            }
        };
        // SAFETY: opened above.
        unsafe { CloseClipboard() };
        text
    }
}

#[cfg(not(windows))]
mod sys {
    pub(super) fn write(_text: &str) -> bool {
        false
    }

    pub(super) fn read() -> Option<String> {
        None
    }
}
