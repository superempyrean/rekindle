//! Client state read from a client's cache store, at query time.
//!
//! A client that keeps its state in a key-value store answers
//! `client-state` queries from that store, row by row, when a plugin asks.
//! rekindle keeps no copy. [`CacheStore`] knows the store's tables and
//! reads them through a [`StoreReader`], which the host implements over
//! whatever reaches the client's store.
//!
//! The tables are the ones Phoenix's guest writes, as recorded from build
//! 356207 (`cache_version` 34). A value is the JSON of Discord's API object,
//! and a key is a snowflake padded to 20 digits. A row in a guild's table
//! is keyed by the guild and the id (`<guild>:<id>`), with guild 0 for a
//! DM.
//!
//! | Table | Key | Value |
//! |---|---|---|
//! | `meta` | `me`, `selected`, `guild_order` | the user, `{guild_id, channel_id}`, guild ids in the user's order |
//! | `users` | user | a user |
//! | `guilds` | guild | `{properties, roles, emojis, stickers, member_count, joined_at_ms}` |
//! | `channels` | guild:channel | a channel, without its overwrites or topic |
//! | `channel_overwrites` | guild:channel | the channel's overwrites |
//! | `channel_topics` | guild:channel | the topic, a JSON string |
//! | `members` | guild:user | a member, `user` null and `user_id` set |
//! | `relationships` | user | `{id, type, user_id, since}` |
//! | `messages` | channel:message | a message, with its author |
//! | `user_guild_settings` | guild | the guild's notification settings, with `muted` |
//!
//! The store holds what the client keeps between launches: every guild, its
//! channels and roles, the members and users the client has seen, and the
//! newest messages of the channels the user opened. A query for anything
//! else answers nothing.

use serde::de::DeserializeOwned;

use crate::api;
use crate::backend::gateway::convert;
use crate::backend::gateway::wire;

/// Reads a client's key-value store.
pub trait StoreReader: Send + Sync {
    /// The value under `key`.
    fn get(&self, table: &str, key: &str) -> Option<Vec<u8>>;
    /// The rows under `keys` that exist, as (key, value).
    fn get_many(&self, table: &str, keys: &[String]) -> Vec<(String, Vec<u8>)>;
    /// Every row whose key starts with `prefix`, in key order.
    fn scan(&self, table: &str, prefix: &str) -> Vec<(String, Vec<u8>)>;
    /// The keys that start with `prefix`, in order.
    fn keys(&self, table: &str, prefix: &str) -> Vec<String> {
        self.scan(table, prefix)
            .into_iter()
            .map(|(k, _)| k)
            .collect()
    }
}

/// Every permission bit Discord defines (through bit 53).
const ALL_PERMISSIONS: u64 = (1 << 54) - 1;
const ADMINISTRATOR: u64 = 1 << 3;
const VIEW_CHANNEL: u64 = 1 << 10;
/// A DM channel's type.
const DM: u16 = 1;

/// Announcement, public and private threads (channel types 10 to 12).
fn is_thread(kind: u16) -> bool {
    (10..=12).contains(&kind)
}

/// A snowflake as the store keys it.
fn key(id: u64) -> String {
    format!("{id:020}")
}

/// The key of a row in a guild's table. Guild 0 holds the DMs.
fn pair(guild: u64, id: u64) -> String {
    format!("{guild:020}:{id:020}")
}

/// The id after the `:` in a pair key.
fn second(k: &str) -> Option<u64> {
    k.split_once(':')?.1.parse().ok()
}

fn parse<T: DeserializeOwned>(v: &[u8]) -> Option<T> {
    serde_json::from_slice(v).ok()
}

/// The client's state, read from its store.
pub struct CacheStore {
    reader: Box<dyn StoreReader>,
}

impl CacheStore {
    pub fn new(reader: Box<dyn StoreReader>) -> Self {
        Self { reader }
    }

    fn row<T: DeserializeOwned>(&self, table: &str, k: &str) -> Option<T> {
        parse(&self.reader.get(table, k)?)
    }

    /// The user, from `meta`'s `me`.
    pub fn me(&self) -> Option<api::User> {
        self.user_row("meta", "me").map(|u| user(&u))
    }

    fn me_id(&self) -> Option<u64> {
        self.user_row("meta", "me").map(|u| u.id)
    }

    /// A user's row. The guest keeps the user's own `store_country` as an
    /// object, where the gateway sends a string.
    fn user_row(&self, table: &str, k: &str) -> Option<wire::User> {
        let mut v: serde_json::Value = self.row(table, k)?;
        if let Some(o) = v.as_object_mut()
            && o.get("store_country").is_some_and(|c| !c.is_string())
        {
            o.remove("store_country");
        }
        serde_json::from_value(v).ok()
    }

    /// The guild and channel the user has open.
    pub fn selection(&self) -> Option<(Option<u64>, Option<u64>)> {
        #[derive(serde::Deserialize)]
        struct Selected {
            #[serde(default, deserialize_with = "wire::opt_snowflake")]
            guild_id: Option<u64>,
            #[serde(default, deserialize_with = "wire::opt_snowflake")]
            channel_id: Option<u64>,
        }
        let s: Selected = self.row("meta", "selected")?;
        Some((s.guild_id, s.channel_id))
    }

    pub fn user(&self, id: u64) -> Option<api::User> {
        match self.user_row("users", &key(id)) {
            Some(u) => Some(user(&u)),
            None => self.me().filter(|me| me.id == id),
        }
    }

    /// The user's guilds, in the order of their list.
    pub fn guild_ids(&self) -> Vec<u64> {
        let ordered: Option<Vec<String>> = self.row("meta", "guild_order");
        match ordered {
            Some(ids) => ids.iter().filter_map(|s| s.parse().ok()).collect(),
            None => self
                .reader
                .keys("guilds", "")
                .iter()
                .filter_map(|k| k.parse().ok())
                .collect(),
        }
    }

    fn guild_record(&self, id: u64) -> Option<StoredGuild> {
        self.row("guilds", &key(id))
    }

    pub fn guild(&self, id: u64) -> Option<api::Guild> {
        let g = self.guild_record(id)?;
        let props = g.props();
        let muted = self
            .row::<serde_json::Value>("user_guild_settings", &key(id))
            .and_then(|s| s.get("muted")?.as_bool())
            .unwrap_or(false);
        Some(api::Guild {
            id,
            name: props.name,
            owner: props.owner_id.unwrap_or(0),
            description: props.description,
            features: props.features,
            premium_tier: props.premium_tier,
            premium_subscribers: g.boosts(),
            verification_level: props.verification_level,
            nsfw_level: props.nsfw_level,
            preferred_locale: props.preferred_locale.unwrap_or_else(|| "en-US".into()),
            member_count: g.member_count,
            joined_ms: g.joined_at_ms,
            muted,
            icon: props.icon,
            banner: props.banner,
            splash: props.splash,
            discovery_splash: props.discovery_splash,
            vanity_url_code: props.vanity_url_code,
            afk_channel: props.afk_channel_id,
            afk_timeout_secs: props.afk_timeout,
            system_channel: props.system_channel_id,
            rules_channel: props.rules_channel_id,
            public_updates_channel: props.public_updates_channel_id,
            safety_alerts_channel: props.safety_alerts_channel_id,
            system_channel_flags: props.system_channel_flags,
            mfa_level: props.mfa_level,
            explicit_content_filter: props.explicit_content_filter,
            default_message_notifications: props.default_message_notifications,
            premium_progress_bar: props.premium_progress_bar_enabled,
            max_members: props.max_members,
            max_video_channel_users: props.max_video_channel_users,
            application: props.application_id,
            large: false,
            unavailable: false,
        })
    }

    pub fn roles(&self, guild: u64) -> Vec<api::Role> {
        self.guild_record(guild)
            .map(|g| {
                g.roles
                    .into_iter()
                    .map(|r| convert::role(r, guild))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn role(&self, guild: u64, id: u64) -> Option<api::Role> {
        self.roles(guild).into_iter().find(|r| r.id == id)
    }

    /// The guild a channel is in, or 0 for a DM, with the channel's row:
    /// one read for the DMs and every guild at once.
    fn locate(&self, id: u64) -> Option<(u64, wire::Channel)> {
        let guilds: Vec<u64> = std::iter::once(0).chain(self.guild_ids()).collect();
        let keys: Vec<String> = guilds.iter().map(|g| pair(*g, id)).collect();
        let (k, v) = self.reader.get_many("channels", &keys).into_iter().next()?;
        let guild = k.split_once(':')?.0.parse().ok()?;
        Some((guild, parse(&v)?))
    }

    pub fn channel(&self, id: u64) -> Option<api::Channel> {
        let (guild, c) = self.locate(id)?;
        Some(self.channel_record(guild, &c, None))
    }

    /// `c` as plugins see it. `perms`, when known, is the user's
    /// permissions there, for `visible`.
    fn channel_record(&self, guild: u64, c: &wire::Channel, perms: Option<u64>) -> api::Channel {
        let guild = (guild != 0).then_some(guild).or(c.guild_id);
        let recipients: Vec<api::UserRef> = c
            .recipient_ids
            .iter()
            .chain(c.recipients.iter().map(|u| &u.id))
            .map(|id| api::UserRef {
                id: *id,
                name: self.display_name(guild, *id),
            })
            .collect();
        let name = c.name.clone().unwrap_or_else(|| {
            recipients
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        });
        let topic = c
            .topic
            .clone()
            .or_else(|| guild.and_then(|g| self.row::<String>("channel_topics", &pair(g, c.id))));
        let visible = match (guild, perms) {
            (None, _) => true,
            (Some(_), Some(p)) => p & VIEW_CHANNEL != 0,
            (Some(g), None) => self
                .my_id_permissions(g, c)
                .is_none_or(|p| p & VIEW_CHANNEL != 0),
        };
        api::Channel {
            id: c.id,
            kind: c.kind,
            guild,
            parent: c.parent_id,
            name,
            topic,
            position: c.position,
            nsfw: c.nsfw,
            slowmode: c.rate_limit_per_user,
            last_message: c.last_message_id,
            recipients,
            thread: c.thread_metadata.as_ref().map(|t| api::ThreadMeta {
                owner: c.owner_id.unwrap_or(0),
                archived: t.archived,
                locked: t.locked,
                message_count: c.message_count,
                member_count: c.member_count,
                applied_tags: c.applied_tags.clone(),
                joined: c.member.is_some(),
            }),
            forum_tags: c
                .available_tags
                .iter()
                .map(|t| api::ForumTag {
                    id: t.id,
                    name: t.name.clone(),
                    emoji: t.emoji_name.clone().or(t.emoji_id.map(|i| i.to_string())),
                    moderated: t.moderated,
                })
                .collect(),
            muted: false,
            visible,
            owner: c.owner_id,
            icon: c.icon.clone(),
            bitrate: c.bitrate,
            user_limit: c.user_limit,
            rtc_region: c.rtc_region.clone(),
            video_quality: c.video_quality_mode,
            flag_bits: c.flags,
            default_auto_archive_mins: c.default_auto_archive_duration,
            default_thread_slowmode: c.default_thread_rate_limit_per_user,
            default_reaction: c
                .default_reaction_emoji
                .as_ref()
                .and_then(|r| r.emoji_name.clone().or(r.emoji_id.map(|i| i.to_string()))),
            default_sort: c.default_sort_order,
            default_layout: c.default_forum_layout,
            last_pin_ms: c.last_pin_timestamp.as_deref().and_then(convert::iso_ms),
            message_request: c.is_message_request,
            spam: c.is_spam,
        }
    }

    /// A guild's channels. Without `include_hidden`, only those the user
    /// can view.
    pub fn channels(&self, guild: u64, include_hidden: bool) -> Vec<api::Channel> {
        let prefix = format!("{guild:020}:");
        let rows: Vec<wire::Channel> = self
            .reader
            .scan("channels", &prefix)
            .iter()
            .filter_map(|(_, v)| parse(v))
            .collect();
        let me = self.me_id();
        let context = me.and_then(|me| self.permission_context(guild, me));
        let overwrites = self.overwrites_in(guild);
        rows.iter()
            .map(|c| {
                let perms = context.as_ref().map(|(base, roles, me)| {
                    let id = channel_for_overwrites(c);
                    let o = overwrites
                        .iter()
                        .find(|(i, _)| *i == id)
                        .map_or(&[][..], |(_, o)| o);
                    apply_overwrites(guild, *base, *me, roles, o)
                });
                self.channel_record(guild, c, perms)
            })
            .filter(|c| include_hidden || c.visible)
            .collect()
    }

    pub fn threads(&self, parent: u64) -> Vec<api::Channel> {
        let Some((guild, _)) = self.locate(parent) else {
            return Vec::new();
        };
        self.channels(guild, true)
            .into_iter()
            .filter(|c| c.parent == Some(parent) && is_thread(c.kind))
            .collect()
    }

    pub fn private_channels(&self) -> Vec<api::Channel> {
        self.reader
            .scan("channels", &format!("{:020}:", 0))
            .iter()
            .filter_map(|(_, v)| parse::<wire::Channel>(v))
            .map(|c| self.channel_record(0, &c, None))
            .collect()
    }

    pub fn dm_channel(&self, user: u64) -> Option<u64> {
        self.private_channels()
            .into_iter()
            .find(|c| c.kind == DM && c.recipients.iter().any(|r| r.id == user))
            .map(|c| c.id)
    }

    pub fn member(&self, guild: u64, user: u64) -> Option<api::Member> {
        let m: wire::Member = self.row("members", &pair(guild, user))?;
        convert::member(guild, &m)
    }

    /// The members the client holds, up to `limit`, and how many it holds.
    pub fn loaded_members(&self, guild: u64, limit: u32) -> (Vec<api::Member>, u32) {
        let rows = self.reader.scan("members", &format!("{guild:020}:"));
        let total = u32::try_from(rows.len()).unwrap_or(u32::MAX);
        let members = rows
            .iter()
            .filter_map(|(_, v)| parse::<wire::Member>(v))
            .filter_map(|m| convert::member(guild, &m))
            .take(limit as usize)
            .collect();
        (members, total)
    }

    pub fn relationships(&self) -> Vec<api::Relationship> {
        self.reader
            .scan("relationships", "")
            .iter()
            .filter_map(|(_, v)| parse::<wire::Relationship>(v))
            .filter_map(|r| convert::relationship(&r))
            .collect()
    }

    /// Nickname in `guild`, else global name, else username.
    pub fn display_name(&self, guild: Option<u64>, user: u64) -> String {
        let nick = guild
            .and_then(|g| self.member(g, user))
            .and_then(|m| m.nick);
        let u = self.user(user);
        nick.or(u.as_ref().and_then(|u| u.global_name.clone()))
            .or(u.map(|u| u.username))
            .unwrap_or_default()
    }

    fn message_record(&self, guild: Option<u64>, m: &wire::Message) -> api::Message {
        let names = |g: Option<u64>, u: u64| self.display_name(g, u);
        let author_of = |channel: u64, id: u64| {
            self.row::<wire::Message>("messages", &pair(channel, id))
                .and_then(|m| m.author.map(|a| a.id))
        };
        convert::message(m, guild, None, &names, &author_of)
    }

    pub fn message(&self, channel: u64, id: u64) -> Option<api::Message> {
        let m: wire::Message = self.row("messages", &pair(channel, id))?;
        let guild = self.locate(channel).map(|(g, _)| g).filter(|g| *g != 0);
        Some(self.message_record(guild, &m))
    }

    /// The author of message `id` in the channel the user has open, for a
    /// target that names only the message.
    pub fn message_in_selected(&self, id: u64) -> Option<api::Message> {
        let (_, channel) = self.selection()?;
        self.message(channel?, id)
    }

    /// Newest first, older than `before` if given, at most `limit`.
    pub fn loaded_messages(
        &self,
        channel: u64,
        before: Option<u64>,
        limit: u32,
    ) -> Vec<api::Message> {
        let guild = self.locate(channel).map(|(g, _)| g).filter(|g| *g != 0);
        self.reader
            .scan("messages", &format!("{channel:020}:"))
            .iter()
            .rev()
            .filter(|(k, _)| before.is_none_or(|b| second(k).is_some_and(|id| id < b)))
            .filter_map(|(_, v)| parse::<wire::Message>(v))
            .take(limit as usize)
            .map(|m| self.message_record(guild, &m))
            .collect()
    }

    pub fn permission_overwrites(&self, channel: u64) -> Vec<api::Overwrite> {
        let Some((guild, _)) = self.locate(channel).filter(|(g, _)| *g != 0) else {
            return Vec::new();
        };
        self.overwrites(guild, channel)
    }

    fn overwrites(&self, guild: u64, channel: u64) -> Vec<api::Overwrite> {
        self.row::<Vec<wire::Overwrite>>("channel_overwrites", &pair(guild, channel))
            .unwrap_or_default()
            .iter()
            .map(overwrite)
            .collect()
    }

    /// Every channel's overwrites in `guild`, by channel.
    fn overwrites_in(&self, guild: u64) -> Vec<(u64, Vec<api::Overwrite>)> {
        self.reader
            .scan("channel_overwrites", &format!("{guild:020}:"))
            .iter()
            .filter_map(|(k, v)| {
                let o: Vec<wire::Overwrite> = parse(v)?;
                Some((second(k)?, o.iter().map(overwrite).collect()))
            })
            .collect()
    }

    /// A member's guild-wide permissions and roles, and the user.
    fn permission_context(&self, guild: u64, user: u64) -> Option<(u64, Vec<u64>, u64)> {
        let g = self.guild_record(guild)?;
        let owner = g.props().owner_id;
        let roles = self.member(guild, user)?.roles;
        if owner == Some(user) {
            return Some((ALL_PERMISSIONS, roles, user));
        }
        let role_bits = |id: u64| {
            g.roles
                .iter()
                .find(|r| r.id == id)
                .map_or(0, |r| r.permissions)
        };
        let bits = roles
            .iter()
            .fold(role_bits(guild), |b, r| b | role_bits(*r));
        let base = if bits & ADMINISTRATOR == 0 {
            bits
        } else {
            ALL_PERMISSIONS
        };
        Some((base, roles, user))
    }

    fn my_id_permissions(&self, guild: u64, c: &wire::Channel) -> Option<u64> {
        let (base, roles, me) = self.permission_context(guild, self.me_id()?)?;
        let o = self.overwrites(guild, channel_for_overwrites(c));
        Some(apply_overwrites(guild, base, me, &roles, &o))
    }

    /// `user`'s permissions in a channel or guild.
    pub fn member_permissions(&self, user: u64, channel_or_guild: u64) -> Option<u64> {
        if let Some((base, _, _)) = self.permission_context(channel_or_guild, user) {
            return Some(base);
        }
        let (guild, c) = self.locate(channel_or_guild).filter(|(g, _)| *g != 0)?;
        let (base, roles, user) = self.permission_context(guild, user)?;
        let o = self.overwrites(guild, channel_for_overwrites(&c));
        Some(apply_overwrites(guild, base, user, &roles, &o))
    }

    pub fn my_permissions(&self, channel_or_guild: u64) -> u64 {
        self.me_id()
            .and_then(|me| self.member_permissions(me, channel_or_guild))
            .unwrap_or(0)
    }

    /// The row the client keeps for `target`, as its JSON.
    pub fn raw_json(&self, target: &api::Target) -> Option<String> {
        let raw = match target {
            api::Target::Message(m) => self.reader.get("messages", &pair(m.channel, m.id))?,
            api::Target::User((u, _)) => self.reader.get("users", &key(u.id))?,
            api::Target::Channel(c) => {
                let (guild, _) = self.locate(c.id)?;
                self.reader.get("channels", &pair(guild, c.id))?
            }
            api::Target::Guild(g) => self.reader.get("guilds", &key(g.id))?,
            _ => return None,
        };
        String::from_utf8(raw).ok()
    }

    /// The colour of a member's highest coloured role.
    pub fn member_color(&self, guild: u64, user: u64) -> Option<u32> {
        let mine = self.member(guild, user)?.roles;
        let g = self.guild_record(guild)?;
        let mut roles: Vec<&wire::Role> = g
            .roles
            .iter()
            .filter(|r| r.color != 0 && mine.contains(&r.id))
            .collect();
        roles.sort_by_key(|r| std::cmp::Reverse(r.position));
        roles.first().map(|r| r.color)
    }
}

/// A guild's row: the gateway's `properties`, its roles and counts.
#[derive(serde::Deserialize)]
struct StoredGuild {
    #[serde(default)]
    properties: serde_json::Value,
    #[serde(default)]
    roles: Vec<wire::Role>,
    member_count: Option<u32>,
    joined_at_ms: Option<u64>,
}

impl StoredGuild {
    fn props(&self) -> wire::GuildProperties {
        serde_json::from_value(self.properties.clone()).unwrap_or_default()
    }

    /// The boosts, which the guest keeps with the properties.
    fn boosts(&self) -> u32 {
        self.properties
            .get("premium_subscription_count")
            .and_then(serde_json::Value::as_u64)
            .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
    }
}

/// A thread takes its parent's overwrites.
fn channel_for_overwrites(c: &wire::Channel) -> u64 {
    match c.parent_id {
        Some(parent) if is_thread(c.kind) => parent,
        _ => c.id,
    }
}

/// `base` with a channel's overwrites applied, in Discord's order: the
/// `@everyone` overwrite, then the member's roles' together, then the
/// member's own.
fn apply_overwrites(guild: u64, base: u64, user: u64, roles: &[u64], o: &[api::Overwrite]) -> u64 {
    if base & ADMINISTRATOR != 0 {
        return ALL_PERMISSIONS;
    }
    let apply = |bits: u64, allow: u64, deny: u64| (bits & !deny) | allow;
    let mut bits = base;
    if let Some(e) = o.iter().find(|o| !o.member && o.id == guild) {
        bits = apply(bits, e.allow, e.deny);
    }
    let (allow, deny) = o
        .iter()
        .filter(|o| !o.member && roles.contains(&o.id))
        .fold((0, 0), |(a, d), o| (a | o.allow, d | o.deny));
    bits = apply(bits, allow, deny);
    if let Some(m) = o.iter().find(|o| o.member && o.id == user) {
        bits = apply(bits, m.allow, m.deny);
    }
    bits
}

fn overwrite(o: &wire::Overwrite) -> api::Overwrite {
    api::Overwrite {
        id: o.id,
        member: o.kind == 1,
        allow: o.allow,
        deny: o.deny,
    }
}

fn user(u: &wire::User) -> api::User {
    let mut out = api::User {
        id: u.id,
        username: u.username.clone().unwrap_or_default(),
        global_name: u.global_name.clone(),
        bot: u.bot,
        system: u.system,
        public_flags: u.public_flags,
        accent: u.accent_color.map(convert::rgb),
        ..Default::default()
    };
    convert::user_fields(u, &mut out);
    out
}

#[cfg(test)]
mod tests;
