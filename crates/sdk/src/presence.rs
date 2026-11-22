//! The user's presence (requires `presence`): status, custom status,
//! nickname and rich presence. [`set_status`] and [`set_nickname`] work
//! only during a gesture, like [`crate::actions`]. [`set_custom_status`] and
//! [`set_rich_presence`] are the plugin's own activity and work at any time,
//! from a timer or an event too, up to 5 changes per 20 s. Past that they
//! return [`ActionError::RateLimited`].

pub use crate::raw::rekindle::plugin::action_types::ActionError;
use crate::raw::rekindle::plugin::presence as raw;
pub use raw::RichPresence;

/// Sets the user's status: `"online"`, `"idle"`, `"dnd"` or `"invisible"`.
pub fn set_status(status: &str) -> Result<(), ActionError> {
    raw::set_status(status)
}

/// Sets the user's custom status. With neither text nor emoji, it clears
/// the status.
pub fn set_custom_status(
    text: Option<&str>,
    emoji: Option<&str>,
    expires_ms: Option<u64>,
) -> Result<(), ActionError> {
    raw::set_custom_status(text, emoji, expires_ms)
}

/// Sets the user's nickname in `guild`; `None` resets it. Needs the Change
/// Nickname permission.
pub fn set_nickname(guild: u64, nick: Option<&str>) -> Result<(), ActionError> {
    raw::set_nickname(guild, nick)
}

/// Shows a rich-presence activity of the plugin's own; `None` clears it.
/// Its images are the plugin's, registered with
/// [`ui::register_image`](crate::ui::register_image).
pub fn set_rich_presence(activity: Option<&RichPresence>) -> Result<(), ActionError> {
    raw::set_rich_presence(activity)
}

/// A "Playing `name`" activity with nothing else set.
pub fn playing(name: &str) -> RichPresence {
    RichPresence {
        kind: 0,
        name: name.into(),
        details: None,
        state: None,
        started_ms: None,
        ends_ms: None,
        large_image: None,
        large_text: None,
        small_image: None,
        small_text: None,
        buttons: Vec::new(),
    }
}
