//! Input: events built for and read from the client's
//! `Window::dispatch_event`, keys as listeners see them, and the hit test a
//! click on rekindle's elements goes by.

use std::cell::Cell;
use std::mem::ManuallyDrop;

use super::{Rect, check, layout, read_checked};
use crate::bytes::{Bytes, put, put_word};
use crate::layout::{ClientLayout, Resolved};
use crate::mem;

thread_local! {
    static PRESSED: Cell<Option<Rect>> = const { Cell::new(None) };
}

/// The bounds of the element whose click handler is running, in logical
/// pixels: what a handler needs to turn the pointer's position into a
/// position inside its element. `None` outside a click handler.
pub fn pressed_bounds() -> Option<Rect> {
    PRESSED.with(Cell::get)
}

/// Record `hitbox`'s bounds for [`pressed_bounds`], or clear them.
///
/// # Safety
/// `hitbox` is null or a live `&Hitbox`.
pub(crate) unsafe fn note_pressed(c: &ClientLayout, hitbox: *const u8) {
    let bounds = (!hitbox.is_null()).then(|| {
        // SAFETY: the caller's live hitbox; its bounds lie inside it
        // (checked by `resolve`).
        let [x, y, w, h] = unsafe { mem::read::<[f32; 4]>(hitbox.addr() + c.hitbox.1) };
        (x, y, w, h)
    });
    PRESSED.with(|p| p.set(bounds));
}

/// `Hitbox::is_hovered`, from the window's own state (the function is inlined
/// away in optimized builds): the hitbox has captured the pointer, or (last
/// input not from the keyboard) its id is among the first
/// `hover_hitbox_count` ids of the window's mouse hit test. `false` where
/// the window's memory isn't readable.
///
/// # Safety
/// `hitbox` must be a live `&Hitbox`, `window` the live window it belongs
/// to, on its UI thread.
pub(crate) unsafe fn hitbox_hovered(
    c: &ClientLayout,
    hitbox: *const u8,
    window: *const u8,
) -> bool {
    // SAFETY: the caller's contract.
    unsafe { hovered(c, hitbox, window) }.unwrap_or(false)
}

/// [`hitbox_hovered`], `None` where the window's memory isn't readable.
///
/// # Safety
/// As [`hitbox_hovered`].
unsafe fn hovered(c: &ClientLayout, hitbox: *const u8, window: *const u8) -> Option<bool> {
    // SAFETY: the caller's live hitbox; its id lies inside it (checked by
    // `resolve`).
    let id = unsafe { mem::read::<u64>(hitbox.addr() + c.hitbox_id) };
    let w = window.addr();
    // SAFETY: the caller's live window: its captured hitbox
    // (`Option<HitboxId>`: a u32 tag, the id at +8), last input modality (a
    // byte) and mouse hit test.
    unsafe {
        let [tag, captured] =
            read_checked::<[u64; 2]>(w + c.captured_hitbox, "the captured hitbox")?;
        if tag as u32 == c.option_some_tag && captured == id {
            return Some(true);
        }
        let modality = read_checked::<u8>(w + c.last_input_modality, "the last input modality")?;
        if modality == c.modality_keyboard {
            return Some(false);
        }
    }
    let test = w + c.mouse_hit_test;
    // HitTest.ids: SmallVec<[HitboxId; 8]>: inline while capacity <= 8 (then
    // capacity is the length), else (ptr, len) on the heap.
    let ids = test + c.hit_test_ids;
    // SAFETY: as above, the hit test's ids and its hover count.
    let (ptr, len, count) = unsafe {
        let cap = read_checked::<usize>(ids + c.ids_capacity, "the hit test")?;
        let (ptr, len) = if cap <= c.ids_inline {
            (ids, cap)
        } else {
            let [ptr, len] = read_checked::<[usize; 2]>(ids, "the hit test")?;
            (ptr, len)
        };
        let count = read_checked::<usize>(test + c.hit_test_hover_count, "the hit test")?;
        (ptr, len, count)
    };
    let n = len.min(count);
    if n == 0 {
        return Some(false);
    }
    check(ptr, n.checked_mul(8)?, false, "the hit test's ids")?;
    // SAFETY: checked readable just above: `n` ids of 8 bytes.
    Some((0..n).any(|i| unsafe { mem::read::<u64>(ptr + i * 8) } == id))
}

/// A mouse event for [`dispatch_mouse`], logical pixels; left button.
#[derive(Clone, Copy, Debug)]
pub enum Mouse {
    Move(f32, f32),
    Down(f32, f32),
    Up(f32, f32),
    /// The right button's press and release (a context menu).
    RightDown(f32, f32),
    RightUp(f32, f32),
}

/// Feed `event` to the client's own `Window::dispatch_event` (the path the
/// platform's input takes). Returns whether it propagated, or `None` if the
/// layout has no whole events.
///
/// # Safety
/// `window` and `cx` must be the client's live `Window` and `App`, on its
/// UI thread, outside any draw (e.g. from `on_next_frame`).
pub unsafe fn dispatch_mouse(window: *mut u8, cx: *mut u8, event: Mouse) -> Option<bool> {
    let l = layout()?;
    let (c, e) = (&l.c, l.c.events?);
    let mut buf = Bytes::zeroed(e.size);
    let (tag_move, tag_down, tag_up) = e.tags;
    let (pay_move, pay_down, pay_up) = e.payload;
    let point = |buf: &mut Bytes, at: usize, x: f32, y: f32| {
        put(buf, at, &x.to_ne_bytes());
        put(buf, at + 4, &y.to_ne_bytes());
    };
    match event {
        Mouse::Move(x, y) => {
            buf[0] = tag_move;
            point(&mut buf, pay_move, x, y);
            // `pressed_button: None`.
            buf[pay_move + e.move_pressed_button] = 0xff;
        }
        Mouse::Down(x, y) | Mouse::Up(x, y) | Mouse::RightDown(x, y) | Mouse::RightUp(x, y) => {
            let (tag, pay) = if matches!(event, Mouse::Down(..) | Mouse::RightDown(..)) {
                (tag_down, pay_down)
            } else {
                (tag_up, pay_up)
            };
            let button = if matches!(event, Mouse::RightDown(..) | Mouse::RightUp(..)) {
                e.button_right
            } else {
                c.button_left
            };
            buf[0] = tag;
            point(&mut buf, pay, x, y);
            put_word(&mut buf, pay + e.click_count, 1);
            buf[pay + c.mouse_button] = button;
        }
    }
    // SAFETY: the caller's contract; `buf` is a whole mouse event, which owns
    // nothing.
    Some(unsafe { dispatch(&l, window, &mut buf, cx) })
}

/// Feed a key press to the client's `Window::dispatch_event`: `key` is
/// gpui's key name, `typed` the text it types. Returns whether it
/// propagated, or `None` if the layout has no whole events.
///
/// # Safety
/// As [`dispatch_mouse`].
pub unsafe fn dispatch_key(
    window: *mut u8,
    cx: *mut u8,
    key: &str,
    typed: &str,
    control: bool,
    shift: bool,
) -> Option<bool> {
    let l = layout()?;
    let (k, e) = (&l.c.key, l.c.events?);
    let mut buf = Bytes::zeroed(e.size);
    let (tag, pay) = e.key_down;
    buf[0] = tag;
    // An owned `String`, which the client drops with the event.
    let string = |buf: &mut Bytes, at: usize, text: &str| {
        let mut owned = ManuallyDrop::new(text.as_bytes().to_vec());
        let [cap, ptr, len] = k.string;
        put_word(buf, at + cap, owned.capacity());
        put_word(buf, at + ptr, owned.as_mut_ptr() as usize);
        put_word(buf, at + len, owned.len());
    };
    string(&mut buf, pay + k.key, key);
    // Always `Some`: bindings match on the key and modifiers, so a Ctrl
    // combination carries its text too.
    string(&mut buf, pay + k.key_char, typed);
    buf[pay + k.modifiers + k.control] = u8::from(control);
    buf[pay + k.modifiers + k.shift] = u8::from(shift);
    // SAFETY: the caller's contract; `buf` is a whole key event, and the
    // client takes and drops the strings it owns.
    Some(unsafe { dispatch(&l, window, &mut buf, cx) })
}

/// The client's `Window::dispatch_event(&mut self, event: PlatformInput, cx)
/// -> DispatchEventResult` on `event`, which it takes (a large value, so by
/// pointer): whether the event propagated (the result's first byte, in ax).
///
/// # Safety
/// As [`dispatch_mouse`], with `event` a whole `PlatformInput`.
unsafe fn dispatch(l: &Resolved, window: *mut u8, event: &mut Bytes, cx: *mut u8) -> bool {
    // SAFETY: the caller's contract; the client's `dispatch_event`.
    let result = unsafe {
        let f: unsafe fn(*mut u8, *mut u8, *mut u8) -> u16 =
            std::mem::transmute(l.fns.dispatch_event);
        f(window, event.as_mut_ptr(), cx)
    };
    result & 0xff != 0
}

/// The address of the client's `Window::dispatch_event`, which every
/// platform input goes through.
pub fn dispatch_event_addr() -> Option<usize> {
    layout().map(|l| l.fns.dispatch_event)
}

/// A mouse event read from a gpui `PlatformInput`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    Move(f32, f32),
    /// A button went down or up at (x, y); `left` for the left button.
    Down {
        x: f32,
        y: f32,
        left: bool,
    },
    Up {
        x: f32,
        y: f32,
        left: bool,
    },
    /// Anything else (keys, scrolls, files).
    Other,
}

/// The `PlatformInput` at `event`, as bytes.
///
/// # Safety
/// `event` points at a live `PlatformInput` of this build's gpui.
unsafe fn event_bytes<'a>(l: &Resolved, event: *const u8) -> Option<&'a [u8]> {
    let e = l.c.events?;
    check(event.addr(), e.size, false, "an input event")?;
    // SAFETY: checked readable just above; the caller's contract puts a
    // `PlatformInput` of that size there.
    Some(unsafe { std::slice::from_raw_parts(event, e.size) })
}

/// Read the `PlatformInput` at `event` (what `Window::dispatch_event` is
/// handed).
///
/// # Safety
/// `event` points at a live `PlatformInput` of this build's gpui.
pub unsafe fn read_input(event: *const u8) -> Option<Input> {
    let l = layout()?;
    let (c, e) = (&l.c, l.c.events?);
    // SAFETY: the caller's contract.
    let b = unsafe { event_bytes(&l, event)? };
    let f32_at = |at: usize| f32::from_ne_bytes(*b[at..].first_chunk().expect("inside the event"));
    let (tag_move, tag_down, tag_up) = e.tags;
    let (pay_move, pay_down, pay_up) = e.payload;
    let tag = b[0];
    let pay = match tag {
        t if t == tag_move => return Some(Input::Move(f32_at(pay_move), f32_at(pay_move + 4))),
        t if t == tag_down => pay_down,
        t if t == tag_up => pay_up,
        _ => return Some(Input::Other),
    };
    let (x, y) = (f32_at(pay), f32_at(pay + 4));
    let left = b[pay + c.mouse_button] == c.button_left;
    Some(if tag == tag_down {
        Input::Down { x, y, left }
    } else {
        Input::Up { x, y, left }
    })
}

/// A key press as a key-down listener sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Key {
    /// gpui's key name ("a", "backspace", "enter", ...).
    pub key: String,
    /// The text it types, if any.
    pub key_char: Option<String>,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub platform: bool,
}

/// The key pressed, if the `PlatformInput` at `event` (what
/// `Window::dispatch_event` is handed) is a key-down.
///
/// # Safety
/// `event` points at a live `PlatformInput` of this build's gpui.
pub unsafe fn read_key_down(event: *const u8) -> Option<Key> {
    let l = layout()?;
    // SAFETY: the caller's contract.
    let b = unsafe { event_bytes(&l, event)? };
    let (tag, pay) = l.c.events?.key_down;
    // SAFETY: a key-down's payload is a `KeyDownEvent`, its keystroke first.
    (b[0] == tag).then(|| unsafe { read_key(&l.c, event.addr() + pay) })
}

/// Read a `&KeyDownEvent` (its keystroke at 0).
///
/// # Safety
/// `event` is a live `KeyDownEvent` of this build's gpui.
pub(crate) unsafe fn read_key(c: &ClientLayout, event: usize) -> Key {
    let k = &c.key;
    let string = |at: usize| -> Option<String> {
        let [cap, ptr, len] = k.string;
        // SAFETY: the caller's live event, whose keystroke has `String`s
        // (or an `Option<String>`) at `at`.
        let (cap, ptr, len) = unsafe {
            (
                mem::read::<usize>(at + cap),
                mem::read::<usize>(at + ptr),
                mem::read::<usize>(at + len),
            )
        };
        // Option<String>'s None sets cap's top bit.
        if cap >> 63 != 0 || len > cap || (len > 0 && ptr == 0) {
            return None;
        }
        if len == 0 {
            return Some(String::new());
        }
        check(ptr, len, false, "a keystroke's text")?;
        // SAFETY: checked readable just above: the string's `len` bytes.
        let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
        Some(String::from_utf8_lossy(bytes).into_owned())
    };
    let m = event + k.modifiers;
    // SAFETY: as above: the keystroke's `Modifiers`, a byte each.
    let flag = |at: usize| unsafe { mem::read::<u8>(m + at) } != 0;
    Key {
        key: string(event + k.key).unwrap_or_default(),
        key_char: string(event + k.key_char),
        control: flag(k.control),
        alt: flag(k.alt),
        shift: flag(k.shift),
        platform: flag(k.platform),
    }
}
