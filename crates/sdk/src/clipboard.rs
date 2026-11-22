//! Write-only clipboard (requires `clipboard`).
//!
//! Writing works only while the host is delivering one of the user's own
//! gestures to the plugin: a click in
//! [`Plugin::on_slot`](crate::Plugin::on_slot),
//! [`Plugin::on_command`](crate::Plugin::on_command) or
//! [`Plugin::on_ui`](crate::Plugin::on_ui). Anywhere else (timers, events,
//! `activate`) it fails with [`ClipboardError::NoGesture`]. The clipboard
//! can't be read.
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! # struct CopyIds;
//! # impl Plugin for CopyIds {
//! #     fn activate(_: &Granted) -> Result<Self, String> { Ok(CopyIds) }
//! fn on_slot(&mut self, ev: SlotEvent) {
//!     if let SlotEvent::Clicked(SlotClick { target: Target::User((u, _)), .. }) = ev
//!         && let Err(e) = clipboard::write_text(&u.id.to_string())
//!     {
//!         ui::toast_kind(&format!("Couldn't copy: {e:?}"), ToastKind::Error);
//!     }
//! }
//! # }
//! ```

use crate::raw::rekindle::plugin::clipboard as raw;
pub use raw::ClipboardError;

/// Replaces the clipboard's contents with `text`, at most 64 KiB.
pub fn write_text(text: &str) -> Result<(), ClipboardError> {
    raw::write_text(text)
}

/// Puts a PNG on the clipboard, at most 8 MiB.
pub fn write_png(png: &[u8]) -> Result<(), ClipboardError> {
    raw::write_png(png)
}
