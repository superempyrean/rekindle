//! rekindle's overlay layer: the `overlay` plugins' sprites as real
//! elements, drawn in the framework layer or, without it, through the
//! client's overlay registry.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rekindle_native::{Edges, Len, Node, Rgba, Style, client};

/// One solid run of an overlay sprite, in logical pixels relative to the
/// sprite's origin. `rgba` is `0xRRGGBBAA`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub rgba: u32,
}

/// A sprite's pixels: `width` by `height`, RGBA, drawn `w` by `h` logical
/// pixels. `key` names these pixels (it changes when they do).
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayPicture {
    pub key: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub w: f32,
    pub h: f32,
}

/// A sprite rekindle draws on its overlay layer: a position, its pixel runs,
/// and its picture. Drawn as one image where the client lets rekindle draw
/// images, else as its runs. Shared (`Arc`) so an unchanged frame is not
/// rebuilt.
#[derive(Clone, Debug)]
pub struct OverlayItem {
    pub x: f32,
    pub y: f32,
    pub rects: std::sync::Arc<Vec<OverlayRect>>,
    pub picture: Option<std::sync::Arc<OverlayPicture>>,
}

/// Set the sprites rekindle's overlay layer draws (empty: the layer draws nothing).
pub fn set_overlay(items: Vec<OverlayItem>) {
    *OVERLAY_ITEMS.lock().unwrap() = items;
}

/// What the client's window last reported while drawing rekindle's layer: the
/// cursor and the viewport, in logical pixels. `None` until the layer has been
/// drawn once (the client's window is up).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OverlayView {
    pub pointer: (f32, f32),
    pub viewport: (f32, f32),
}

/// What the client's window last reported, once rekindle's layer has been
/// drawn.
pub fn overlay_view() -> Option<OverlayView> {
    *OVERLAY_VIEW.lock().unwrap()
}

pub(crate) static OVERLAY_ITEMS: Mutex<Vec<OverlayItem>> = Mutex::new(Vec::new());
static OVERLAY_VIEW: Mutex<Option<OverlayView>> = Mutex::new(None);

/// How often rekindle built the sprites' elements: the drive checks the
/// client draws them.
pub(crate) static SPRITES_BUILT: AtomicUsize = AtomicUsize::new(0);

/// A sprite's picture as an image of the size it's drawn at in device
/// pixels, scaled nearest-neighbour so pixel art stays sharp. Made once per
/// picture and size.
fn sprite_image(p: &OverlayPicture, scale: f32) -> Option<rekindle_native::Image> {
    if !rekindle_native::can_draw_images() {
        return None;
    }
    let (dw, dh) = (
        (p.w * scale).round().max(1.0) as u32,
        (p.h * scale).round().max(1.0) as u32,
    );
    let key = format!("{}@{dw}x{dh}", p.key);
    if let Some(i) = rekindle_native::cached_image(&key) {
        return Some(i);
    }
    if dw > 2048 || dh > 2048 || p.width == 0 || p.height == 0 {
        return None;
    }
    let mut px = Vec::with_capacity((dw * dh * 4) as usize);
    for y in 0..dh {
        let sy = (y * p.height / dh) as usize;
        for x in 0..dw {
            let sx = (x * p.width / dw) as usize;
            let i = (sy * p.width as usize + sx) * 4;
            px.extend_from_slice(&p.rgba[i..i + 4]);
        }
    }
    rekindle_native::image(&key, dw, dh, &px)
}

/// The overlay plugins' live sprites as real elements: each solid run an
/// absolutely placed div, edges snapped to whole device pixels. (Sprites move
/// by fractional amounts and gpui antialiases each quad's edges on its own,
/// so unsnapped neighbours each half-cover the shared pixel and the
/// background bleeds through as seams.) Plain divs carry no hitbox, so the
/// sprites never take the client's mouse.
pub(crate) fn sprite_nodes(ctx: &mut rekindle_native::Ctx) -> Vec<Node> {
    let items = OVERLAY_ITEMS.lock().unwrap().clone();
    if items.is_empty() {
        return Vec::new();
    }
    // SAFETY: rekindle-native builds elements with the live window, on its
    // UI thread.
    let scale = unsafe { client::scale_factor(ctx.window()) }.unwrap_or(1.0);
    let snap = |v: f32| (v * scale).round() / scale;
    SPRITES_BUILT.fetch_add(1, Ordering::SeqCst);
    // One element per sprite, which rekindle paints, where it can: its
    // picture as one image, else its runs as quads.
    if rekindle_native::can_paint() {
        return items
            .iter()
            .filter_map(|item| painted_sprite(item, scale))
            .collect();
    }
    let mut out = Vec::new();
    for item in &items {
        let (ox, oy) = (snap(item.x), snap(item.y));
        for r in item.rects.iter() {
            let (x0, y0) = (ox + snap(r.x), oy + snap(r.y));
            let (x1, y1) = (ox + snap(r.x + r.w), oy + snap(r.y + r.h));
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let mut style = placed(x0, y0, x1 - x0, y1 - y0);
            style.background = Some(Rgba::hex(r.rgba));
            out.push(Node::div(style, vec![]));
        }
    }
    out
}

/// A sprite as one element rekindle paints: its picture as an image, else
/// its runs as quads, snapped to whole device pixels at `scale`.
fn painted_sprite(item: &OverlayItem, scale: f32) -> Option<Node> {
    let snap = |v: f32| (v * scale).round() / scale;
    let (ox, oy) = (snap(item.x), snap(item.y));
    if let Some(p) = item.picture.as_deref()
        && let Some(image) = sprite_image(p, scale)
    {
        let (w, h) = (snap(p.w), snap(p.h));
        let paint = rekindle_native::PaintImage {
            x: 0.0,
            y: 0.0,
            w,
            h,
            image,
        };
        return Some(Node::painted(
            placed(ox, oy, w, h),
            vec![rekindle_native::Paint::Image(paint)],
        ));
    }
    let rects: Vec<rekindle_native::PaintRect> = item
        .rects
        .iter()
        .filter_map(|r| {
            let (x0, y0, x1, y1) = (snap(r.x), snap(r.y), snap(r.x + r.w), snap(r.y + r.h));
            (x1 > x0 && y1 > y0).then(|| rekindle_native::PaintRect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
                rgba: Rgba::hex(r.rgba),
            })
        })
        .collect();
    if rects.is_empty() || rects.iter().any(|r| r.x < 0.0 || r.y < 0.0) {
        return None;
    }
    let (w, h) = rects.iter().fold((0f32, 0f32), |(w, h), r| {
        (w.max(r.x + r.w), h.max(r.y + r.h))
    });
    Some(Node::painted(
        placed(ox, oy, w, h),
        rects
            .into_iter()
            .map(rekindle_native::Paint::Quad)
            .collect(),
    ))
}

/// An absolutely placed box at `(x, y)`, `w` by `h`.
fn placed(x: f32, y: f32, w: f32, h: f32) -> Style {
    Style {
        absolute: true,
        inset: Edges {
            top: Some(Len::Px(y)),
            right: None,
            bottom: None,
            left: Some(Len::Px(x)),
        },
        width: Some(Len::Px(w)),
        height: Some(Len::Px(h)),
        ..Default::default()
    }
}
/// An overlay render, `Rc<dyn Fn(&mut Window, &mut App) -> Option<AnyElement>>`,
/// as (data, vtable) words. Its body always returns an element, and
/// `Some(element)` is the element's own bytes (`AnyElement`'s `Rc` is the
/// niche), so it's a `rekindle_native::render_body` too.
type OverlayRender = [usize; 2];

/// rekindle's overlay layer through the client's own overlay registry
/// (used when the framework layer isn't installed): the live sprites as
/// real elements, and plugin clipboard writes on the UI thread.
pub(crate) fn overlay_layer() -> OverlayRender {
    rekindle_native::render_body(|ctx| {
        if crate::framework::active() {
            // The framework layer draws (and copies) instead.
            return Node::div(Style::default(), vec![]);
        }
        crate::clipboard::deliver_queued();
        // SAFETY: the client renders its overlays with its live window, on
        // its UI thread, mid-draw: next-frame callbacks aren't running.
        unsafe {
            note_view(ctx.window());
            // The plugins animate on their own clock: keep frames coming.
            // Without the callback, frames stop once nothing changes; the
            // next frame the client draws schedules it again.
            let _ = client::on_next_frame(ctx.window(), |w, cx| {
                crate::insert::input_tick(w, cx);
                client::refresh(w);
            });
        }
        Node::div(Style::default(), sprite_nodes(ctx))
    })
}

/// Record the pointer and viewport, the overlay plugins' input.
///
/// # Safety
///
/// `window` is the client's live `Window`, on its UI thread.
pub(crate) unsafe fn note_view(window: *const u8) {
    // SAFETY: the caller's contract.
    let (pointer, viewport) = unsafe { (client::mouse_position(window), client::viewport(window)) };
    if let (Some(pointer), Some(viewport)) = (pointer, viewport) {
        *OVERLAY_VIEW.lock().unwrap() = Some(OverlayView { pointer, viewport });
    }
}
