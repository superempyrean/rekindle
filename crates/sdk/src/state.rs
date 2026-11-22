//! Client structure and the user's own state (requires `read-client`).
//!
//! Queries answer from what the client already holds. They never cause a
//! network request, and `None` means the client hasn't loaded it or the user
//! can't see it. Records are modelled on the client's stores (`GuildStore`,
//! `ChannelStore`, ...), not on gateway JSON, and never contain secrets.
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! if let Some(ch) = state::selection().channel.and_then(state::channel) {
//!     info!("the user is in #{}", ch.name);
//! }
//! ```

pub use crate::Member;
use crate::Target;
use crate::raw::rekindle::plugin::client_state as raw;
use crate::raw::rekindle::plugin::inspect;
pub use crate::records::{
    Channel, CustomStatus, Emoji, ForumTag, Guild, GuildFolder, GuildReadState, MyVoice, Overwrite,
    ReadState, Role, Selection, Sticker, ThreadMeta, User,
};

/// Where the user is: guild, channel, voice channel, side thread, focus and
/// idle.
pub fn selection() -> Selection {
    raw::get_selection()
}

/// The user.
pub fn me() -> User {
    raw::me()
}

/// The user's status: `"online"`, `"idle"`, `"dnd"` or `"invisible"`.
pub fn my_status() -> String {
    raw::my_status()
}

/// The user's custom status, if they set one.
pub fn my_custom_status() -> Option<CustomStatus> {
    raw::my_custom_status()
}

/// A user the client knows.
pub fn user(id: u64) -> Option<User> {
    raw::get_user(id)
}

/// The name the client shows for a user: the global display name, else the
/// username.
pub fn display_name(id: u64) -> Option<String> {
    user(id).map(|u| u.global_name.unwrap_or(u.username))
}

/// A guild the client has loaded.
pub fn guild(id: u64) -> Option<Guild> {
    raw::get_guild(id)
}

/// The user's guild ids, in sidebar order.
pub fn guild_ids() -> Vec<u64> {
    raw::guild_ids()
}

/// The user's guild folders.
pub fn guild_folders() -> Vec<GuildFolder> {
    raw::guild_folders()
}

/// A channel the client has loaded.
pub fn channel(id: u64) -> Option<Channel> {
    raw::get_channel(id)
}

/// A guild's channels in sidebar order. With `include_hidden`, the list
/// includes the channels the user can't view, with `visible: false`.
pub fn channels(guild: u64, include_hidden: bool) -> Vec<Channel> {
    raw::channels(guild, include_hidden)
}

/// The active threads the client knows under a channel.
pub fn threads(parent: u64) -> Vec<Channel> {
    raw::threads(parent)
}

/// The user's DMs and group DMs, most recent first.
pub fn private_channels() -> Vec<Channel> {
    raw::private_channels()
}

/// The DM channel with `user`, if one exists.
pub fn dm_channel(user: u64) -> Option<u64> {
    raw::dm_channel(user)
}

/// A role in a guild.
pub fn role(guild: u64, id: u64) -> Option<Role> {
    raw::get_role(guild, id)
}

/// A guild's roles, highest position first.
pub fn roles(guild: u64) -> Vec<Role> {
    raw::roles(guild)
}

/// A guild's custom emoji.
pub fn guild_emoji(guild: u64) -> Vec<Emoji> {
    raw::guild_emoji(guild)
}

/// The user's most-used emoji, most used first, at most `limit`. A Unicode
/// emoji has id 0 and the emoji itself in `name`.
pub fn frequent_emoji(limit: u32) -> Vec<Emoji> {
    raw::frequent_emoji(limit)
}

/// A guild's stickers.
pub fn guild_stickers(guild: u64) -> Vec<Sticker> {
    raw::guild_stickers(guild)
}

/// The user's membership in a guild.
pub fn my_member(guild: u64) -> Option<Member> {
    raw::my_member(guild)
}

/// The user's permission bits in a channel, or in a guild given its id. See
/// [`crate::consts::perms`].
pub fn my_permissions(channel_or_guild: u64) -> u64 {
    raw::my_permissions(channel_or_guild)
}

/// Whether the user has every permission in `bits`, in a channel or a
/// guild.
pub fn can(bits: u64, channel_or_guild: u64) -> bool {
    raw::can(bits, channel_or_guild)
}

/// A channel's permission overwrites.
pub fn permission_overwrites(channel: u64) -> Vec<Overwrite> {
    raw::permission_overwrites(channel)
}

/// A channel's unread and mention state.
pub fn read_state(channel: u64) -> Option<ReadState> {
    raw::get_read_state(channel)
}

/// A guild's unread and mention state.
pub fn guild_read_state(guild: u64) -> GuildReadState {
    raw::get_guild_read_state(guild)
}

/// The channels with unread messages, most recent first, at most `limit`.
pub fn unread_channels(limit: u32) -> Vec<ReadState> {
    raw::unread_channels(limit)
}

/// The user's voice connection, if any: channel, since when, self mute and
/// deafen, ping.
pub fn my_voice() -> Option<MyVoice> {
    raw::get_my_voice()
}

/// One of the user's client settings, by a key from [`my_setting_keys`]
/// (`"appearance.message-display-compact"`,
/// `"text-and-images.gif-auto-play"`, ...).
pub fn my_setting(key: &str) -> Option<String> {
    raw::my_setting(key)
}

/// The keys [`my_setting`] answers: a curated, read-only list.
pub fn my_setting_keys() -> Vec<String> {
    raw::my_setting_keys()
}

/// What the client holds about a target, as JSON with secrets stripped, for
/// a developer to look at. The shape may change in any build, so show it
/// and don't parse it. Messages need `read-messages`, and users
/// `read-users`.
pub fn raw_json(target: &Target) -> Option<String> {
    inspect::raw_json(target)
}
