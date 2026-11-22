//! Scrolling divs, as gpui's `overflow_y_scroll()` makes them, with the
//! position kept by rekindle under a key. Each position is an
//! `Rc<RefCell<Point<Pixels>>>` allocation in gpui's layout that rekindle
//! holds one strong count of, so it lives for the process. Only the UI
//! thread touches them (the counts aren't atomic).

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::bytes::put_word;
use crate::layout::{Resolved, ScrollLayout};
use crate::mem;

/// The allocation of each scroll position, by key.
static SCROLLS: Mutex<BTreeMap<String, usize>> = Mutex::new(BTreeMap::new());

/// Whether this build's layout lets rekindle's elements scroll.
pub fn can_scroll() -> bool {
    crate::resolved().is_some_and(|r| r.scroll.is_some())
}

/// The scroll layout and the allocation under `key`, once an element has
/// used it.
fn position(key: &str) -> Option<(ScrollLayout, usize)> {
    let s = crate::resolved()?.scroll?;
    let rc = *SCROLLS.lock().unwrap().get(key)?;
    Some((s, rc))
}

/// The vertical scroll position under `key` (0 at the top, negative further
/// down, as gpui keeps it), once an element has used it.
///
/// # Safety
/// On the client's UI thread, the only one that writes the position.
pub unsafe fn scroll_y_position(key: &str) -> Option<f32> {
    let (s, rc) = position(key)?;
    // SAFETY: `rc` is a live allocation of `rc_point + 8` bytes (rekindle's
    // count keeps it), whose point's `y` is at `rc_point + 4`. The caller's
    // contract rules out a concurrent write.
    Some(unsafe { mem::read::<f32>(rc + s.rc_point + 4) })
}

/// Set the vertical scroll position under `key` (gpui clamps it on the next
/// layout). For tests, which can't send a wheel event. `false` if no
/// element has used the key yet, or gpui has the position borrowed.
///
/// # Safety
/// As [`scroll_y_position`].
pub unsafe fn set_scroll_y_position(key: &str, y: f32) -> bool {
    let Some((s, rc)) = position(key) else {
        return false;
    };
    // SAFETY: as in `scroll_y_position`; the `RefCell`'s borrow flag is at
    // `rc_borrow`, and 0 means nothing borrows the point.
    unsafe {
        if mem::read::<isize>(rc + s.rc_borrow) != 0 {
            return false;
        }
        mem::write(rc + s.rc_point + 4, y);
    }
    true
}

/// `.overflow_y_scroll()` with the position kept under `key`: the style's
/// `overflow.y`, and the `Rc` in the Div's `scroll_offset` with a strong
/// count for the Div, which gpui drops with it at the end of the frame. A
/// layout without the scroll entries leaves the div as it is.
pub(crate) fn attach(r: &Resolved, div: &mut [u8], style: &mut [u8], key: &str) {
    let Some(s) = r.scroll else {
        return;
    };
    style[s.overflow_y] = s.overflow_scroll;
    let rc = allocation(&s, key);
    // SAFETY: `rc` is rekindle's live `RcInner`, its strong count first.
    // Building elements happens on the UI thread, the only one that touches
    // the counts.
    unsafe { mem::write(rc, mem::read::<usize>(rc) + 1) };
    put_word(div, s.scroll_offset, rc);
}

/// The allocation of the scroll position under `key`, made the first time:
/// an `RcInner { strong, weak, value }` with rekindle's strong count and the
/// weak count every `Rc` starts with, and a zeroed `RefCell<Point>`.
fn allocation(s: &ScrollLayout, key: &str) -> usize {
    *SCROLLS
        .lock()
        .unwrap()
        .entry(key.to_string())
        .or_insert_with(|| {
            let p = mem::alloc(s.rc_point + 8, 8, true) as usize;
            // SAFETY: `p` is a fresh allocation of at least 32 bytes
            // (`resolve` checked `rc_point >= 24`).
            unsafe {
                mem::write(p, 1usize);
                mem::write(p + 8, 1usize);
            }
            p
        })
}
