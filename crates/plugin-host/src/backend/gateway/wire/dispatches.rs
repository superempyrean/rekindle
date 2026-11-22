//! The bodies of dispatches that aren't one object: deletes, reaction and
//! poll votes, typing, relationships, read states, settings, `READY` and
//! the rest.

use serde::Deserialize;

use super::{
    Channel, Emoji, Guild, Member, Presence, Role, Sticker, User, VoiceState, nullable,
    opt_snowflake, snowflake, snowflakes,
};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MessageDelete {
    #[serde(deserialize_with = "snowflake")]
    pub id: u64,
    #[serde(deserialize_with = "snowflake")]
    pub channel_id: u64,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub guild_id: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MessageDeleteBulk {
    #[serde(deserialize_with = "snowflakes")]
    pub ids: Vec<u64>,
    #[serde(deserialize_with = "snowflake")]
    pub channel_id: u64,
}

/// `MESSAGE_REACTION_ADD` / _REMOVE / _`REMOVE_ALL` / _`REMOVE_EMOJI`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReactionEvent {
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub user_id: Option<u64>,
    #[serde(deserialize_with = "snowflake")]
    pub message_id: u64,
    #[serde(deserialize_with = "snowflake")]
    pub channel_id: u64,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub guild_id: Option<u64>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub message_author_id: Option<u64>,
    pub emoji: Option<Emoji>,
    #[serde(default)]
    pub burst: bool,
    /// 0 normal, 1 burst (on removes).
    #[serde(rename = "type", default)]
    pub kind: u8,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PollVote {
    #[serde(deserialize_with = "snowflake")]
    pub user_id: u64,
    #[serde(deserialize_with = "snowflake")]
    pub message_id: u64,
    #[serde(deserialize_with = "snowflake")]
    pub channel_id: u64,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub guild_id: Option<u64>,
    pub answer_id: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Typing {
    #[serde(deserialize_with = "snowflake")]
    pub user_id: u64,
    #[serde(deserialize_with = "snowflake")]
    pub channel_id: u64,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub guild_id: Option<u64>,
    /// Seconds.
    #[serde(default)]
    pub timestamp: u64,
    pub member: Option<Member>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Relationship {
    /// The other user.
    #[serde(deserialize_with = "snowflake")]
    pub id: u64,
    #[serde(rename = "type", default)]
    pub kind: u8,
    pub user: Option<User>,
    pub nickname: Option<String>,
    pub since: Option<String>,
    #[serde(default)]
    pub user_ignored: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReadStateEntry {
    #[serde(deserialize_with = "snowflake")]
    pub id: u64,
    /// 0 for a channel; other types (guild events, notification center)
    /// aren't channels.
    #[serde(default)]
    pub read_state_type: u8,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub last_message_id: Option<u64>,
    #[serde(default, deserialize_with = "nullable")]
    pub mention_count: u32,
    #[serde(default, deserialize_with = "nullable")]
    pub flags: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ChannelOverride {
    #[serde(deserialize_with = "snowflake")]
    pub channel_id: u64,
    #[serde(default)]
    pub muted: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GuildSettings {
    /// None: the DM settings.
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub guild_id: Option<u64>,
    #[serde(default)]
    pub muted: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub channel_overrides: Vec<ChannelOverride>,
}

/// `{ "entries": [...] }`, the versioned form, or a bare list.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Versioned<T> {
    Entries { entries: Vec<T> },
    List(Vec<T>),
}

impl<T> Versioned<T> {
    pub fn into_vec(self) -> Vec<T> {
        match self {
            Versioned::Entries { entries } => entries,
            Versioned::List(v) => v,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MergedPresences {
    #[serde(default, deserialize_with = "nullable")]
    pub friends: Vec<Presence>,
    /// Indexed like READY's `guilds`.
    #[serde(default, deserialize_with = "nullable")]
    pub guilds: Vec<Vec<Presence>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Ready {
    pub user: User,
    #[serde(default, deserialize_with = "nullable")]
    pub users: Vec<User>,
    #[serde(default, deserialize_with = "nullable")]
    pub guilds: Vec<Guild>,
    /// Indexed like `guilds`.
    #[serde(default, deserialize_with = "nullable")]
    pub merged_members: Vec<Vec<Member>>,
    #[serde(default, deserialize_with = "nullable")]
    pub private_channels: Vec<Channel>,
    #[serde(default, deserialize_with = "nullable")]
    pub relationships: Vec<Relationship>,
    pub read_state: Option<Versioned<ReadStateEntry>>,
    pub user_guild_settings: Option<Versioned<GuildSettings>>,
    #[serde(default, deserialize_with = "nullable")]
    pub notes: std::collections::BTreeMap<String, String>,
    /// Base64 `PreloadedUserSettings` ([`super::settings`]).
    pub user_settings_proto: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SupplementalGuild {
    #[serde(deserialize_with = "snowflake")]
    pub id: u64,
    #[serde(default, deserialize_with = "nullable")]
    pub voice_states: Vec<VoiceState>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReadySupplemental {
    #[serde(default, deserialize_with = "nullable")]
    pub guilds: Vec<SupplementalGuild>,
    #[serde(default, deserialize_with = "nullable")]
    pub merged_members: Vec<Vec<Member>>,
    #[serde(default, deserialize_with = "nullable")]
    pub merged_presences: MergedPresences,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GuildDelete {
    #[serde(deserialize_with = "snowflake")]
    pub id: u64,
    #[serde(default)]
    pub unavailable: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GuildRole {
    #[serde(deserialize_with = "snowflake")]
    pub guild_id: u64,
    pub role: Option<Role>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub role_id: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GuildEmojis {
    #[serde(deserialize_with = "snowflake")]
    pub guild_id: u64,
    #[serde(default)]
    pub emojis: Vec<Emoji>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GuildStickers {
    #[serde(deserialize_with = "snowflake")]
    pub guild_id: u64,
    #[serde(default)]
    pub stickers: Vec<Sticker>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MemberRemove {
    #[serde(deserialize_with = "snowflake")]
    pub guild_id: u64,
    pub user: User,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MembersChunk {
    #[serde(deserialize_with = "snowflake")]
    pub guild_id: u64,
    #[serde(default)]
    pub members: Vec<Member>,
    #[serde(default, deserialize_with = "nullable")]
    pub presences: Vec<Presence>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ThreadListSync {
    #[serde(deserialize_with = "snowflake")]
    pub guild_id: u64,
    #[serde(default)]
    pub threads: Vec<Channel>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PinsUpdate {
    #[serde(deserialize_with = "snowflake")]
    pub channel_id: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MessageAck {
    #[serde(deserialize_with = "snowflake")]
    pub channel_id: u64,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub message_id: Option<u64>,
    #[serde(default)]
    pub mention_count: Option<u32>,
    #[serde(default)]
    pub manual: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NoteUpdate {
    #[serde(deserialize_with = "snowflake")]
    pub id: u64,
    #[serde(default)]
    pub note: String,
}

/// `SESSIONS_REPLACE`: one entry per session of the account. Only the
/// statuses are read.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub active: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SettingsProto {
    /// 1: `PreloadedUserSettings`; 2: frecency; 3: test settings.
    #[serde(rename = "type", default)]
    pub kind: u8,
    #[serde(default)]
    pub proto: String,
}

/// `USER_SETTINGS_PROTO_UPDATE`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SettingsProtoUpdate {
    pub settings: SettingsProto,
}
