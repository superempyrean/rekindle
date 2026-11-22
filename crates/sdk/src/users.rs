//! Other people, as the client sees them (requires `read-users`, or
//! `read-messages`, which implies it): presence and activities, members,
//! typing, voice, and the user's relationships and notes. Nothing here
//! fetches.

use crate::raw::rekindle::plugin::users as raw;
pub use crate::records::{
    Activity, ClientStatus, Presence, Relationship, RelationshipKind, VoiceState,
};
use crate::{Color, Member};

/// A user's presence as the client knows it, in `guild` if given.
pub fn presence(user: u64, guild: Option<u64>) -> Option<Presence> {
    raw::get_presence(user, guild)
}

/// A member the client has loaded. It never requests one.
pub fn member(guild: u64, user: u64) -> Option<Member> {
    raw::get_member(guild, user)
}

/// The loaded members of a guild, at most `limit` (up to 1000), and how many
/// are loaded.
pub fn loaded_members(guild: u64, limit: u32) -> (Vec<Member>, u32) {
    raw::loaded_members(guild, limit)
}

/// A member's effective permissions in a channel or guild, if the client
/// has them loaded.
pub fn member_permissions(user: u64, channel_or_guild: u64) -> Option<u64> {
    raw::member_permissions(user, channel_or_guild)
}

/// A member's display colour: their highest coloured role's.
pub fn member_color(guild: u64, user: u64) -> Option<Color> {
    raw::member_color(guild, user)
}

/// Who is typing in a channel, oldest first.
pub fn typing(channel: u64) -> Vec<u64> {
    raw::typing(channel)
}

/// The voice states in a voice channel.
pub fn voice_states(channel: u64) -> Vec<VoiceState> {
    raw::voice_states(channel)
}

/// A user's voice state, if they're in voice.
pub fn voice_state(user: u64) -> Option<VoiceState> {
    raw::get_voice_state(user)
}

/// Whether a user is speaking now. Known only in the user's own voice
/// channel.
pub fn speaking(user: u64) -> bool {
    raw::speaking(user)
}

/// The user's relationships: friends, blocks and requests.
pub fn relationships() -> Vec<Relationship> {
    raw::relationships()
}

/// The user's relationship with someone, if any.
pub fn relationship(user: u64) -> Option<RelationshipKind> {
    raw::get_relationship(user)
}

/// The user's private note on someone.
pub fn note(user: u64) -> Option<String> {
    raw::note(user)
}

/// The guilds the user shares with someone, from loaded members.
pub fn mutual_guilds(user: u64) -> Vec<u64> {
    raw::mutual_guilds(user)
}
