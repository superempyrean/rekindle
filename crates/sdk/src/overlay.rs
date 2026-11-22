//! A transparent layer over the client to draw sprites on (requires
//! `overlay`).
//!
//! The plugin ships its own PNG atlas and blits rectangles of it to
//! positions on the layer. With the `pointer-moved` event and a
//! [`timer`](crate::timer), a sprite can follow the cursor. The host allows
//! 4 images (256 KiB and 512x512 each) and 16 live sprites per plugin.
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! # fn f(x: f32, y: f32, cat_png: &[u8]) -> Result<(), overlay::OverlayError> {
//! overlay::register_image("cat", cat_png)?;
//! overlay::draw(&overlay::sprite("pet", "cat", (0, 0, 32, 32), (x, y, 32.0, 32.0)))?;
//! # Ok(())
//! # }
//! ```

use crate::raw::rekindle::plugin::overlay as raw;
pub use raw::{OverlayError, Sprite};

/// Registers an image atlas under `id` for [`draw`]. Reusing an id replaces
/// the image.
pub fn register_image(id: &str, png: &[u8]) -> Result<(), OverlayError> {
    raw::register_image(id, png)
}

/// Removes a registered image and every sprite that uses it.
pub fn remove_image(id: &str) {
    raw::remove_image(id);
}

/// Draws a sprite, or moves it if its `instance` is already drawn.
pub fn draw(s: &Sprite) -> Result<(), OverlayError> {
    raw::draw(s)
}

/// Removes a sprite by its instance id.
pub fn clear(instance: &str) {
    raw::clear(instance);
}

/// The client's size in logical pixels.
pub fn viewport() -> (f32, f32) {
    raw::viewport()
}

/// A [`Sprite`] that blits the rectangle `src` (`x, y, width, height`, in
/// image pixels) of `image` to `dst` (in logical pixels on the layer).
pub fn sprite(
    instance: &str,
    image: &str,
    src: (u32, u32, u32, u32),
    dst: (f32, f32, f32, f32),
) -> Sprite {
    Sprite {
        instance: instance.into(),
        image: image.into(),
        sx: src.0,
        sy: src.1,
        sw: src.2,
        sh: src.3,
        x: dst.0,
        y: dst.1,
        w: dst.2,
        h: dst.3,
    }
}
