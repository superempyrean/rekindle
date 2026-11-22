//! rekindle's overlay backend: what `overlay`-capable plugins drew, turned into
//! something the client's own renderer can paint.
//!
//! The runtime hands the backend size-checked atlas bytes (PNG or GIF) and
//! sprites (a sub-rect of an atlas blitted to a position). This keeps the
//! decoded atlases and live sprites, and converts each sprite into solid
//! pixel runs ([`OverlayItem`]) for `rekindle-ui-hook`'s overlay layer, which
//! draws them inside the client's frame. Nothing here knows what a plugin is
//! drawing.

use std::collections::HashMap;
use std::sync::Arc;

use rekindle_plugin_host::model::OverlaySprite;
use rekindle_ui_hook::{OverlayItem, OverlayPicture, OverlayRect};

use crate::key;

/// Viewport reported before the client's window has drawn a frame. Non-zero, so
/// plugins that lay out against it at activation get a usable area.
const FALLBACK_VIEWPORT: (f32, f32) = (1024.0, 700.0);

struct Atlas {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    /// Counts registrations, so a sprite's picture key changes with its pixels.
    generation: u64,
}

/// (image key, sx, sy, sw, sh, w bits, h bits): one cached frame.
type FrameKey = (String, u32, u32, u32, u32, u32, u32);
/// A cached frame: its pixel runs, and its picture.
type Frame = (Arc<Vec<OverlayRect>>, Option<Arc<OverlayPicture>>);

/// The decoded atlases and live sprites of every `overlay` plugin.
#[derive(Default)]
pub(crate) struct OverlayState {
    atlases: HashMap<String, Atlas>,
    sprites: Vec<OverlaySprite>,
    frames: HashMap<FrameKey, Frame>,
    generation: u64,
    viewport: Option<(f32, f32)>,
    dirty: bool,
}

impl OverlayState {
    pub(crate) fn register_image(&mut self, plugin: &str, id: &str, bytes: &[u8]) {
        let k = key(plugin, id);
        self.frames.retain(|f, _| f.0 != k);
        match image::load_from_memory(bytes) {
            Ok(img) => {
                let img = img.to_rgba8();
                let (width, height) = img.dimensions();
                self.generation += 1;
                self.atlases.insert(
                    k,
                    Atlas {
                        width,
                        height,
                        rgba: img.into_raw(),
                        generation: self.generation,
                    },
                );
            }
            Err(e) => {
                say!("rekindle: overlay image `{id}` from `{plugin}` did not decode: {e}");
                self.atlases.remove(&k);
            }
        }
        self.dirty = true;
    }

    pub(crate) fn remove_image(&mut self, plugin: &str, id: &str) {
        let k = key(plugin, id);
        self.atlases.remove(&k);
        self.frames.retain(|f, _| f.0 != k);
        self.sprites
            .retain(|s| !(s.plugin == plugin && s.image == id));
        self.dirty = true;
    }

    pub(crate) fn draw(&mut self, sprite: OverlaySprite) {
        self.sprites
            .retain(|s| !(s.plugin == sprite.plugin && s.instance == sprite.instance));
        self.sprites.push(sprite);
        self.dirty = true;
    }

    pub(crate) fn clear(&mut self, plugin: &str, instance: &str) {
        self.sprites
            .retain(|s| !(s.plugin == plugin && s.instance == instance));
        self.dirty = true;
    }

    pub(crate) fn plugin_unloaded(&mut self, plugin: &str) {
        let prefix = key(plugin, "");
        self.atlases.retain(|k, _| !k.starts_with(&prefix));
        self.frames.retain(|f, _| !f.0.starts_with(&prefix));
        self.sprites.retain(|s| s.plugin != plugin);
        self.dirty = true;
    }

    pub(crate) fn viewport(&self) -> (f32, f32) {
        self.viewport.unwrap_or(FALLBACK_VIEWPORT)
    }

    pub(crate) fn set_viewport(&mut self, vp: (f32, f32)) {
        if vp.0 > 0.0 && vp.1 > 0.0 {
            self.viewport = Some(vp);
        }
    }

    pub(crate) fn sprite_count(&self) -> usize {
        self.sprites.len()
    }

    /// The layer's items, if anything changed since the last call.
    pub(crate) fn take_items(&mut self) -> Option<Vec<OverlayItem>> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        let mut items = Vec::with_capacity(self.sprites.len());
        for s in &self.sprites {
            let k = key(&s.plugin, &s.image);
            let Some(atlas) = self.atlases.get(&k) else {
                continue;
            };
            let fk = (k, s.sx, s.sy, s.sw, s.sh, s.w.to_bits(), s.h.to_bits());
            let (rects, picture) = self
                .frames
                .entry(fk)
                .or_insert_with(|| (Arc::new(runs(atlas, s)), picture(atlas, s).map(Arc::new)))
                .clone();
            items.push(OverlayItem {
                x: s.x,
                y: s.y,
                rects,
                picture,
            });
        }
        Some(items)
    }
}

/// The sprite's source rect as its own picture (RGBA), drawn at its
/// destination size.
fn picture(atlas: &Atlas, s: &OverlaySprite) -> Option<OverlayPicture> {
    let x1 = s.sx.saturating_add(s.sw).min(atlas.width);
    let y1 = s.sy.saturating_add(s.sh).min(atlas.height);
    if s.sw == 0 || s.sh == 0 || s.sx >= x1 || s.sy >= y1 || s.w <= 0.0 || s.h <= 0.0 {
        return None;
    }
    let (width, height) = (x1 - s.sx, y1 - s.sy);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in s.sy..y1 {
        let row = ((y * atlas.width + s.sx) * 4) as usize;
        rgba.extend_from_slice(&atlas.rgba[row..row + (width * 4) as usize]);
    }
    let key = format!(
        "{}/{}:{},{},{},{}",
        s.plugin, atlas.generation, s.sx, s.sy, width, height
    );
    Some(OverlayPicture {
        key,
        width,
        height,
        rgba,
        w: s.w * width as f32 / s.sw as f32,
        h: s.h * height as f32 / s.sh as f32,
    })
}

/// The sprite's source rect as horizontal runs of one opaque-ish colour,
/// scaled to its destination size. Fully transparent pixels are skipped.
fn runs(atlas: &Atlas, s: &OverlaySprite) -> Vec<OverlayRect> {
    let x1 = s.sx.saturating_add(s.sw).min(atlas.width);
    let y1 = s.sy.saturating_add(s.sh).min(atlas.height);
    if s.sw == 0 || s.sh == 0 || s.sx >= x1 || s.sy >= y1 {
        return Vec::new();
    }
    let kx = s.w / s.sw as f32;
    let ky = s.h / s.sh as f32;
    let px = |x: u32, y: u32| -> u32 {
        let i = ((y * atlas.width + x) * 4) as usize;
        u32::from_be_bytes([
            atlas.rgba[i],
            atlas.rgba[i + 1],
            atlas.rgba[i + 2],
            atlas.rgba[i + 3],
        ])
    };
    let mut out = Vec::new();
    for y in s.sy..y1 {
        let mut x = s.sx;
        while x < x1 {
            let c = px(x, y);
            let mut end = x + 1;
            while end < x1 && px(end, y) == c {
                end += 1;
            }
            if c & 0xff != 0 {
                out.push(OverlayRect {
                    x: (x - s.sx) as f32 * kx,
                    y: (y - s.sy) as f32 * ky,
                    w: (end - x) as f32 * kx,
                    h: ky,
                    rgba: c,
                });
            }
            x = end;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_2x1() -> Vec<u8> {
        // red, transparent
        let img = image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 0, 0, 0]).unwrap();
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn sprite(x: f32) -> OverlaySprite {
        OverlaySprite {
            plugin: "p".into(),
            instance: "i".into(),
            image: "a".into(),
            sx: 0,
            sy: 0,
            sw: 2,
            sh: 1,
            x,
            y: 5.0,
            w: 4.0,
            h: 2.0,
        }
    }

    #[test]
    fn decodes_the_oneko_gif_sheet() {
        let gif = include_bytes!("../../../plugins/rust/oneko/src/oneko.gif");
        let mut st = OverlayState::default();
        st.register_image("p", "a", gif);
        // Idle frame: cell (3, 3) of the 32x32 grid.
        st.draw(OverlaySprite {
            sx: 96,
            sy: 96,
            sw: 32,
            sh: 32,
            w: 32.0,
            h: 32.0,
            ..sprite(0.0)
        });
        let items = st.take_items().unwrap();
        let rects = &items[0].rects;
        let area: f32 = rects.iter().map(|r| r.w * r.h).sum();
        assert!(
            !rects.is_empty() && area < 32.0 * 32.0,
            "opaque cat pixels, transparent background skipped ({area})"
        );
    }

    #[test]
    fn sprites_become_scaled_runs_and_only_publish_on_change() {
        let mut st = OverlayState::default();
        st.register_image("p", "a", &png_2x1());
        st.draw(sprite(10.0));
        let items = st.take_items().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].x, items[0].y), (10.0, 5.0));
        assert_eq!(
            *items[0].rects,
            vec![OverlayRect {
                x: 0.0,
                y: 0.0,
                w: 2.0,
                h: 2.0,
                rgba: 0xff0000ff
            }]
        );
        assert!(st.take_items().is_none(), "unchanged");
        st.draw(sprite(20.0));
        let again = st.take_items().unwrap();
        assert!(
            Arc::ptr_eq(&items[0].rects, &again[0].rects),
            "frame cached"
        );
        st.plugin_unloaded("p");
        assert!(st.take_items().unwrap().is_empty());
    }

    #[test]
    fn a_source_rect_past_u32_max_is_clipped_to_the_atlas() {
        let mut st = OverlayState::default();
        st.register_image("p", "a", &png_2x1());
        st.draw(OverlaySprite {
            sx: 1,
            sw: u32::MAX,
            ..sprite(0.0)
        });
        let items = st.take_items().unwrap();
        assert!(items[0].rects.is_empty(), "the second pixel is transparent");
    }
}
