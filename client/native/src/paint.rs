//! rekindle's own painting: quads and images inside a div, painted by the
//! client's `Window::paint_quad` and `Window::paint_image` in gpui's paint
//! phase. One element instead of a div per quad, so a sprite costs gpui one
//! layout node, and a picture is drawn as one.
//!
//! A painted div is built like any other (its place and size from its
//! style) and named as a group, which gives it a hitbox. Its `AnyElement`
//! stays gpui's own: the element in gpui's arena, and the arena's validity
//! flag. rekindle swaps only its vtable pointer, for a copy of the div's
//! `ElementObject` vtable whose `prepaint` and `paint` call the originals
//! first. `prepaint` then reads the div's bounds off the hitbox it just
//! inserted, and `paint` paints the quads there. gpui drops the element
//! through its arena as it always does, so nothing of rekindle's is left to
//! free.
//!
//! An image is an `Arc<RenderImage>` of one frame that rekindle builds once
//! per key from a template gpui made ([`image`]), with an id of its own
//! range, and keeps for the process. `paint_image` takes the `Arc` by value,
//! so each call gets one more strong count, which gpui drops after it has
//! the frame in its sprite atlas.

use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::bytes::{Bytes, put, put_word};
use crate::layout::{ImageLayout, Resolved};
use crate::style::Rgba;
use crate::{AnyElement, fail, mem};

/// `ElementObject`'s vtable: `drop_in_place`, size, align, then its methods
/// in declaration order: `inner_element`, `request_layout`, `prepaint`,
/// `paint`, `layout_as_root` (gpui's `element.rs`).
const VTABLE_WORDS: usize = 8;
const PREPAINT: usize = 5;
const PAINT: usize = 6;

/// One quad, relative to the painted element's origin (logical pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub rgba: Rgba,
}

/// An image rekindle built for gpui: its `Arc<RenderImage>` allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Image(usize);

/// An image drawn at `(x, y)`, `w` by `h`, relative to the painted element's
/// origin (logical pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintImage {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub image: Image,
}

/// What a painted element paints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Paint {
    Quad(PaintRect),
    Image(PaintImage),
}

/// A painted element of this frame, by its data pointer.
struct Pending {
    items: Arc<[Paint]>,
    /// Its size from its style, to know its hitbox.
    size: (f32, f32),
    /// The vtable rekindle replaced.
    original: usize,
    /// Its origin, once `prepaint` found it.
    origin: Option<(f32, f32)>,
}

thread_local! {
    /// This frame's painted elements (the UI thread's).
    static PENDING: RefCell<HashMap<usize, Pending>> = RefCell::new(HashMap::new());
}

/// rekindle's vtable for each original it replaced (leaked: one per
/// element type, made once).
static VTABLES: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
/// Which word of an `AnyElement` is its vtable, plus one (0 = not found yet).
static SLOT: AtomicUsize = AtomicUsize::new(0);
/// Quads and images painted so far (for tests).
static PAINTED: AtomicUsize = AtomicUsize::new(0);
static PAINTED_IMAGES: AtomicUsize = AtomicUsize::new(0);

/// The images rekindle built, by key.
static IMAGES: Mutex<Vec<(String, Image)>> = Mutex::new(Vec::new());
/// At most this many images are kept (a plugin's sprites: one per frame of
/// an atlas). Past it, [`image`] makes no more.
const MAX_IMAGES: usize = 1024;
/// rekindle's image ids: far above the ones gpui counts up from 0, so the
/// sprite atlas never confuses the two.
const IMAGE_ID_BASE: usize = 1 << 48;

/// Whether this build's layout and functions let rekindle paint.
pub fn can_paint() -> bool {
    crate::resolved().is_some_and(|r| r.paint.is_some())
}

/// Whether this build lets rekindle draw images ([`image`]).
pub fn can_draw_images() -> bool {
    crate::resolved().is_some_and(|r| r.paint.is_some() && r.image.is_some())
}

/// Quads rekindle has painted.
pub fn painted() -> usize {
    PAINTED.load(Ordering::SeqCst)
}

/// Images rekindle has painted.
pub fn painted_images() -> usize {
    PAINTED_IMAGES.load(Ordering::SeqCst)
}

/// The image already made under `key`, if any.
pub fn cached_image(key: &str) -> Option<Image> {
    IMAGES
        .lock()
        .unwrap()
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, i)| *i)
}

/// The image under `key`, made from `rgba` (`width` by `height`, straight
/// alpha) the first time. `None` if this build can't draw images, the pixels
/// don't fit the size, or the cache is full.
pub fn image(key: &str, width: u32, height: u32, rgba: &[u8]) -> Option<Image> {
    let r = crate::resolved()?;
    let l = r.image.as_ref().filter(|_| r.paint.is_some())?;
    let mut all = IMAGES.lock().unwrap();
    if let Some((_, i)) = all.iter().find(|(k, _)| k == key) {
        return Some(*i);
    }
    let pixels = (width as usize) * (height as usize) * 4;
    if all.len() >= MAX_IMAGES || pixels == 0 || rgba.len() != pixels {
        return None;
    }
    // gpui keeps images in BGRA.
    let mut bgra = rgba.to_vec();
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let i = build_image(l, IMAGE_ID_BASE + all.len(), width, height, bgra);
    all.push((key.to_string(), i));
    Some(i)
}

/// An `Arc<RenderImage>` of one frame: the template's bytes, with this id,
/// size and pixels. Allocated with this DLL's allocator (the process heap);
/// rekindle's strong count keeps it for the process.
fn build_image(l: &ImageLayout, id: usize, width: u32, height: u32, bgra: Vec<u8>) -> Image {
    let mut value = l.template.clone();
    put_word(&mut value, l.id, id);
    put(&mut value, l.width, &width.to_ne_bytes());
    put(&mut value, l.height, &height.to_ne_bytes());
    // The frame's pixel `Vec` is the image's now.
    let mut pixels = std::mem::ManuallyDrop::new(bgra);
    let [cap, ptr, len] = l.pixels;
    put_word(&mut value, cap, pixels.capacity());
    put_word(&mut value, ptr, pixels.as_mut_ptr() as usize);
    put_word(&mut value, len, pixels.len());
    let arc = mem::alloc(l.arc_value + value.len(), 16, true);
    // SAFETY: `arc` is a fresh allocation of the two counts (`arc_value`
    // is 16, checked by `resolve`) and the value.
    unsafe {
        mem::write(arc.addr(), 1usize);
        mem::write(arc.addr() + 8, 1usize);
        std::ptr::copy_nonoverlapping(value.as_ptr(), arc.add(l.arc_value), value.len());
    }
    Image(arc.addr())
}

/// A new frame: the last one's elements are gone.
pub(crate) fn new_frame() {
    PENDING.with(|p| p.borrow_mut().clear());
}

/// Which word of `e` (a div's `AnyElement`) is its vtable: the one pointing
/// at a table whose drop function is near the client's code, with a size
/// and alignment a type has, and its methods near the code too.
fn vtable_slot(r: &Resolved, e: &AnyElement) -> Option<usize> {
    if let Some(i) = SLOT.load(Ordering::SeqCst).checked_sub(1) {
        return Some(i);
    }
    let near = |f: usize| f.abs_diff(r.near) < 512 << 20;
    // A fat pointer is (data, vtable): the vtable is never the first word.
    for (i, &w) in e.iter().enumerate().skip(1) {
        if !w.is_multiple_of(8) || !mem::readable(w, VTABLE_WORDS * 8) {
            continue;
        }
        // SAFETY: checked readable just above.
        let words = unsafe { mem::read::<[usize; VTABLE_WORDS]>(w) };
        let [drop, size, align, ..] = words;
        if near(drop)
            && (64..=1 << 16).contains(&size)
            && align.is_power_of_two()
            && align <= 64
            && words[3..].iter().all(|&f| near(f))
        {
            SLOT.store(i + 1, Ordering::SeqCst);
            return Some(i);
        }
    }
    None
}

/// rekindle's copy of the vtable at `original` with its own `prepaint` and
/// `paint`, or `None` if `original` isn't readable.
fn vtable_for(original: usize) -> Option<usize> {
    let mut all = VTABLES.lock().unwrap();
    if let Some((_, ours)) = all.iter().find(|(o, _)| *o == original) {
        return Some(*ours);
    }
    if !mem::readable(original, VTABLE_WORDS * 8) {
        return None;
    }
    // SAFETY: checked readable just above.
    let mut words = unsafe { mem::read::<[usize; VTABLE_WORDS]>(original) };
    words[PREPAINT] = prepaint as *const () as usize;
    words[PAINT] = paint as *const () as usize;
    let ours = Box::leak(Box::new(words)).as_ptr().addr();
    all.push((original, ours));
    Some(ours)
}

/// Make the div element `e` paint `items` (relative to its origin) once
/// gpui has laid it out at `size`. If this build can't, the element stays a
/// plain div, and rekindle's layer turns off when that's a surprise.
///
/// # Safety
/// `e` must be the `AnyElement` gpui just made of a div, on the UI thread.
pub(crate) unsafe fn attach(
    r: &Resolved,
    e: &mut AnyElement,
    items: Arc<[Paint]>,
    size: (f32, f32),
) {
    if r.paint.is_none() {
        return;
    }
    let Some(slot) = vtable_slot(r, e) else {
        fail("no vtable in a div's AnyElement");
        return;
    };
    let (data, original) = (e[slot - 1], e[slot]);
    let Some(ours) = vtable_for(original) else {
        fail("a div's vtable isn't readable");
        return;
    };
    e[slot] = ours;
    PENDING.with(|p| {
        p.borrow_mut().insert(
            data,
            Pending {
                items,
                size,
                original,
                origin: None,
            },
        )
    });
}

/// `ElementObject::prepaint` and `paint`: `(&mut self, &mut Window, &mut App)`.
type Phase = unsafe fn(*mut u8, *mut u8, *mut u8);

/// The original method `index` of the painted element at `this`.
fn original(this: *mut u8, index: usize) -> Option<Phase> {
    let vtable = PENDING.with(|p| p.borrow().get(&this.addr()).map(|e| e.original))?;
    // SAFETY: `vtable_for` checked the original vtable readable for
    // `VTABLE_WORDS` words, and its method slots hold the methods of this
    // element's type.
    Some(unsafe { std::mem::transmute::<usize, Phase>(mem::read::<usize>(vtable + index * 8)) })
}

/// `ElementObject::prepaint`: the div's, then where its hitbox landed.
///
/// # Safety
/// Only gpui calls this, through rekindle's copy of the element's vtable,
/// with the element and the window and app it prepaints in.
unsafe fn prepaint(this: *mut u8, window: *mut u8, cx: *mut u8) {
    let Some(f) = original(this, PREPAINT) else {
        return;
    };
    // SAFETY: the div's own prepaint, with the arguments gpui passed.
    unsafe { f(this, window, cx) };
    let placed = catch_unwind(AssertUnwindSafe(|| {
        let Some(r) = crate::resolved() else {
            return;
        };
        // SAFETY: gpui is prepainting this element in `window`.
        let Some((x, y, w, h)) = (unsafe { crate::client::last_hitbox(&r.c, window) }) else {
            return;
        };
        PENDING.with(|p| {
            if let Some(e) = p.borrow_mut().get_mut(&this.addr()) {
                // Its own hitbox: the size its style gave it.
                if (w - e.size.0).abs() < 0.5 && (h - e.size.1).abs() < 0.5 {
                    e.origin = Some((x, y));
                }
            }
        });
    }));
    if placed.is_err() {
        fail("panic while placing a painted element");
    }
}

/// Logical-pixel bounds as gpui passes a `Bounds<Pixels>` or
/// `Corners<Pixels>`: four f32s. The same layout lowers to the same Rust
/// ABI, so a function pointer typed with it calls gpui's.
#[repr(C)]
#[derive(Clone, Copy)]
struct Four([f32; 4]);

/// `Window::paint_image(&mut self, bounds, image_bounds, corner_radii,
/// Arc<RenderImage>, frame_index, grayscale) -> Result<()>`.
type PaintImageFn = unsafe fn(*mut u8, Four, Four, Four, usize, usize, bool) -> usize;

/// `Window::paint_quad(&mut self, quad: PaintQuad)`: a struct that large is
/// passed by pointer.
type PaintQuadFn = unsafe fn(*mut u8, *mut u8);

/// `ElementObject::paint`: the div's, then rekindle's quads and images.
///
/// # Safety
/// As [`prepaint`].
unsafe fn paint(this: *mut u8, window: *mut u8, cx: *mut u8) {
    let Some(f) = original(this, PAINT) else {
        return;
    };
    // SAFETY: the div's own paint, with the arguments gpui passed.
    unsafe { f(this, window, cx) };
    let painted = catch_unwind(AssertUnwindSafe(|| {
        let Some(r) = crate::resolved() else {
            return;
        };
        let Some(quad) = &r.paint else {
            return;
        };
        let Some((origin, items)) = PENDING.with(|p| {
            p.borrow()
                .get(&this.addr())
                .and_then(|e| e.origin.map(|o| (o, e.items.clone())))
        }) else {
            return;
        };
        // SAFETY: `paint_quad` and `paint_image` are the client's functions
        // with these signatures (see `crate::Fns`).
        let (paint_quad, paint_image) = unsafe {
            (
                std::mem::transmute::<usize, PaintQuadFn>(quad.paint_quad),
                r.image
                    .as_ref()
                    .map(|l| std::mem::transmute::<usize, PaintImageFn>(l.paint_image)),
            )
        };
        let mut buf = Bytes::copied(&quad.template, quad.template.len());
        for item in items.iter() {
            match item {
                Paint::Quad(q) => {
                    buf[..quad.template.len()].copy_from_slice(&quad.template);
                    let bounds = [origin.0 + q.x, origin.1 + q.y, q.w, q.h];
                    for (k, v) in bounds.into_iter().enumerate() {
                        put(&mut buf, quad.bounds + 4 * k, &v.to_ne_bytes());
                    }
                    for (k, v) in q.rgba.to_hsla().into_iter().enumerate() {
                        put(&mut buf, quad.hsla + 4 * k, &v.to_ne_bytes());
                    }
                    // SAFETY: gpui is painting this element in `window`, and
                    // `buf` is a whole `PaintQuad` (the template), owning
                    // nothing, for the call to take.
                    unsafe { paint_quad(window, buf.as_mut_ptr()) };
                    PAINTED.fetch_add(1, Ordering::Relaxed);
                }
                Paint::Image(i) => {
                    let Some(paint_image) = paint_image else {
                        continue;
                    };
                    let b = Four([origin.0 + i.x, origin.1 + i.y, i.w, i.h]);
                    // SAFETY: an `Image` is rekindle's `Arc<RenderImage>`
                    // allocation, kept alive by rekindle's strong count,
                    // which is its first word and atomic, as in `Arc`.
                    let strong = unsafe { &*(i.image.0 as *const AtomicUsize) };
                    // The call's own strong count, which gpui drops.
                    strong.fetch_add(1, Ordering::Relaxed);
                    // SAFETY: gpui is painting this element in `window`, and
                    // the `Arc` passed carries the count added just above.
                    // The `Result<(), anyhow::Error>` it returns is one
                    // pointer, null for `Ok`. An `Err` (an image gpui can't
                    // upload) is leaked, as rekindle has no drop for it, and
                    // the image isn't drawn this frame.
                    let _ =
                        unsafe { paint_image(window, b, b, Four([0.0; 4]), i.image.0, 0, false) };
                    PAINTED_IMAGES.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }));
    if painted.is_err() {
        fail("panic while painting");
    }
}
