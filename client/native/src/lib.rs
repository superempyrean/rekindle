//! rekindle's UI as **real GPUI elements in the client's own element tree**,
//! without linking gpui.
//!
//! Every element is made by the client's own compiled gpui
//! (`Interactivity::new` for `div()`, `<Div as IntoElement>::into_any_element`,
//! `<&str as IntoElement>::into_any_element`) at addresses resolved from
//! anchors ([`Fns`]), and styled and given its children by writing gpui's
//! values at recovered offsets with recovered encodings ([`Spec`]). The client's
//! gpui then lays them out, paints them and routes their input like any other
//! element. Nothing here names a gpui type; everything is bytes.
//!
//! Before each frame the framework layer calls [`before_draw_roots`], which
//! wraps the window's root so that rekindle's layer ([`set_layer`]) is drawn
//! over the client's tree. Render bodies the client calls itself (a modal,
//! a profile section, the settings page) are [`render_body`] closures.
//!
//! Limits: the wrapper is what GPUI stretches to the window, so a client root
//! that has no size of its own (relying on that stretch) would size to its
//! content instead. Roots normally ask for `size_full()`, as the mock's and
//! Zed's do. Text children are `&'static str`: each distinct string is leaked
//! once, so label text should be bounded.

mod build;
mod bytes;
pub mod client;
mod layout;
pub mod mem;
mod paint;
mod root;
mod scroll;
mod spec;
mod style;
pub mod validate;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;

pub use client::{Ctx, Focus, Key};
use layout::Resolved;
pub use paint::{
    Image, Paint, PaintImage, PaintRect, cached_image, can_draw_images, can_paint, image, painted,
    painted_images,
};
pub use root::{before_draw_roots, frames, marker, set_layer};
pub use scroll::{can_scroll, scroll_y_position, set_scroll_y_position};
pub use spec::{Fns, Spec, spec_from_maps};
pub use style::{Align, Cursor, Dir, Div, Edges, Handler, KeyHandler, Len, Node, Rgba, Style};

/// An `AnyElement`, opaque: three words (arena pointer, vtable, validity
/// handle) the client's gpui made and takes back.
type AnyElement = [usize; 3];

/// The layout [`configure`] resolved.
static SPEC: Mutex<Option<Arc<Resolved>>> = Mutex::new(None);
/// Set once building failed: the layer is off for the session.
static FAILED: AtomicBool = AtomicBool::new(false);
static LAST_ERROR: Mutex<String> = Mutex::new(String::new());

/// Validate the recovered layout and keep it. Nothing is hooked until the
/// framework layer calls [`before_draw_roots`].
pub fn configure(spec: Spec) -> Result<()> {
    let r = spec.resolve()?;
    *SPEC.lock().unwrap() = Some(Arc::new(r));
    Ok(())
}

/// The configured layout.
fn resolved() -> Option<Arc<Resolved>> {
    SPEC.lock().unwrap().clone()
}

/// Whether building failed (then the layer is off for the session) and why.
pub fn failure() -> Option<String> {
    FAILED
        .load(Ordering::SeqCst)
        .then(|| LAST_ERROR.lock().unwrap().clone())
}

/// Turn rekindle's layer off for the session, saying why.
fn fail(e: impl std::fmt::Display) {
    *LAST_ERROR.lock().unwrap() = e.to_string();
    FAILED.store(true, Ordering::SeqCst);
}

/// A render body the client stores and calls, `Rc<dyn Fn(&mut Window, &mut
/// App) -> AnyElement>`, as its (data, vtable) words: a closure of ours with
/// the same argument shapes (same compiler, so the same vtable and ABI) that
/// builds `f`'s node as a real element. The client owns the `Rc` and drops
/// it.
pub fn render_body(f: impl Fn(&mut Ctx) -> Node + 'static) -> [usize; 2] {
    type Body = std::rc::Rc<dyn Fn(*mut u8, *mut u8) -> AnyElement>;
    let body: Body = std::rc::Rc::new(move |window, cx| element(&f(&mut Ctx { window, cx })));
    // SAFETY: an `Rc<dyn Fn>` is one fat pointer to its allocation, data
    // then vtable, the representation the client's `Rc<dyn Fn>` has (same
    // compiler). The words carry the `Rc`'s count to the client.
    unsafe { std::mem::transmute::<Body, [usize; 2]>(body) }
}

/// Build `node` now, for a render body the client calls during its render.
/// On any failure the layer is switched off and an empty element stands in,
/// so the client always gets a valid `AnyElement`.
fn element(node: &Node) -> AnyElement {
    let Some(r) = resolved() else {
        // Render bodies are handed to the client only once configured.
        std::process::abort();
    };
    // SAFETY: the client calls a render body while it draws, on its UI
    // thread, and `r` is the configured layout.
    let build = |node: &Node| unsafe { build::build(&r, node) };
    let built = if FAILED.load(Ordering::SeqCst) {
        Err(anyhow::anyhow!("off"))
    } else {
        catch_unwind(AssertUnwindSafe(|| build(node)))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("panic")))
    };
    built.unwrap_or_else(|e| {
        if !FAILED.load(Ordering::SeqCst) {
            fail(format!("{e:#}"));
        }
        build(&Node::div(Style::default(), vec![])).unwrap_or_else(|_| std::process::abort())
    })
}
