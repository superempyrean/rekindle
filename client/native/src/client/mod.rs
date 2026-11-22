//! The client's window, frame record and input, through recovered layout and
//! the client's own compiled gpui functions, with no gpui types: where
//! things are drawn (hitboxes, quads), the scene's colours (theming), the
//! pointer and viewport, next-frame callbacks, event dispatch (drive mode),
//! focus and text input.
//!
//! Every read of the client's memory at a recovered offset is checked
//! first ([`crate::mem`]). When a check fails, rekindle's client layer
//! turns itself off ([`crate::failure`] says where) rather than fault
//! inside the client, and every operation here is off from then on.

mod action;
mod focus;
mod frame;
mod input;
mod text;

use std::sync::Arc;

use anyhow::{Context, Result, bail};

pub use action::dispatch_action;
pub use focus::{Focus, focus_id, is_focused, new_focus};
pub(crate) use frame::last_hitbox;
pub use frame::{FrameRecord, Paint, frame, read_rendered_frame, scene_colors};
pub use input::{
    Input, Key, Mouse, dispatch_event_addr, dispatch_key, dispatch_mouse, pressed_bounds,
    read_input, read_key_down,
};
pub(crate) use input::{hitbox_hovered, note_pressed, read_key};
pub use text::{Inserted, focused_input, insert_text};

use crate::layout::Resolved;
use crate::mem;

/// (x, y, w, h) in logical pixels.
pub type Rect = (f32, f32, f32, f32);

/// What a handler runs with: the client's window and app, and gpui's own
/// operations on them. rekindle makes one only for the length of a call
/// from the client's gpui (a render body, a listener), so the window and
/// app are live and this is their UI thread.
pub struct Ctx {
    pub(crate) window: *mut u8,
    pub(crate) cx: *mut u8,
}

impl Ctx {
    /// The client's `&mut Window`.
    pub fn window(&self) -> *mut u8 {
        self.window
    }

    /// The client's `&mut App`.
    pub fn cx(&self) -> *mut u8 {
        self.cx
    }

    /// `Window::focus`.
    pub fn focus(&mut self, focus: &Focus) {
        if let Some(l) = layout() {
            // SAFETY: the client's `Window::focus`, with the live window and
            // app this `Ctx` holds and a handle rekindle holds.
            unsafe {
                let f: unsafe fn(*mut u8, *const u8, *mut u8) =
                    std::mem::transmute(l.fns.window_focus);
                f(self.window, focus.as_ptr(), self.cx);
            }
        }
    }

    /// `Window::blur`.
    pub fn blur(&mut self) {
        if let Some(l) = layout() {
            // SAFETY: the client's `Window::blur`, with the live window and
            // app this `Ctx` holds.
            unsafe {
                let f: unsafe fn(*mut u8, *mut u8) = std::mem::transmute(l.fns.window_blur);
                f(self.window, self.cx);
            }
        }
    }

    /// `App::stop_propagation`: the event this handler runs for goes no
    /// further.
    pub fn stop_propagation(&mut self) {
        let Some(l) = layout() else {
            return;
        };
        let at = self.cx.addr() + l.c.propagate_event;
        if check(at, 1, true, "the app's propagate_event").is_some() {
            // SAFETY: checked writable just above, in the live app.
            unsafe { mem::write(at, 0u8) };
        }
    }

    /// `Window::refresh`.
    pub fn refresh(&mut self) {
        // SAFETY: the live window this `Ctx` holds.
        unsafe { refresh(self.window) }
    }

    /// Whether `focus` is the window's focused handle.
    pub fn is_focused(&self, focus: &Focus) -> bool {
        // SAFETY: the live window this `Ctx` holds.
        unsafe { is_focused(self.window, focus) }
    }

    /// The focused element's focus id, if any.
    pub fn focused_id(&self) -> Option<u64> {
        // SAFETY: the live window this `Ctx` holds.
        unsafe { focus::focused_id(self.window) }
    }

    /// Focus the element with focus id `id` ([`focus_id`]).
    pub fn focus_id(&mut self, id: u64) -> Result<()> {
        // SAFETY: the live window and app this `Ctx` holds, from inside a
        // call of the client's gpui (outside any draw).
        unsafe { focus_id(self.window, self.cx, id) }
    }
}

/// The layout, unless rekindle's client layer has failed (a check below, or
/// the framework layer's own): then every client operation is off.
fn layout() -> Option<Arc<Resolved>> {
    if crate::failure().is_some() {
        return None;
    }
    crate::resolved()
}

/// The layout, or why there is none.
fn layout_or_err() -> Result<Arc<Resolved>> {
    layout().context("rekindle's client layer is off")
}

/// The error for a [`check`] that failed (it has turned the client layer off).
fn unsafe_memory() -> anyhow::Error {
    anyhow::anyhow!(
        "client memory isn't accessible: {}",
        crate::failure().unwrap_or_default()
    )
}

/// `Some(())` if `[p, p + n)` is client memory rekindle may read (or
/// write); otherwise rekindle's client layer turns itself off (`failure()`
/// says where) rather than fault inside the client.
fn check(p: usize, n: usize, write: bool, what: &str) -> Option<()> {
    let ok = if write {
        mem::writable(p, n)
    } else {
        mem::readable(p, n)
    };
    if !ok {
        crate::fail(format!(
            "client memory ({what}) at {p:#x}+{n} isn't {}",
            if write { "writable" } else { "readable" }
        ));
    }
    ok.then_some(())
}

/// The `T` at `addr`, once [`check`]ed readable.
///
/// # Safety
/// `addr` is where a live value of the client's keeps a `T`, read on the
/// client's UI thread.
unsafe fn read_checked<T: Copy>(addr: usize, what: &str) -> Option<T> {
    check(addr, std::mem::size_of::<T>(), false, what)?;
    // SAFETY: checked readable just above; the caller's contract says a `T`
    // is there.
    Some(unsafe { mem::read(addr) })
}

/// The window's viewport size, logical pixels.
///
/// # Safety
/// `window` must be the client's live `Window`, on its UI thread.
pub unsafe fn viewport(window: *const u8) -> Option<(f32, f32)> {
    let l = layout()?;
    // SAFETY: the caller's contract; `viewport_size` is the window's
    // `Size<Pixels>`.
    let [w, h] = unsafe {
        read_checked::<[f32; 2]>(window.addr() + l.c.viewport_size, "the viewport size")?
    };
    Some((w, h))
}

/// The window's scale factor: device pixels per logical pixel.
///
/// # Safety
/// As [`viewport`].
pub unsafe fn scale_factor(window: *const u8) -> Option<f32> {
    let l = layout()?;
    // SAFETY: the caller's contract; `scale_factor` is the window's f32.
    unsafe { read_checked(window.addr() + l.c.scale_factor, "the scale factor") }
}

/// The pointer's position in the window, logical pixels.
///
/// # Safety
/// As [`viewport`].
pub unsafe fn mouse_position(window: *const u8) -> Option<(f32, f32)> {
    let l = layout()?;
    // SAFETY: the caller's contract; `mouse_position` is the window's
    // `Point<Pixels>`.
    let [x, y] = unsafe {
        read_checked::<[f32; 2]>(window.addr() + l.c.mouse_position, "the mouse position")?
    };
    Some((x, y))
}

/// The type name of the window's root view (`AnyView.view_type_name`), e.g.
/// `chat_client::ui::ChatClient`: which view rekindle is wrapping, by name.
///
/// # Safety
/// As [`viewport`].
pub unsafe fn root_view_type(window: *const u8) -> Option<String> {
    let l = layout()?;
    let at = window.addr() + l.c.window_root + l.c.view_type_name;
    // SAFETY: the caller's contract; the root view's type name is a
    // `&'static str`, (ptr, len).
    let [ptr, len] = unsafe { read_checked::<[usize; 2]>(at, "the root view's type name")? };
    if ptr == 0 || len == 0 || len > 512 {
        return None;
    }
    check(ptr, len, false, "the root view's type name")?;
    // SAFETY: checked readable just above; a `&'static str` lives for the
    // process.
    let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// The client's `Window::refresh` (redraw next frame; a no-op mid-draw).
///
/// # Safety
/// As [`viewport`].
pub unsafe fn refresh(window: *mut u8) {
    if let Some(l) = layout() {
        // SAFETY: the client's `Window::refresh`, with the caller's live
        // window.
        unsafe {
            let f: unsafe fn(*mut u8) = std::mem::transmute(l.fns.window_refresh);
            f(window);
        }
    }
}

/// Schedule `f(window, cx)` right after the next frame, as gpui's
/// `Window::on_next_frame` does (that one is generic, so it can't be called
/// with a closure of ours): push onto `next_frame_callbacks`, ask the
/// platform window for a frame, wake the invalidator.
///
/// # Safety
/// As [`viewport`]; not while gpui is running next-frame callbacks.
pub unsafe fn on_next_frame(
    window: *mut u8,
    f: impl FnOnce(*mut u8, *mut u8) + 'static,
) -> Result<()> {
    let Some(l) = layout() else {
        bail!("not configured")
    };
    let c = &l.c;
    let base = window.addr();
    // `Rc<RefCell<Vec<FrameCallback>>>`: the borrow flag, then the Vec.
    // SAFETY: the caller's contract; `next_frame_callbacks` is the window's
    // `Rc`.
    let rc =
        unsafe { read_checked::<usize>(base + c.next_frame_callbacks, "the next-frame callbacks") }
            .ok_or_else(unsafe_memory)?;
    if rc == 0 {
        bail!("no next-frame callback list");
    }
    let borrow = rc + c.refcell_borrow;
    let callbacks = rc + c.refcell_vec;
    check(borrow, 8, true, "the callbacks' RefCell").ok_or_else(unsafe_memory)?;
    check(callbacks, 24, true, "the callbacks' Vec").ok_or_else(unsafe_memory)?;
    // SAFETY: the caller's contract: the `RefCell`'s `Vec` of boxed
    // callbacks, 16 bytes each, which only this thread touches.
    let list = unsafe { frame::ClientVec::at(c, callbacks, 16, false) }
        .context("the next-frame callback list doesn't look like a Vec")?;
    if list.len < list.cap {
        check(
            list.ptr + list.len * 16,
            16,
            true,
            "the callbacks' next slot",
        )
        .ok_or_else(unsafe_memory)?;
    }
    // `Box<dyn PlatformWindow>`: (data, vtable).
    // SAFETY: the caller's contract; `platform_window` is the window's box.
    let [data, vtable] =
        unsafe { read_checked::<[usize; 2]>(base + c.platform_window, "the platform window") }
            .ok_or_else(unsafe_memory)?;
    // SAFETY: a vtable slot of the live platform window.
    let schedule = unsafe {
        read_checked::<usize>(
            vtable + c.schedule_frame_slot,
            "the platform window's vtable",
        )
    }
    .ok_or_else(unsafe_memory)?;
    if !mem::executable(schedule) {
        bail!("the platform window's schedule_frame slot doesn't point at code");
    }
    // SAFETY: checked readable above; `isize` is the `RefCell`'s flag.
    if unsafe { mem::read::<isize>(borrow) } != 0 {
        bail!("next-frame callbacks are borrowed");
    }
    // `FrameCallback` is `Box<dyn FnOnce(&mut Window, &mut App)>`: a closure
    // of ours with the same argument shapes has the same vtable layout and
    // ABI (same compiler).
    let boxed: Box<dyn FnOnce(*mut u8, *mut u8)> = Box::new(f);
    let raw = Box::into_raw(boxed);
    // SAFETY: a `*mut dyn FnOnce` is two words, data then vtable, as the
    // client's `Box<dyn FnOnce>` is.
    let fat = unsafe { std::mem::transmute::<*mut dyn FnOnce(*mut u8, *mut u8), [usize; 2]>(raw) };
    let mut elem = [0u8; 16];
    elem[..8].copy_from_slice(&fat[0].to_ne_bytes());
    elem[8..].copy_from_slice(&fat[1].to_ne_bytes());
    // SAFETY: the `RefCell` isn't borrowed (checked above) and this is its
    // UI thread, so the `Vec` (checked above to look like one, with room or
    // a readable buffer to grow from) is rekindle's to push to, and the
    // callback is a valid element for it to own.
    if let Err(e) = unsafe { mem::push_vec(callbacks, c.vec, &elem) } {
        // SAFETY: `raw` came from `Box::into_raw` just above, and the push
        // that would have handed it over failed.
        drop(unsafe { Box::from_raw(raw) });
        return Err(e);
    }
    // SAFETY: the platform window's `schedule_frame(&self)` (checked to be
    // code above) on its live data, then the client's
    // `WindowInvalidator::wake_platform` on the window's invalidator.
    unsafe {
        let schedule: unsafe fn(usize) = std::mem::transmute(schedule);
        schedule(data);
        let wake: unsafe fn(usize) = std::mem::transmute(l.fns.wake_platform);
        wake(base + c.invalidator);
    }
    Ok(())
}
