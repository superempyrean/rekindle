//! The framework layer: rekindle's per-frame work in the client's gpui,
//! from a detour of `Window::draw_roots` (or `Window::draw`) and one of
//! `Window::dispatch_event`.
//!
//! Before gpui draws, rekindle's layer (sprites, chat-bar buttons, toasts,
//! rekindle's window, the inspector) joins the client's element tree as
//! real elements (`rekindle-native`). After it draws, the frame record
//! (which includes what gpui copied from the previous frame for cached
//! views) is read to find surfaces by shape, and the scene is recoloured
//! for theming. The window and frame layout come from `rekindle-native`'s
//! spec.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::{Result, anyhow};
use rekindle_hook::{Detour, InlineDetour};
use rekindle_native::{Ctx, Edges, Len, Node, Style, client};

use crate::{
    Hook, chat_bar, clipboard, drive, insert, inspector, overlay, panel, theme, toast, window,
};

/// The gpui function the framework layer hooks.
#[derive(Clone, Copy, Debug)]
pub struct FrameworkLayout {
    pub frame_hook: FrameHook,
}

/// Where rekindle's per-frame work hooks the client's gpui.
#[derive(Clone, Copy, Debug)]
pub enum FrameHook {
    /// `Window::draw_roots(&mut self, &mut App)`: the frame is in
    /// `next_frame` when it returns.
    DrawRoots(usize),
    /// `Window::draw(&mut self, &mut App) -> ArenaClearNeeded`, for a build
    /// that inlined `draw_roots` into it: the frame has been swapped into
    /// `rendered_frame` when it returns (and not yet presented).
    Draw(usize),
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Set if a frame's work panicked or startup validation failed; the layer
/// then stays off.
static FAILED: AtomicBool = AtomicBool::new(false);
/// Frames whose finished scene held rekindle-native's layer (as painted by
/// the client's gpui), and where it was last.
static NATIVE_PAINTED: AtomicUsize = AtomicUsize::new(0);
static NATIVE_BOUNDS: Mutex<Option<(f32, f32, f32, f32)>> = Mutex::new(None);
static TRAMP_DRAW: AtomicUsize = AtomicUsize::new(0);
static TRAMP_DISPATCH: AtomicUsize = AtomicUsize::new(0);
/// The inspector kept a press from the app, so it keeps the release too.
static HELD: AtomicBool = AtomicBool::new(false);

/// Install the framework layer. While it's installed, the overlay layer in
/// the client's registry stays registered but draws nothing.
///
/// Fails if the frame hook can't be detoured, or if the inspector is on and
/// `dispatch_event` can't be. Without the inspector, a missing
/// `dispatch_event` only costs the colour picker's drag.
///
/// # Safety
///
/// `layout.frame_hook` holds the address of the client's gpui function it
/// names, and `rekindle_native::configure` has run with this client's spec.
pub unsafe fn install_framework_layer(layout: FrameworkLayout) -> Result<Hook> {
    let (target, hook) = match layout.frame_hook {
        FrameHook::DrawRoots(a) => (a, hooked_draw_roots as *const () as usize),
        FrameHook::Draw(a) => (a, hooked_draw as *const () as usize),
    };
    client::read_rendered_frame(matches!(layout.frame_hook, FrameHook::Draw(_)));
    // SAFETY: the caller's contract: `target` is the gpui function whose
    // signature `hook` shares.
    let d = unsafe { InlineDetour.install(target, hook)? };
    TRAMP_DRAW.store(d.trampoline(), Ordering::SeqCst);
    rekindle_native::set_layer(native_layer);
    inspector::enable_from_env();
    let mut detours = vec![d];
    let dispatch = client::dispatch_event_addr()
        .ok_or_else(|| anyhow!("no dispatch_event"))
        .and_then(|addr| {
            // SAFETY: the spec's `Window::dispatch_event`, whose signature
            // `hooked_dispatch` shares.
            unsafe { InlineDetour.install(addr, hooked_dispatch as *const () as usize) }
        });
    match dispatch {
        Ok(d) => {
            TRAMP_DISPATCH.store(d.trampoline(), Ordering::SeqCst);
            detours.push(d);
        }
        Err(e) if inspector::enabled() => {
            return Err(e.context("hooking dispatch_event for the inspector"));
        }
        Err(_) => {}
    }
    ACTIVE.store(true, Ordering::SeqCst);
    Ok(Hook::new(detours))
}

/// Whether the framework layer is installed.
pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

/// Frames the framework layer has injected into (for self-tests).
pub fn framework_frames() -> usize {
    FRAMES.load(Ordering::SeqCst)
}

/// Whether the framework layer gave up after a failed frame.
pub fn framework_failed() -> bool {
    FAILED.load(Ordering::SeqCst)
}

/// rekindle-native's layer as the client painted it: frames seen, last bounds.
pub fn native_painted() -> (usize, Option<(f32, f32, f32, f32)>) {
    (
        NATIVE_PAINTED.load(Ordering::SeqCst),
        *NATIVE_BOUNDS.lock().unwrap(),
    )
}

/// rekindle's layer in the client's window, rebuilt every frame: the overlay
/// plugins' sprites, the chat-bar buttons, the live toasts, rekindle's
/// window and the inspector. All real elements: the client's gpui lays them
/// out, paints them, hovers them and routes their clicks.
fn native_layer(ctx: &mut Ctx) -> Option<Node> {
    let mut layer = overlay::sprite_nodes(ctx);
    layer.extend(chat_bar::nodes());
    // SAFETY: rekindle-native builds the layer with the live window, on its
    // UI thread.
    let viewport = unsafe { client::viewport(ctx.window()) };
    // Above the composer's row where rekindle has found it, so a toast
    // never covers the client's buttons or rekindle's.
    let bottom = match (chat_bar::first_button(), viewport) {
        (Some(first), Some((_, h))) if first.y > 0.0 && first.y < h => h - first.y + 8.0,
        _ => 16.0,
    };
    layer.extend(toast::stack(bottom));
    layer.extend(window::nodes(ctx, viewport));
    // Last, so it's drawn over everything else.
    layer.extend(inspector::nodes(viewport));
    if layer.is_empty() {
        return None;
    }
    // The layer covers the window but has no listeners, so gpui gives it no
    // hitbox: it takes no input from the client.
    Some(Node::div(
        Style {
            absolute: true,
            inset: Edges::all(Some(Len::Px(0.0))),
            ..Default::default()
        },
        layer,
    ))
}

/// `Window::draw_roots(&mut self, cx: &mut App)`: two pointers, Rust ABI.
///
/// # Safety
///
/// Called only by the client, in place of `draw_roots`, with its arguments.
unsafe fn hooked_draw_roots(this: *mut u8, cx: *mut u8) {
    // SAFETY: the client's own window, on the thread that draws it.
    unsafe { before_draw(this) };
    // SAFETY: TRAMP_DRAW holds the original, stored before the detour went
    // live, and this is its signature.
    unsafe {
        let original: unsafe fn(*mut u8, *mut u8) =
            std::mem::transmute(TRAMP_DRAW.load(Ordering::SeqCst));
        original(this, cx);
    }
    // SAFETY: as above.
    unsafe { after_draw(this) };
}

/// `Window::draw(&mut self, cx: &mut App) -> ArenaClearNeeded`: two
/// pointers in, one pointer-sized value back (in rax), Rust ABI.
///
/// # Safety
///
/// Called only by the client, in place of `draw`, with its arguments.
unsafe fn hooked_draw(this: *mut u8, cx: *mut u8) -> usize {
    // SAFETY: the client's own window, on the thread that draws it.
    unsafe { before_draw(this) };
    // SAFETY: TRAMP_DRAW holds the original, stored before the detour went
    // live, and this is its signature.
    let arena = unsafe {
        let original: unsafe fn(*mut u8, *mut u8) -> usize =
            std::mem::transmute(TRAMP_DRAW.load(Ordering::SeqCst));
        original(this, cx)
    };
    // SAFETY: as above.
    unsafe { after_draw(this) };
    arena
}

/// Before the client draws its roots: deliver a pending copy, and wrap the
/// root's render so rekindle's real elements join the tree.
///
/// # Safety
///
/// `window` is the client's live `Window`, on the thread that draws it.
unsafe fn before_draw(window: *mut u8) {
    clipboard::deliver_queued();
    // SAFETY: the caller's contract.
    unsafe { rekindle_native::before_draw_roots(window) };
}

/// After the client drew a frame (before it's presented): validate, drive,
/// recolour, find the chat bar, keep frames coming.
///
/// # Safety
///
/// As [`before_draw`].
unsafe fn after_draw(window: *mut u8) {
    if FAILED.load(Ordering::SeqCst) {
        return;
    }
    // Never let a rekindle panic unwind through gpui's frame.
    let frame = std::panic::AssertUnwindSafe(|| {
        // SAFETY: the caller's contract, for each call below.
        unsafe {
            // Startup validation: the canary must be in this scene.
            rekindle_native::validate::after_frame(window);
            if rekindle_native::validate::failed() {
                // Nothing of rekindle's runs in a build it doesn't understand.
                FAILED.store(true, Ordering::SeqCst);
                return;
            }
            // Before recolouring, so colours are as rekindle asked.
            note_native_paint(window);
            if drive::active() && drive::observe(window, &frame_quads(window)) {
                // A failed schedule only delays the drive's step a frame.
                // The callback runs with the live window and `App`, on the UI
                // thread, outside a draw: `act`'s contract.
                let _ = client::on_next_frame(window, |w, cx| drive::act(w, cx));
            }
            // Where the toasts are, for test runs that press what they cover.
            if drive::started()
                && toast::any()
                && let Some((w, _)) = client::viewport(window)
            {
                toast::note_cards(&frame_quads(window), w);
            }
            // Before recolouring, so it reports the client's own colours.
            inspector::observe(window);
            theme::recolor_scene(window);
            chat_bar::locate(window);
            frame_upkeep(window);
        }
    });
    if std::panic::catch_unwind(frame).is_err() {
        FAILED.store(true, Ordering::SeqCst);
    }
}

/// Per frame: record the pointer and viewport (the overlay plugins' input)
/// and keep frames coming, since the plugins animate on their own clock
/// (`refresh` is a no-op mid-draw, so it's asked for from the next frame).
///
/// # Safety
///
/// As [`before_draw`], after gpui has run this frame's callbacks.
unsafe fn frame_upkeep(window: *mut u8) {
    // SAFETY: the caller's contract.
    unsafe {
        overlay::note_view(window);
        // Without the callback, frames stop coming once nothing changes;
        // the next frame the client draws schedules it again.
        let _ = client::on_next_frame(window, |w, cx| {
            insert::input_tick(w, cx);
            // Outside the client's draw, with nothing of the client's
            // borrowed: the reads rekindle's worker is waiting on.
            crate::client::service(crate::client::SERVICE_BUDGET);
            client::refresh(w);
        });
    }
    FRAMES.fetch_add(1, Ordering::SeqCst);
}

/// The frame's solid quads (logical bounds, HSLA), for drive mode.
///
/// # Safety
///
/// As [`before_draw`].
unsafe fn frame_quads(window: *const u8) -> Vec<drive::FrameQuad> {
    // SAFETY: the caller's contract.
    unsafe { client::frame(window) }
        .map(|f| {
            f.quads
                .into_iter()
                .filter_map(|(r, c)| Some((r, c?)))
                .collect()
        })
        .unwrap_or_default()
}

/// Did the client's gpui paint rekindle's layer this frame? Look for a quad
/// in the finished scene with the layer's first background, and keep its
/// bounds (logical pixels).
///
/// # Safety
///
/// As [`before_draw`].
unsafe fn note_native_paint(window: *const u8) {
    let Some(want) = rekindle_native::marker() else {
        return;
    };
    let close = |c: &[f32; 4]| {
        (c[0] - want[0]).abs() < 1e-3
            && (c[1] - want[1]).abs() < 1e-3
            && (c[2] - want[2]).abs() < 1e-3
    };
    // SAFETY: the caller's contract.
    let quads = unsafe { frame_quads(window) };
    if let Some((r, _)) = quads.iter().find(|(_, c)| close(c)) {
        *NATIVE_BOUNDS.lock().unwrap() = Some(*r);
        NATIVE_PAINTED.fetch_add(1, Ordering::SeqCst);
    }
}

/// `Window::dispatch_event(&mut self, event: PlatformInput, cx: &mut App)
/// -> DispatchEventResult`: the event by pointer, the result in ax. In
/// inspect mode a press selects what's under it instead of reaching the
/// app. Mouse events own nothing, so one that isn't passed on needs no
/// drop.
///
/// # Safety
///
/// Called only by the client, in place of `dispatch_event`, with its
/// arguments.
unsafe fn hooked_dispatch(window: *mut u8, event: *mut u8, cx: *mut u8) -> u16 {
    // rekindle's window: its shortcut, and Escape while it's open. Kept from
    // the client.
    // SAFETY: the client's own event, as gpui hands it to dispatch_event.
    if let Some(key) = unsafe { client::read_key_down(event) } {
        if window::shortcut(&key) {
            window::open(!window::is_open());
            return 0;
        }
        if key.key == "escape" && !key.control && !key.shift && !key.alt && window::is_open() {
            window::open(false);
            return 0;
        }
    }
    // SAFETY: as above.
    let input = unsafe { client::read_input(event) };
    if let Some(input) = &input {
        // SAFETY: the client's live window, on its UI thread.
        unsafe { panel::picker_input(window, input) };
    }
    match input {
        Some(client::Input::Down { x, y, .. }) if inspector::inspecting() => {
            inspector::clicked(x, y);
            inspector::swallowed();
            HELD.store(true, Ordering::SeqCst);
            return 0;
        }
        Some(client::Input::Up { .. }) if HELD.swap(false, Ordering::SeqCst) => {
            inspector::swallowed();
            return 0;
        }
        _ => {}
    }
    // SAFETY: TRAMP_DISPATCH holds the original, stored before the detour
    // went live; the client's own arguments are passed on.
    unsafe {
        let original: unsafe fn(*mut u8, *mut u8, *mut u8) -> u16 =
            std::mem::transmute(TRAMP_DISPATCH.load(Ordering::SeqCst));
        original(window, event, cx)
    }
}
