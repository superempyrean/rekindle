//! Acting as the user. Every function works only while the host is
//! delivering one of the user's own gestures to the plugin (a slot click, a
//! menu item, a palette or slash command, a keybind, a panel interaction),
//! and within that gesture's budget. From timers, events or `activate` they
//! return [`ActionError::NoGesture`]. The plugin's own activity in
//! [`crate::presence`] is the exception.
//!
//! - Local and self-only (`act-local`): mark read, open a reply or edit,
//!   insert text, open a DM, per-user volume, notes, saved messages, mute.
//! - Reactions (`react`; `act` implies it).
//! - Visible actions (`act`): send or forward a message (with files the
//!   plugin made), pin, delete, edit the user's own message, suppress
//!   embeds, vote, join voice, self mute and deafen, threads, typing,
//!   friend requests, invites and joining a guild.
//!
//! Moderating other members is [`crate::moderation`].

use crate::MessageRef;
pub use crate::raw::rekindle::plugin::action_types::{ActionError, OutgoingFile};
use crate::raw::rekindle::plugin::{actions as act, local_actions as local, reactions};

/// What every action returns.
pub type Result = std::result::Result<(), ActionError>;

/// A file for [`send_message`], not marked as a spoiler.
pub fn file(filename: &str, content_type: Option<&str>, data: Vec<u8>) -> OutgoingFile {
    OutgoingFile {
        filename: filename.into(),
        content_type: content_type.map(Into::into),
        data,
        spoiler: false,
    }
}

/// Sends a message as the user to `channel`, one they can send in: text,
/// files the plugin made, or both. Up to 10 files and 25 MiB in all; the
/// text may be empty when there are files. Needs `act`.
pub fn send_message(channel: u64, content: &str, files: &[OutgoingFile]) -> Result {
    act::send_message(channel, content, files)
}

// ---- act-local ----

/// Marks a channel read.
pub fn mark_read(channel: u64) -> Result {
    local::mark_read(channel)
}

/// Marks every channel of a guild read.
pub fn mark_guild_read(guild: u64) -> Result {
    local::mark_guild_read(guild)
}

/// Marks everything read, as the client's "Mark all read" does.
pub fn mark_all_read() -> Result {
    local::mark_all_read()
}

/// Marks a channel unread from `message` on, like Alt+click.
pub fn mark_unread(message: &MessageRef) -> Result {
    local::mark_unread(*message)
}

/// Opens a reply to `message` in the composer; the user still writes and
/// sends it. `mention` sets whether the reply pings its author.
pub fn start_reply(message: &MessageRef, mention: bool) -> Result {
    local::start_reply(*message, mention)
}

/// Opens the user's own message for editing.
pub fn start_edit(message: &MessageRef) -> Result {
    local::start_edit(*message)
}

/// Puts text into the composer at the cursor, for the user to send.
pub fn insert_text(text: &str) -> Result {
    local::insert_text(text)
}

/// Opens the DM with `user`, creating it if needed, and returns the DM
/// channel.
pub fn open_dm(user: u64) -> std::result::Result<u64, ActionError> {
    local::open_dm(user)
}

/// Sets the local volume of a user in voice, from 0 to 200 percent.
pub fn set_user_volume(user: u64, percent: f32) -> Result {
    local::set_user_volume(user, percent)
}

/// Mutes or unmutes a user in voice, for the user only.
pub fn set_user_local_mute(user: u64, muted: bool) -> Result {
    local::set_user_local_mute(user, muted)
}

/// Sets the user's private note on someone; `""` clears it.
pub fn set_note(user: u64, note: &str) -> Result {
    local::set_note(user, note)
}

/// Saves a message for later, or removes it from the saved messages.
pub fn save_message(message: &MessageRef, saved: bool) -> Result {
    local::save_message(*message, saved)
}

/// Collapses or expands a category in the channel list.
pub fn set_category_collapsed(category: u64, collapsed: bool) -> Result {
    local::set_category_collapsed(category, collapsed)
}

/// Mutes or unmutes the notifications of a channel or a guild.
pub fn set_muted(channel_or_guild: u64, muted: bool) -> Result {
    local::set_muted(channel_or_guild, muted)
}

// ---- react ----

/// Adds the user's reaction. `emoji` is Unicode or `name:id`. Up to 3 per
/// gesture.
pub fn react(message: &MessageRef, emoji: &str) -> Result {
    reactions::react(*message, emoji, false)
}

/// Adds the user's super reaction.
pub fn react_burst(message: &MessageRef, emoji: &str) -> Result {
    reactions::react(*message, emoji, true)
}

/// Removes the user's reaction. A super reaction stays.
pub fn unreact(message: &MessageRef, emoji: &str) -> Result {
    reactions::unreact(*message, emoji, false)
}

// ---- act ----

/// Pins or unpins a message. Needs the Pin Messages permission.
pub fn set_pinned(message: &MessageRef, pinned: bool) -> Result {
    act::set_pinned(*message, pinned)
}

/// Deletes the user's own message, or another's with Manage Messages. The
/// client shows its own confirmation unless the gesture held Shift.
pub fn delete_message(message: &MessageRef) -> Result {
    act::delete_message(*message)
}

/// Replaces the text of the user's own message.
pub fn edit_message(message: &MessageRef, content: &str) -> Result {
    act::edit_message(*message, content)
}

/// Hides the embeds on the user's own message.
pub fn suppress_embeds(message: &MessageRef) -> Result {
    act::suppress_embeds(*message)
}

/// Votes in a poll, by answer id.
pub fn vote(message: &MessageRef, answers: &[u32]) -> Result {
    act::vote(*message, answers)
}

/// Joins a voice channel; `None` leaves voice.
pub fn join_voice(channel: Option<u64>) -> Result {
    act::join_voice(channel)
}

/// Mutes or unmutes the user's microphone.
pub fn set_self_mute(muted: bool) -> Result {
    act::set_self_mute(muted)
}

/// Deafens or undeafens the user.
pub fn set_self_deaf(deafened: bool) -> Result {
    act::set_self_deaf(deafened)
}

/// Joins or leaves a thread.
pub fn set_thread_joined(thread: u64, joined: bool) -> Result {
    act::set_thread_joined(thread, joined)
}

/// Forwards a message to a channel the user can send in.
pub fn forward_message(message: &MessageRef, to_channel: u64) -> Result {
    act::forward_message(*message, to_channel)
}

/// Shows the user as typing in `channel` for a few seconds.
pub fn set_typing(channel: u64) -> Result {
    act::set_typing(channel)
}

/// With `add`, sends or accepts a friend request. Otherwise removes the
/// friend, or cancels or declines the request.
pub fn set_friend(user: u64, add: bool) -> Result {
    act::set_friend(user, add)
}

/// Creates an invite to `channel` and returns its code. A `max_age_secs` or
/// `max_uses` of 0 means no limit.
pub fn create_invite(
    channel: u64,
    max_age_secs: u32,
    max_uses: u32,
) -> std::result::Result<String, ActionError> {
    act::create_invite(channel, max_age_secs, max_uses)
}

/// Joins a guild from an invite code and returns the guild.
pub fn join_guild(invite_code: &str) -> std::result::Result<u64, ActionError> {
    act::join_guild(invite_code)
}
