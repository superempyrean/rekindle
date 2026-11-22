//! Into the client's tree. A window's root is an `AnyView` whose `render` is
//! a plain fn pointer (`Window.root`, recovered). Before each frame the
//! framework layer calls [`before_draw_roots`], which points it at
//! [`render_root`]. That calls the client's render, then returns
//! `div().child(client_root).child(layer)`: the client's tree untouched as
//! the first child, and rekindle's layer (absolutely positioned by its own
//! style) on top. The pointer is checked every frame, so a client that
//! replaces its root is followed.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::build::{build, into_any, new_div, push_child};
use crate::layout::Resolved;
use crate::style::{Edges, Len, Node, Rgba, Style};
use crate::{AnyElement, Ctx, FAILED, fail, mem, paint, validate};

/// Builds rekindle's layer each frame (from whatever state the caller keeps).
type LayerFn = Box<dyn Fn(&mut Ctx) -> Option<Node> + Send>;

static LAYER: Mutex<Option<LayerFn>> = Mutex::new(None);
/// The first background in the last layer built, for the scene check.
static MARKER: Mutex<Option<[f32; 4]>> = Mutex::new(None);
/// (window, the client's own root render fn) for each window wrapped.
static ORIGINALS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
static FRAMES: AtomicUsize = AtomicUsize::new(0);

/// What rekindle shows in the client's window, as real elements, rebuilt
/// every frame by `f`; `None` from it shows nothing (the client's root is then
/// returned unwrapped).
pub fn set_layer(f: impl Fn(&mut Ctx) -> Option<Node> + Send + 'static) {
    *LAYER.lock().unwrap() = Some(Box::new(f));
}

/// Frames in which rekindle's layer was built into the client's tree.
pub fn frames() -> usize {
    FRAMES.load(Ordering::SeqCst)
}

/// The first background in the last layer built (depth first), if any: what
/// to look for in the finished scene to confirm the client's gpui painted it.
pub fn marker() -> Option<[f32; 4]> {
    *MARKER.lock().unwrap()
}

/// Call with the `&mut Window` (as a pointer) before the client's
/// `Window::draw_roots` runs: point the window's root render at
/// [`render_root`] if it isn't already.
///
/// # Safety
/// `window` must be a live gpui `Window` of the client this layout was
/// recovered from, on the thread that draws it.
pub unsafe fn before_draw_roots(window: *mut u8) {
    let Some(r) = crate::resolved() else {
        return;
    };
    paint::new_frame();
    // First frame: check the layout against the OS before building anything.
    // SAFETY: the caller's contract.
    unsafe { validate::check_os(window) };
    if validate::failed() {
        fail(format!("validation: {}", validate::status()));
        return;
    }
    let root = window.addr() + r.window_root;
    let slot = root + r.view_render;
    if !mem::readable(root, 4) || !mem::writable(slot, 8) {
        fail(format!(
            "the window's root view at {root:#x} isn't accessible client memory"
        ));
        return;
    }
    // Option<AnyView>: draw_roots tests the first dword for None.
    // SAFETY: checked readable above, in the caller's live window.
    if unsafe { mem::read::<u32>(root) } == 0 {
        return;
    }
    // SAFETY: checked writable (so readable) above.
    let current = unsafe { mem::read::<usize>(slot) };
    let ours = render_root as *const () as usize;
    if current == ours {
        return;
    }
    // Only take over something that looks like a function in the client's
    // code; otherwise the layout is wrong for this build and rekindle stays
    // out.
    if current == 0 || current.abs_diff(r.near) > 256 << 20 {
        fail(format!(
            "window root render {current:#x} is not near the client's code ({:#x})",
            r.near
        ));
        return;
    }
    let mut originals = ORIGINALS.lock().unwrap();
    originals.retain(|&(w, _)| w != window.addr());
    originals.push((window.addr(), current));
    // SAFETY: checked writable above, and the caller's contract puts this on
    // the thread that reads it. `render_root` takes the arguments of the
    // render fn it replaces, and calls that one first.
    unsafe { mem::write(slot, ours) };
}

/// A root view's `render: fn(&AnyView, &mut Window, &mut App) -> AnyElement`:
/// sret in rcx, then view, window, cx (the registers gpui's
/// `any_view::render::<V>` takes them in). `C-unwind` so a panic in the
/// client's own render unwinds as it would have.
type RenderFn =
    unsafe extern "C-unwind" fn(*mut AnyElement, *const u8, *mut u8, *mut u8) -> *mut AnyElement;

/// Stands in for the root view's render: the client's render, then
/// rekindle's layer beside it.
///
/// # Safety
/// Only the client's gpui calls this, through the pointer
/// [`before_draw_roots`] put in a window's root view, with what it passes
/// that view's render.
unsafe extern "C-unwind" fn render_root(
    ret: *mut AnyElement,
    view: *const u8,
    window: *mut u8,
    cx: *mut u8,
) -> *mut AnyElement {
    let original = {
        let originals = ORIGINALS.lock().unwrap();
        originals
            .iter()
            .find(|(w, _)| *w == window.addr())
            .or(originals.last())
            .map(|(_, f)| *f)
    };
    let Some(original) = original else {
        // Can't happen: the slot points here only once its original is
        // recorded, and nothing drains the record.
        std::process::abort();
    };
    // SAFETY: `original` is the render fn the slot held, with the arguments
    // gpui passed this stand-in.
    unsafe {
        let original = std::mem::transmute::<usize, RenderFn>(original);
        original(ret, view, window, cx);
    }
    if FAILED.load(Ordering::SeqCst) {
        return ret;
    }
    let node = layer(window, cx);
    *MARKER.lock().unwrap() = node.as_ref().and_then(first_background);
    let (Some(node), Some(r)) = (node, crate::resolved()) else {
        return ret;
    };
    // SAFETY: the client's render just wrote its root element to `ret`.
    let root = unsafe { ret.read() };
    // SAFETY: gpui is drawing this window's frame, on its UI thread, and
    // `r` is the configured layout.
    match catch_unwind(AssertUnwindSafe(|| unsafe { wrap(&r, root, &node) })) {
        Ok(Ok(wrapped)) => {
            // SAFETY: `ret` is the render's return slot; `wrap` took
            // ownership of what it held.
            unsafe { ret.write(wrapped) };
            FRAMES.fetch_add(1, Ordering::SeqCst);
        }
        // The client's root is still in `*ret` (wrapping consumes it only
        // once everything else is built), so the frame is the client's own.
        Ok(Err(e)) => fail(format!("{e:#}")),
        Err(_) => fail("panic while building rekindle's layer"),
    }
    ret
}

/// This frame's layer: the canary alone until it has shown up in a finished
/// scene, nothing if a check failed, else what [`set_layer`]'s function
/// builds.
fn layer(window: *mut u8, cx: *mut u8) -> Option<Node> {
    if validate::wants_canary() {
        return Some(Node::div(
            Style {
                absolute: true,
                inset: Edges::all(Some(Len::Px(0.0))),
                ..Default::default()
            },
            vec![validate::canary()],
        ));
    }
    if !validate::passed() {
        return None;
    }
    let layer = LAYER.lock().unwrap();
    let f = layer.as_ref()?;
    catch_unwind(AssertUnwindSafe(|| f(&mut Ctx { window, cx }))).unwrap_or_else(|_| {
        fail("panic in the layer function");
        None
    })
}

/// `div().child(root).child(layer)`. The layer is built first, so a failure
/// leaves `root` with the caller.
///
/// # Safety
/// As `build`.
unsafe fn wrap(r: &Arc<Resolved>, root: AnyElement, layer: &Node) -> Result<AnyElement> {
    // SAFETY: the caller's contract.
    let layer = unsafe { build(r, layer)? };
    // SAFETY: the caller's contract.
    let mut div = unsafe { new_div(r)? };
    push_child(r, &mut div, root);
    push_child(r, &mut div, layer);
    // SAFETY: the caller's contract; `div` came from `new_div`.
    Ok(unsafe { into_any(r, div) })
}

fn first_background(node: &Node) -> Option<[f32; 4]> {
    match node {
        Node::Div(d) => d
            .style
            .background
            .map(Rgba::to_hsla)
            .or_else(|| d.children.iter().find_map(first_background)),
        Node::Text(_) => None,
    }
}
