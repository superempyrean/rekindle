//! Users, members, channels, roles, emoji, stickers, guilds, presences and
//! voice states.

use serde::Deserialize;

use super::{bits, nullable, opt_snowflake, snowflake, snowflakes};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct User {
    #[serde(deserialize_with = "snowflake")]
    pub id: u64,
    pub username: Option<String>,
    pub global_name: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub bot: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub system: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub public_flags: u64,
    pub accent_color: Option<u32>,
    pub avatar_decoration_data: Option<AvatarDecoration>,
    pub discriminator: Option<String>,
    pub avatar: Option<String>,
    pub banner: Option<String>,
    pub flags: Option<u64>,
    pub premium_type: Option<u8>,
    pub collectibles: Option<Collectibles>,
    pub display_name_styles: Option<DisplayNameStyles>,
    pub primary_guild: Option<PrimaryGuild>,
    pub bio: Option<String>,
    pub pronouns: Option<String>,
    // The current user's own account, in READY.
    pub locale: Option<String>,
    pub verified: Option<bool>,
    pub mfa_enabled: Option<bool>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub nsfw_allowed: Option<bool>,
    pub age_verification_status: Option<u8>,
    pub purchased_flags: Option<u64>,
    pub authenticator_types: Option<Vec<u8>>,
    pub store_country: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AvatarDecoration {
    #[serde(default)]
    pub asset: String,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub sku_id: Option<u64>,
    /// Unix seconds.
    pub expires_at: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Nameplate {
    #[serde(default)]
    pub asset: String,
    #[serde(default)]
    pub palette: String,
    #[serde(default)]
    pub label: String,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub sku_id: Option<u64>,
    pub expires_at: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Collectibles {
    pub nameplate: Option<Nameplate>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DisplayNameStyles {
    #[serde(default)]
    pub font_id: u32,
    #[serde(default)]
    pub effect_id: u32,
    #[serde(default, deserialize_with = "nullable")]
    pub colors: Vec<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PrimaryGuild {
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub identity_guild_id: Option<u64>,
    pub identity_enabled: Option<bool>,
    pub tag: Option<String>,
    pub badge: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Member {
    pub user: Option<User>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub user_id: Option<u64>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub guild_id: Option<u64>,
    pub nick: Option<String>,
    #[serde(default, deserialize_with = "snowflakes")]
    pub roles: Vec<u64>,
    pub joined_at: Option<String>,
    pub premium_since: Option<String>,
    pub communication_disabled_until: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub pending: bool,
    pub avatar: Option<String>,
    pub banner: Option<String>,
    pub bio: Option<String>,
    pub avatar_decoration_data: Option<AvatarDecoration>,
    #[serde(default, deserialize_with = "nullable")]
    pub flags: u32,
    #[serde(default, deserialize_with = "nullable")]
    pub mute: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub deaf: bool,
    pub unusual_dm_activity_until: Option<String>,
}

impl Member {
    pub fn user_id(&self) -> Option<u64> {
        self.user_id.or(self.user.as_ref().map(|u| u.id))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Overwrite {
    #[serde(deserialize_with = "snowflake")]
    pub id: u64,
    /// 0 role, 1 member.
    #[serde(rename = "type", default)]
    pub kind: u8,
    #[serde(default, deserialize_with = "bits")]
    pub allow: u64,
    #[serde(default, deserialize_with = "bits")]
    pub deny: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ThreadMetadata {
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub locked: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ForumTag {
    #[serde(deserialize_with = "snowflake")]
    pub id: u64,
    pub name: String,
    pub emoji_name: Option<String>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub emoji_id: Option<u64>,
    #[serde(default)]
    pub moderated: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Channel {
    #[serde(deserialize_with = "snowflake")]
    pub id: u64,
    #[serde(rename = "type", default)]
    pub kind: u16,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub guild_id: Option<u64>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub parent_id: Option<u64>,
    pub name: Option<String>,
    pub topic: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub position: i32,
    #[serde(default, deserialize_with = "nullable")]
    pub nsfw: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub rate_limit_per_user: u32,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub last_message_id: Option<u64>,
    #[serde(default, deserialize_with = "nullable")]
    pub permission_overwrites: Vec<Overwrite>,
    /// DMs under `DEDUPE_USER_OBJECTS`: ids into READY's `users`.
    #[serde(default, deserialize_with = "snowflakes")]
    pub recipient_ids: Vec<u64>,
    #[serde(default, deserialize_with = "nullable")]
    pub recipients: Vec<User>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub owner_id: Option<u64>,
    pub thread_metadata: Option<ThreadMetadata>,
    #[serde(default, deserialize_with = "nullable")]
    pub message_count: u32,
    #[serde(default, deserialize_with = "nullable")]
    pub member_count: u32,
    #[serde(default, deserialize_with = "snowflakes")]
    pub applied_tags: Vec<u64>,
    #[serde(default, deserialize_with = "nullable")]
    pub available_tags: Vec<ForumTag>,
    /// Present on a thread you've joined.
    pub member: Option<serde_json::Value>,
    pub icon: Option<String>,
    pub bitrate: Option<u32>,
    pub user_limit: Option<u32>,
    pub rtc_region: Option<String>,
    pub video_quality_mode: Option<u8>,
    #[serde(default, deserialize_with = "nullable")]
    pub flags: u64,
    pub default_auto_archive_duration: Option<u32>,
    pub default_thread_rate_limit_per_user: Option<u32>,
    pub default_reaction_emoji: Option<DefaultReaction>,
    pub default_sort_order: Option<u8>,
    pub default_forum_layout: Option<u8>,
    pub last_pin_timestamp: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub is_message_request: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub is_spam: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DefaultReaction {
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub emoji_id: Option<u64>,
    pub emoji_name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RoleColors {
    pub primary_color: Option<u32>,
    pub secondary_color: Option<u32>,
    pub tertiary_color: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Role {
    #[serde(deserialize_with = "snowflake")]
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub color: u32,
    pub colors: Option<RoleColors>,
    #[serde(default)]
    pub position: i32,
    #[serde(default, deserialize_with = "bits")]
    pub permissions: u64,
    #[serde(default)]
    pub hoist: bool,
    #[serde(default)]
    pub mentionable: bool,
    #[serde(default)]
    pub managed: bool,
    pub unicode_emoji: Option<String>,
    pub icon: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub flags: u32,
    /// Discord marks the boolean tags by their presence, with `null`.
    pub tags: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Emoji {
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub id: Option<u64>,
    pub name: Option<String>,
    #[serde(default)]
    pub animated: bool,
    #[serde(default = "yes")]
    pub available: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Sticker {
    #[serde(deserialize_with = "snowflake")]
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub format_type: u8,
}

/// A guild's own fields: `properties` under `CLIENT_STATE_V2`, the guild
/// object itself in `GUILD_UPDATE`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GuildProperties {
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub id: Option<u64>,
    #[serde(default)]
    pub name: String,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub owner_id: Option<u64>,
    pub description: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub features: Vec<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub premium_tier: u8,
    #[serde(default, deserialize_with = "nullable")]
    pub verification_level: u8,
    #[serde(default, deserialize_with = "nullable")]
    pub nsfw_level: u8,
    pub preferred_locale: Option<String>,
    pub icon: Option<String>,
    pub banner: Option<String>,
    pub splash: Option<String>,
    pub discovery_splash: Option<String>,
    pub vanity_url_code: Option<String>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub afk_channel_id: Option<u64>,
    #[serde(default, deserialize_with = "nullable")]
    pub afk_timeout: u32,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub system_channel_id: Option<u64>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub rules_channel_id: Option<u64>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub public_updates_channel_id: Option<u64>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub safety_alerts_channel_id: Option<u64>,
    #[serde(default, deserialize_with = "nullable")]
    pub system_channel_flags: u32,
    #[serde(default, deserialize_with = "nullable")]
    pub mfa_level: u8,
    #[serde(default, deserialize_with = "nullable")]
    pub explicit_content_filter: u8,
    #[serde(default, deserialize_with = "nullable")]
    pub default_message_notifications: u8,
    #[serde(default, deserialize_with = "nullable")]
    pub premium_progress_bar_enabled: bool,
    pub max_members: Option<u32>,
    pub max_video_channel_users: Option<u32>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub application_id: Option<u64>,
}

/// A guild in READY or `GUILD_CREATE`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Guild {
    #[serde(deserialize_with = "snowflake")]
    pub id: u64,
    #[serde(default)]
    pub unavailable: bool,
    pub properties: Option<GuildProperties>,
    /// "full", or "partial" when the client sent a cached version.
    pub data_mode: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub channels: Vec<Channel>,
    #[serde(default, deserialize_with = "nullable")]
    pub threads: Vec<Channel>,
    #[serde(default, deserialize_with = "nullable")]
    pub roles: Vec<Role>,
    #[serde(default, deserialize_with = "nullable")]
    pub emojis: Vec<Emoji>,
    #[serde(default, deserialize_with = "nullable")]
    pub stickers: Vec<Sticker>,
    #[serde(default, deserialize_with = "nullable")]
    pub members: Vec<Member>,
    #[serde(default, deserialize_with = "nullable")]
    pub presences: Vec<Presence>,
    #[serde(default, deserialize_with = "nullable")]
    pub voice_states: Vec<VoiceState>,
    pub member_count: Option<u32>,
    pub premium_subscription_count: Option<u32>,
    pub joined_at: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    pub large: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ActivityTimestamps {
    pub start: Option<u64>,
    pub end: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Activity {
    #[serde(rename = "type", default)]
    pub kind: u8,
    #[serde(default)]
    pub name: String,
    pub details: Option<String>,
    pub state: Option<String>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub application_id: Option<u64>,
    pub timestamps: Option<ActivityTimestamps>,
    pub emoji: Option<Emoji>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ClientStatus {
    pub desktop: Option<String>,
    pub mobile: Option<String>,
    pub web: Option<String>,
    pub embedded: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Presence {
    /// Partial: only `id` is certain.
    pub user: Option<User>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub user_id: Option<u64>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub guild_id: Option<u64>,
    #[serde(default)]
    pub status: String,
    #[serde(default, deserialize_with = "nullable")]
    pub client_status: ClientStatus,
    #[serde(default, deserialize_with = "nullable")]
    pub activities: Vec<Activity>,
}

impl Presence {
    pub fn user_id(&self) -> Option<u64> {
        self.user_id.or(self.user.as_ref().map(|u| u.id))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct VoiceState {
    #[serde(deserialize_with = "snowflake")]
    pub user_id: u64,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub guild_id: Option<u64>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub channel_id: Option<u64>,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub deaf: bool,
    #[serde(default)]
    pub self_mute: bool,
    #[serde(default)]
    pub self_deaf: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub self_video: bool,
    #[serde(default, deserialize_with = "nullable")]
    pub self_stream: bool,
    #[serde(default)]
    pub suppress: bool,
    pub member: Option<Member>,
}
