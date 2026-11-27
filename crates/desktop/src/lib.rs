//! Desktop Phoenix's own GPUI, resolved from the build.
//!
//! The desktop client has no wire and no webview. rekindle builds its UI out
//! of the client's own `Div`, `StyleRefinement` and text elements. This crate
//! finds those types and their fields in a specific `Discord.exe` by shape —
//! strings, call graphs, `.pdata` extents and the derived `Debug::fmt` — and
//! writes styles at the offsets that build actually has. Nothing is a fixed
//! RVA.
//!
//! [`symbols::Symbols::resolve`] is the entry point; `rekindle-desktop check`
//! runs it on a build and fails loudly when a shape changed.

pub mod components;
pub mod cursor;
pub mod debug;
pub mod div;
pub mod field;
pub mod image;
pub mod menu;
pub mod native;
pub mod net;
pub mod palette;
pub mod settings;
pub mod symbols;
pub mod text;

pub use debug::StyleOffsets;
pub use div::Div;
pub use image::Image;
pub use menu::{Builder, Label, Menu, Target};
pub use native::{Written, write_style};
pub use symbols::{Report, Symbols};
pub use text::Text;
