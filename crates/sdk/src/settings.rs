//! The plugin's settings, declared as a schema in the manifest. The host
//! keeps the values, renders the settings page and validates every value.
//! No capability is needed.
//!
//! ```toml
//! [[package.metadata.rekindle.settings]]
//! key = "keywords"
//! type = "string-list"
//! label = "Keywords to highlight"
//! ```
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! let words = settings::items("keywords");
//! ```
//!
//! The types are `toggle`, `select`, `slider`, `number`, `text`, `color`,
//! `keybind`, `string-list`, `table`, `users`, `channels` and `guilds`.
//! Changes arrive in
//! [`Plugin::on_settings_changed`](crate::Plugin::on_settings_changed).
//! The typed readers return `false`, `0`, an empty value or `None` when the
//! key is missing or holds another type.

use crate::Color;
use crate::raw::rekindle::plugin::settings as raw;
pub use raw::SettingValue;

/// The current value of `key`: the schema's default until the user changes
/// it. `None` if the schema has no such key.
pub fn get(key: &str) -> Option<SettingValue> {
    raw::get(key)
}

/// Sets a value, validated against the schema.
/// [`Plugin::on_settings_changed`](crate::Plugin::on_settings_changed)
/// follows once the current call returns.
#[expect(
    clippy::needless_pass_by_value,
    reason = "callers build the value inline: `set(\"on\", SettingValue::Boolean(true))`"
)]
pub fn set(key: &str, value: SettingValue) -> Result<(), String> {
    raw::set(key, &value)
}

/// Every key in the schema.
pub fn keys() -> Vec<String> {
    raw::keys()
}

/// A `toggle`'s value.
pub fn bool(key: &str) -> bool {
    matches!(get(key), Some(SettingValue::Boolean(true)))
}

/// A `slider`'s or `number`'s value.
pub fn number(key: &str) -> Option<f64> {
    match get(key)? {
        SettingValue::Number(n) => Some(n),
        _ => None,
    }
}

/// A `text`'s or `keybind`'s value.
pub fn text(key: &str) -> String {
    match get(key) {
        Some(SettingValue::Text(t) | SettingValue::Keybind(t)) => t,
        _ => String::new(),
    }
}

/// A `select`'s chosen index.
pub fn selected(key: &str) -> u32 {
    match get(key) {
        Some(SettingValue::Selected(i)) => i,
        _ => 0,
    }
}

/// A `color`'s value.
pub fn color(key: &str) -> Option<Color> {
    match get(key)? {
        SettingValue::Color(c) => Some(c),
        _ => None,
    }
}

/// A `string-list`'s items.
pub fn items(key: &str) -> Vec<String> {
    match get(key) {
        Some(SettingValue::Items(v)) => v,
        _ => Vec::new(),
    }
}

/// A `table`'s rows, each a list of cells.
pub fn rows(key: &str) -> Vec<Vec<String>> {
    match get(key) {
        Some(SettingValue::Rows(v)) => v,
        _ => Vec::new(),
    }
}

/// The ids a `users`, `channels` or `guilds` picker holds.
pub fn ids(key: &str) -> Vec<u64> {
    match get(key) {
        Some(SettingValue::Ids(v)) => v,
        _ => Vec::new(),
    }
}
