//! Text into the client's focused field, the way the platform delivers a
//! typed character or an IME commit: through the window's input handler.

use anyhow::{Result, bail};

use super::{layout_or_err, read_checked, unsafe_memory};
use crate::bytes::{Bytes, word};
use crate::layout::InputLayout;
use crate::mem;

/// What [`insert_text`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inserted {
    /// The focused field took the text.
    Inserted,
    /// No text field is focused, so there is no input handler to take it.
    NoField,
}

/// The input handler's layout, and the platform window's data pointer and
/// its `take_input_handler` and `set_input_handler`, checked to be code.
///
/// # Safety
/// `window` must be the client's live `Window`, on its UI thread.
unsafe fn input_slots(window: *mut u8) -> Result<(InputLayout, usize, usize, usize)> {
    let l = layout_or_err()?;
    let c = &l.c;
    let Some(i) = c.input else {
        bail!("this client's layout has no input handler entries");
    };
    // SAFETY: the caller's contract; `platform_window` is the window's
    // `Box<dyn PlatformWindow>`, (data, vtable).
    let [data, vtable] = unsafe {
        read_checked::<[usize; 2]>(window.addr() + c.platform_window, "the platform window")
    }
    .ok_or_else(unsafe_memory)?;
    // SAFETY: two slots of the live platform window's vtable.
    let (take, set) = unsafe {
        (
            read_checked::<usize>(vtable + i.take_slot, "the platform window's vtable"),
            read_checked::<usize>(vtable + i.set_slot, "the platform window's vtable"),
        )
    };
    let (take, set) = take.zip(set).ok_or_else(unsafe_memory)?;
    if !mem::executable(take) || !mem::executable(set) {
        bail!("the platform window's input handler slots don't point at code");
    }
    Ok((i, data, take, set))
}

/// Run `f` on the window's `PlatformInputHandler` (the focused text field's,
/// registered while it painted), if there is one, and put it back. Moving it
/// out and back is what gpui does around every use of it. `f` gets the
/// handler's address, a `PlatformInputHandler` of `InputLayout::size` bytes.
///
/// # Safety
/// As [`input_slots`].
unsafe fn with_input_handler<R>(
    window: *mut u8,
    f: impl FnOnce(&InputLayout, usize) -> R,
) -> Result<Option<R>> {
    // SAFETY: the caller's contract.
    let (i, data, take, set) = unsafe { input_slots(window)? };
    let mut handler = Bytes::zeroed(i.size);
    // SAFETY: `take_input_handler(&mut self) -> Option<PlatformInputHandler>`
    // returns through a hidden pointer, here a buffer of the value's size.
    unsafe {
        let take: unsafe extern "C" fn(*mut u8, usize) -> *mut u8 = std::mem::transmute(take);
        take(handler.as_mut_ptr(), data);
    }
    // `None` is 0 in the first word.
    if word(&handler, 0) == 0 {
        return Ok(None);
    }
    let r = f(&i, handler.as_ptr().addr());
    // SAFETY: `set_input_handler(&mut self, handler)` takes the large value
    // by pointer and moves it, handing the handler taken above back.
    unsafe {
        let set: unsafe extern "C" fn(usize, *mut u8) = std::mem::transmute(set);
        set(data, handler.as_mut_ptr());
    }
    Ok(Some(r))
}

/// The `FocusId` of the client's text field that has the window's input
/// handler: the field the user last typed in. `None` when no field has it.
///
/// # Safety
/// `window` must be the client's live `Window`, on its UI thread.
pub unsafe fn focused_input(window: *mut u8) -> Option<u64> {
    // SAFETY: the caller's contract; `f` reads the handler's
    // `Option<FocusId>`, inside it (checked by `resolve`).
    let id = unsafe {
        with_input_handler(window, |i, h| mem::read::<u64>(h + i.focus_id))
            .ok()
            .flatten()?
    };
    // `Option<FocusId>`: a slotmap key, whose high half is never 0.
    (id >> 32 != 0).then_some(id)
}

/// Put `text` into the focused text field at its caret, the way the platform
/// delivers a typed character or an IME commit: the field's input handler
/// gets `replace_text_in_range(None, text)`. gpui registers an input handler
/// only for the focused field, so with none focused this is
/// [`Inserted::NoField`].
///
/// # Safety
/// `window` and `cx` must be the client's live `Window` and `App`, on its UI
/// thread, outside any draw (a next-frame callback).
pub unsafe fn insert_text(window: *mut u8, cx: *mut u8, text: &str) -> Result<Inserted> {
    let insert = |i: &InputLayout, h: usize| -> Result<()> {
        // The `Box<dyn InputHandler>`: (data, vtable).
        // SAFETY: inside the live handler (checked by `resolve`).
        let [data, vtable] = unsafe { mem::read::<[usize; 2]>(h + i.handler) };
        // SAFETY: a slot of the live handler's vtable.
        let replace =
            unsafe { read_checked::<usize>(vtable + i.replace_slot, "the input handler's vtable") }
                .ok_or_else(unsafe_memory)?;
        if !mem::executable(replace) {
            bail!("the input handler's replace_text_in_range slot doesn't point at code");
        }
        // `replace_text_in_range(&mut self, Option<Range<usize>>, &str, &mut
        // Window, &mut App)`: the Option (24 bytes) by pointer, the str as
        // (ptr, len). `None` replaces the selection, at the caret.
        let none = [0u64; 3];
        // SAFETY: the handler's own method (checked to be code above), with
        // the caller's live window and app.
        unsafe {
            let replace: unsafe fn(usize, *const [u64; 3], *const u8, usize, *mut u8, *mut u8) =
                std::mem::transmute(replace);
            replace(data, &raw const none, text.as_ptr(), text.len(), window, cx);
        }
        Ok(())
    };
    // SAFETY: the caller's contract.
    match unsafe { with_input_handler(window, insert)? } {
        Some(r) => r.map(|()| Inserted::Inserted),
        None => Ok(Inserted::NoField),
    }
}
