//! Moderating other members (`moderate`). Each call works only during one
//! of the user's gestures, like [`crate::actions`], and needs the Discord
//! permission for it. The host asks the user to confirm.
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! # let (guild, user) = (1, 2);
//! let ten_minutes = now_millis() + 600_000;
//! let _ = moderation::timeout(guild, user, Some(ten_minutes), Some("spam"));
//! ```

pub use crate::raw::rekindle::plugin::action_types::ActionError;
use crate::raw::rekindle::plugin::moderation as raw;

/// What every call returns.
pub type Result = std::result::Result<(), ActionError>;

/// Removes `user` from `guild`. Needs `KICK_MEMBERS`.
pub fn kick(guild: u64, user: u64, reason: Option<&str>) -> Result {
    raw::kick(guild, user, reason)
}

/// Bans `user`, deleting their messages from the last
/// `delete_message_secs` seconds (up to 7 days). Needs `BAN_MEMBERS`.
pub fn ban(guild: u64, user: u64, delete_message_secs: u32, reason: Option<&str>) -> Result {
    raw::ban(guild, user, delete_message_secs, reason)
}

/// Lifts a ban. Needs `BAN_MEMBERS`.
pub fn unban(guild: u64, user: u64) -> Result {
    raw::unban(guild, user)
}

/// Times `user` out until `until_ms` (epoch milliseconds, up to 28 days
/// ahead), or ends a timeout with `None`. Needs `MODERATE_MEMBERS`.
pub fn timeout(guild: u64, user: u64, until_ms: Option<u64>, reason: Option<&str>) -> Result {
    raw::timeout(guild, user, until_ms, reason)
}

/// Server-mutes or unmutes `user` in voice. Needs `MUTE_MEMBERS`.
pub fn set_voice_mute(guild: u64, user: u64, muted: bool) -> Result {
    raw::set_voice_mute(guild, user, muted)
}

/// Server-deafens or undeafens `user` in voice. Needs `DEAFEN_MEMBERS`.
pub fn set_voice_deaf(guild: u64, user: u64, deafened: bool) -> Result {
    raw::set_voice_deaf(guild, user, deafened)
}

/// Moves `user` to a voice channel, or disconnects them with `None`. Needs
/// `MOVE_MEMBERS`.
pub fn move_member(guild: u64, user: u64, channel: Option<u64>) -> Result {
    raw::move_member(guild, user, channel)
}

/// Deletes 2 to 100 messages in `channel`, each under 14 days old. Needs
/// `MANAGE_MESSAGES`.
pub fn bulk_delete(channel: u64, messages: &[u64]) -> Result {
    raw::bulk_delete(channel, messages)
}
