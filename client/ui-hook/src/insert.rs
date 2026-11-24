//! `insert-text`: queued text, put into the client's text field the user
//! last typed in.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rekindle_native::client::{self, Inserted};

use crate::toast::{ToastStyle, push_toast};

/// Queue `text` for the client's text field the user last typed in, at its
/// caret. It goes in on the client's UI thread, from the next frame, the way
/// the platform delivers typed text (through the field's input handler). If
/// focus has moved to rekindle's own UI (a plugin's search field), that field
/// is focused again first. Needs the framework or overlay layer, which keep
/// frames coming.
pub fn queue_insert(text: String) {
    INSERTS.lock().unwrap().push(text);
}

/// Take the queued text, for a client with no frame callback to put it in:
/// one whose fields are on the other side of a wire.
pub fn take_inserts() -> Vec<String> {
    std::mem::take(&mut *INSERTS.lock().unwrap())
}

/// How many queued inserts reached a field (for self-tests).
pub(crate) fn inserted_count() -> usize {
    INSERTED.load(Ordering::SeqCst)
}

static INSERTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// What the insert path holds now (diagnostics): queued texts and the field
/// it would focus.
pub(crate) fn insert_state() -> String {
    format!(
        "queued={};field={:?}",
        INSERTS.lock().unwrap().len(),
        *LAST_FIELD.lock().unwrap()
    )
}

/// How many times an insert had to focus the field first (for self-tests).
pub(crate) fn refocused_count() -> usize {
    REFOCUS_COUNT.load(Ordering::SeqCst)
}

static INSERTED: AtomicUsize = AtomicUsize::new(0);
static REFOCUS_COUNT: AtomicUsize = AtomicUsize::new(0);
/// The client's text field that last had the window's input handler.
static LAST_FIELD: Mutex<Option<u64>> = Mutex::new(None);
/// The queued text is waiting for the field rekindle just focused to paint.
static REFOCUSED: AtomicBool = AtomicBool::new(false);

/// Each frame, before it's drawn (a next-frame callback): note the client's
/// text field that has the input handler, and put queued text into it.
///
/// # Safety
///
/// `window` and `cx` are the client's live `Window` and `App`, on its UI
/// thread, outside a draw.
pub(crate) unsafe fn input_tick(window: *mut u8, cx: *mut u8) {
    // SAFETY: the caller's contract.
    if let Some(id) = unsafe { client::focused_input(window) } {
        *LAST_FIELD.lock().unwrap() = Some(id);
    }
    let text: String = std::mem::take(&mut *INSERTS.lock().unwrap()).concat();
    if text.is_empty() {
        return;
    }
    // SAFETY: the caller's contract.
    match unsafe { client::insert_text(window, cx, &text) } {
        Ok(Inserted::Inserted) => {
            REFOCUSED.store(false, Ordering::SeqCst);
            INSERTED.fetch_add(1, Ordering::SeqCst);
        }
        Ok(Inserted::NoField) => {
            let field = *LAST_FIELD.lock().unwrap();
            match field.filter(|_| !REFOCUSED.swap(true, Ordering::SeqCst)) {
                // Focus the field and try again once it has painted.
                // SAFETY: the caller's contract.
                Some(id) if unsafe { client::focus_id(window, cx, id) }.is_ok() => {
                    REFOCUS_COUNT.fetch_add(1, Ordering::SeqCst);
                    INSERTS.lock().unwrap().insert(0, text);
                }
                _ => {
                    REFOCUSED.store(false, Ordering::SeqCst);
                    push_toast("There's no text field to put that in.", ToastStyle::Warning);
                }
            }
        }
        Err(e) => {
            REFOCUSED.store(false, Ordering::SeqCst);
            push_toast(
                &format!("rekindle couldn't insert the text: {e}"),
                ToastStyle::Error,
            );
        }
    }
}
