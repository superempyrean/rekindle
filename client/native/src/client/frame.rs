//! The frame the client's gpui just drew, read from its window: hitboxes,
//! quads, glyphs and images ([`frame`]), and the scene's colours, which
//! theming rewrites ([`scene_colors`]).

use std::sync::atomic::{AtomicBool, Ordering};

use super::{Rect, check, layout};
use crate::layout::ClientLayout;
use crate::mem;

/// Whether the frame just drawn is read from `rendered_frame` (hooked after
/// `Window::draw`, which has already swapped the frames) rather than
/// `next_frame` (hooked after `Window::draw_roots`).
static RENDERED_FRAME: AtomicBool = AtomicBool::new(false);

/// Read the frame just drawn from `rendered_frame`: for the fallback hook on
/// `Window::draw`, where a build has inlined `draw_roots` away.
pub fn read_rendered_frame(yes: bool) {
    RENDERED_FRAME.store(yes, Ordering::SeqCst);
}

/// (hitboxes, scene) of the frame just drawn, as offsets in the window.
fn drawn(c: &ClientLayout) -> (usize, usize) {
    if RENDERED_FRAME.load(Ordering::SeqCst) {
        (c.rendered_hitboxes, c.rendered_scene)
    } else {
        (c.hitboxes, c.scene)
    }
}

/// A `Vec` of the client's, checked: its header looked like one and its
/// elements' bytes are readable (and writable, if made so).
pub(super) struct ClientVec {
    pub(super) ptr: usize,
    pub(super) len: usize,
    pub(super) cap: usize,
    size: usize,
    writable: bool,
}

impl ClientVec {
    /// The `Vec` whose header is at `at`, with `size`-byte elements, or
    /// `None` if it doesn't look like one (the layout is wrong for this
    /// build) or its memory isn't accessible.
    ///
    /// # Safety
    /// `at` must be a `Vec` header inside a live value of the client's, read
    /// on its UI thread, with elements of `size` bytes, and nothing may free
    /// or move its buffer while this is used.
    pub(super) unsafe fn at(c: &ClientLayout, at: usize, size: usize, write: bool) -> Option<Self> {
        let [cap_at, ptr_at, len_at] = c.vec;
        check(at, 24, false, "a Vec header")?;
        // SAFETY: checked readable just above, and the caller's contract
        // puts a `Vec` there.
        let (cap, ptr, len) = unsafe {
            (
                mem::read::<usize>(at + cap_at),
                mem::read::<usize>(at + ptr_at),
                mem::read::<usize>(at + len_at),
            )
        };
        if len > cap || cap >= 4_000_000 || (cap > 0 && ptr == 0) {
            return None;
        }
        if len > 0 {
            check(ptr, len.checked_mul(size)?, write, "a Vec's elements")?;
        }
        Some(ClientVec {
            ptr,
            len,
            cap,
            size,
            writable: write,
        })
    }

    /// The address of element `i`'s field at `off`, which must hold `n`
    /// bytes inside the element.
    fn field(&self, i: usize, off: usize, n: usize) -> usize {
        assert!(
            i < self.len && off + n <= self.size,
            "a field outside the Vec's elements"
        );
        self.ptr + i * self.size + off
    }

    /// The `T` at `off` in element `i`.
    fn get<T: Copy>(&self, i: usize, off: usize) -> T {
        let at = self.field(i, off, std::mem::size_of::<T>());
        // SAFETY: inside the elements `at` checked readable; every type read
        // here is plain data, valid for any bytes.
        unsafe { mem::read(at) }
    }

    /// Element `i`'s `Bounds` at `off`, divided by `k` (the scale factor,
    /// for device pixels; 1 for logical).
    fn rect(&self, i: usize, off: usize, k: f32) -> Rect {
        let [x, y, w, h] = self.get::<[f32; 4]>(i, off);
        (x / k, y / k, w / k, h / k)
    }

    /// Replace the `T` at `off` in element `i` by `f` of it.
    fn update<T: Copy>(&self, i: usize, off: usize, f: impl FnOnce(&mut T)) {
        assert!(self.writable, "a Vec checked for reading only");
        let at = self.field(i, off, std::mem::size_of::<T>());
        // SAFETY: inside the elements `at` checked writable, on the client's
        // UI thread (its contract).
        unsafe {
            let mut v = mem::read::<T>(at);
            f(&mut v);
            mem::write(at, v);
        }
    }
}

/// The last hitbox inserted into the frame being built (logical pixels):
/// in an element's `prepaint`, the one it just inserted.
///
/// # Safety
/// `window` must be the client's live `Window`, mid-frame on its UI thread.
pub(crate) unsafe fn last_hitbox(c: &ClientLayout, window: *const u8) -> Option<Rect> {
    let (size, bounds) = c.hitbox;
    // SAFETY: the caller's contract; `hitboxes` is the window's `Vec<Hitbox>`.
    let v = unsafe { ClientVec::at(c, window.addr() + c.hitboxes, size, false)? };
    (v.len > 0).then(|| v.rect(v.len - 1, bounds, 1.0))
}

/// What the client's gpui drew this frame, as recorded (reused cached-view
/// content included): every hitbox, every quad with its solid fill (if it
/// has one), and every text glyph sprite, in logical pixels.
pub struct FrameRecord {
    pub hitboxes: Vec<Rect>,
    pub quads: Vec<(Rect, Option<[f32; 4]>)>,
    /// gpui's draw order of each of `quads`.
    pub quad_orders: Vec<u32>,
    /// Each of `quads`' border colour (HSLA; alpha 0 when it has none).
    pub quad_borders: Vec<[f32; 4]>,
    /// Each glyph's bounds and draw order.
    pub glyphs: Vec<(Rect, u32)>,
    /// Each glyph's colour (HSLA), in the order of `glyphs`.
    pub glyph_colors: Vec<[f32; 4]>,
    /// Each image's bounds (`paint_image`), where the layout says where they are.
    pub images: Vec<Rect>,
    /// Each image's size in gpui's sprite atlas (device pixels, padding
    /// taken off): the size gpui read from the image. Empty where the
    /// layout doesn't say where the tile is.
    pub image_sizes: Vec<(i32, i32)>,
}

/// The frame just drawn, or `None` if its record doesn't look like one.
///
/// # Safety
/// `window` must be the client's live `Window`, on its UI thread, after its
/// `draw_roots` (or before the next).
pub unsafe fn frame(window: *const u8) -> Option<FrameRecord> {
    let l = layout()?;
    let c = &l.c;
    let base = window.addr();
    // SAFETY: the caller's contract; `scale_factor` is the window's f32.
    let k: f32 = unsafe { super::read_checked(base + c.scale_factor, "the scale factor")? };
    if !(0.25..=8.0).contains(&k) {
        return None;
    }
    let (hitboxes_at, scene_at) = drawn(c);
    let scene = base + scene_at;
    // SAFETY: the caller's contract: the window's hitbox `Vec` and its
    // scene's `Vec`s of primitives, which gpui leaves alone between frames.
    let vec = |at: usize, size: usize| unsafe { ClientVec::at(c, at, size, false) };
    let hb = vec(base + hitboxes_at, c.hitbox.0)?;
    let (qsize, qbounds, qbg, qborder) = c.quad;
    let q = vec(scene + c.quads, qsize)?;
    let quads = (0..q.len)
        .map(|i| {
            let solid = q.get::<u32>(i, qbg) == c.background_solid_tag;
            let fill = solid.then(|| q.get::<[f32; 4]>(i, qbg + c.fill_hsla));
            (q.rect(i, qbounds, k), fill)
        })
        .collect();
    let mut glyphs = Vec::new();
    let mut glyph_colors = Vec::new();
    for (list, (size, color), bounds, order) in [
        (c.mono, c.mono_sprite, c.sprite_bounds.0, c.sprite_order.0),
        (c.subpx, c.subpx_sprite, c.sprite_bounds.1, c.sprite_order.1),
    ] {
        let g = vec(scene + list, size)?;
        glyphs.extend((0..g.len).map(|i| (g.rect(i, bounds, k), g.get::<u32>(i, order))));
        glyph_colors.extend((0..g.len).map(|i| g.get::<[f32; 4]>(i, color)));
    }
    let mut images = Vec::new();
    let mut image_sizes = Vec::new();
    if let Some((list, size, bounds)) = c.poly {
        let p = vec(scene + list, size)?;
        images.extend((0..p.len).map(|i| p.rect(i, bounds, k)));
        if let Some((tile, tile_bounds, padding)) = c.poly_tile {
            image_sizes.extend((0..p.len).map(|i| {
                let pad = p.get::<u32>(i, tile + padding) as i32;
                let [_, _, w, h] = p.get::<[i32; 4]>(i, tile + tile_bounds);
                (w - 2 * pad, h - 2 * pad)
            }));
        }
    }
    Some(FrameRecord {
        // Hitboxes are in logical pixels already; the scene is in device
        // pixels.
        hitboxes: (0..hb.len).map(|i| hb.rect(i, c.hitbox.1, 1.0)).collect(),
        quads,
        quad_orders: (0..q.len).map(|i| q.get(i, c.quad_order)).collect(),
        quad_borders: (0..q.len).map(|i| q.get(i, qborder)).collect(),
        glyphs,
        glyph_colors,
        images,
        image_sizes,
    })
}

/// Which part of the scene a colour belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paint {
    Fill,
    Border,
    Text,
    Underline,
    Shadow,
}

/// Visit every colour of the scene the client's gpui just built (HSLA,
/// mutable): quad fills (solid ones) and borders, text sprites, underlines,
/// shadows. Returns `false` if the scene doesn't look like one.
///
/// # Safety
/// As [`frame`]; nothing else may touch the scene meanwhile.
pub unsafe fn scene_colors(window: *mut u8, mut f: impl FnMut(Paint, &mut [f32; 4])) -> bool {
    let Some(l) = layout() else {
        return false;
    };
    let c = &l.c;
    let scene = window.addr() + drawn(c).1;
    let mut visit =
        |list: usize, (size, color): (usize, usize), kind: Paint, solid_tag_at: Option<usize>| {
            // SAFETY: the caller's contract: one of the scene's `Vec`s of
            // primitives, which nothing else touches meanwhile.
            let Some(v) = (unsafe { ClientVec::at(c, scene + list, size, true) }) else {
                return false;
            };
            for i in 0..v.len {
                if let Some(tag_at) = solid_tag_at
                    && v.get::<u32>(i, tag_at) != c.background_solid_tag
                {
                    continue;
                }
                v.update(i, color, |colour: &mut [f32; 4]| f(kind, colour));
            }
            true
        };
    let (qsize, _, qbg, qborder) = c.quad;
    visit(c.quads, (qsize, qbg + c.fill_hsla), Paint::Fill, Some(qbg))
        && visit(c.quads, (qsize, qborder), Paint::Border, None)
        && visit(c.mono, c.mono_sprite, Paint::Text, None)
        && visit(c.subpx, c.subpx_sprite, Paint::Text, None)
        && visit(c.underlines, c.underline, Paint::Underline, None)
        && visit(c.shadows, c.shadow, Paint::Shadow, None)
}
