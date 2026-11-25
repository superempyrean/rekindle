//! The client's own elements, by the names the build gives them (requires
//! `ui`).
//!
//! A build names the elements a plugin can reach: `composer.gift`,
//! `guild-search`, `expression-picker.tab`. [`names`] lists them, and
//! [`features`](crate::features) has each as `element:<name>`. A call with
//! a name the build doesn't have returns [`ElementError::Unavailable`].
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! # use rekindle_sdk::elements::{self, Place, PressMode};
//! # fn f() -> Result<(), elements::ElementError> {
//! elements::hide("composer.gift")?;
//! elements::on_press("guild-search", &PressMode::Observe)?;
//! elements::insert(
//!     "note",
//!     "guild-search",
//!     Place::After,
//!     &Vec::new().text("Searching this server only"),
//! )?;
//! # Ok(())
//! # }
//! ```
//!
//! A press on an element with [`on_press`] reaches
//! [`Plugin::on_ui`](crate::Plugin::on_ui) as `UiValue::Clicked`, with the
//! surface [`surface`] makes for its name and the name as the element.
//! Interactions with an insert's elements arrive with the surface
//! [`insert_surface`] makes for its id. Everything is undone when the plugin
//! unloads.

use crate::raw::rekindle::plugin::elements as raw;
use crate::ui::Element;
pub use raw::{ElementError, ElementStyle, Place, PressMode};

/// The names of the client's elements this build has, sorted.
pub fn names() -> Vec<String> {
    raw::names()
}

/// Takes the element and everything in it off the screen.
pub fn hide(name: &str) -> Result<(), ElementError> {
    raw::hide(name)
}

/// Restyles the element. A field left `None` keeps the client's value,
/// and a style with every field `None` drops the plugin's style.
pub fn style(name: &str, style: ElementStyle) -> Result<(), ElementError> {
    raw::style(name, style)
}

/// What a press on the element does.
pub fn on_press(name: &str, mode: &PressMode) -> Result<(), ElementError> {
    raw::on_press(name, mode)
}

/// Draws `elements` next to or inside the client's element `name`, as a
/// panel's rows are drawn. Calling it again with the same `id` replaces
/// them.
pub fn insert(id: &str, name: &str, at: Place, elements: &[Element]) -> Result<(), ElementError> {
    raw::insert(id, name, at, elements)
}

/// Takes down the insert with this id.
pub fn remove(id: &str) {
    raw::remove(id);
}

/// Undoes [`hide`], [`style`] and [`on_press`] for `name`.
pub fn reset(name: &str) {
    raw::reset(name);
}

/// The [`Plugin::on_ui`](crate::Plugin::on_ui) surface of a press on the
/// element `name`: `element:<name>`.
pub fn surface(name: &str) -> String {
    format!("element:{name}")
}

/// The [`Plugin::on_ui`](crate::Plugin::on_ui) surface of the insert `id`:
/// `insert:<id>`.
pub fn insert_surface(id: &str) -> String {
    format!("insert:{id}")
}
