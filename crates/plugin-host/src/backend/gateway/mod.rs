//! Plugin events from the gateway's dispatch stream.
//!
//! rekindle keeps no copy of the client's state. Plugin queries read the
//! client, and the dispatches the client receives (`MESSAGE_CREATE`,
//! `GUILD_MEMBER_UPDATE`, ...) only say what changed. [`Dispatch`] turns
//! one dispatch into plugin events, in three steps around the client:
//!
//! 1. [`Dispatch::new`], before the client processes it, reads what the
//!    events need from before the change: the message an edit or delete
//!    replaces, the relationship or voice channel it changes.
//! 2. [`Dispatch::filter`] runs it past the plugins' `event-filter`s.
//!    `None`: a plugin dropped it, and the client must not process it.
//! 3. The client processes `dispatch.d`, then [`Dispatch::events`] reads
//!    the new state from the client and returns the events.
//!
//! On a real build this needs one hook point, where the client receives
//! decoded dispatches, and the gateway protocol doesn't change when the
//! client's UI does. The decoder in [`wire`] reads only the fields
//! rekindle uses, so session ids, tokens, voice session ids and activity
//! secrets are skipped while parsing. The dispatches that carry nothing
//! else are never forwarded, and any other dispatch rekindle doesn't
//! translate reaches plugins as `raw` after
//! [`redact`](crate::runtime::redact).

pub mod convert;
pub mod settings;
pub mod wire;

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::ClientBackend;
use crate::api::{self, Event};
use wire::parse;

/// Dispatches that carry credentials or session state and nothing a plugin
/// may see. They are never forwarded as `raw`.
const NEVER_FORWARDED: &[&str] = &[
    "READY",
    "READY_SUPPLEMENTAL",
    "RESUMED",
    "SESSIONS_REPLACE",
    "VOICE_SERVER_UPDATE",
    "STREAM_SERVER_UPDATE",
    "STREAM_CREATE",
    "USER_CONNECTIONS_UPDATE",
    "USER_SETTINGS_PROTO_UPDATE",
    "USER_REQUIRED_ACTION_UPDATE",
    "PASSIVE_UPDATE_V1",
    "PASSIVE_UPDATE_V2",
];

/// Prefixes of dispatches that are never forwarded (`AUTH_SESSION_CHANGE`,
/// `REMOTE_AUTH_*`, `USER_PAYMENT_*`, ...).
const NEVER_FORWARDED_PREFIXES: &[&str] = &[
    "AUTH_",
    "REMOTE_AUTH",
    "USER_PAYMENT",
    "BILLING_",
    "ENTITLEMENT_",
];

fn never_forwarded(name: &str) -> bool {
    NEVER_FORWARDED.contains(&name) || NEVER_FORWARDED_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// A `GUILD_CREATE` for a guild the user joined longer ago than this is the
/// client loading it (lazily, or after an outage), not a join.
const JOIN_WINDOW_MS: u64 = 10 * 60_000;

/// The ids a dispatch names, wherever it puts them.
#[derive(Debug, Default, serde::Deserialize)]
struct Ids {
    #[serde(default, deserialize_with = "wire::opt_snowflake")]
    id: Option<u64>,
    #[serde(default, deserialize_with = "wire::opt_snowflake")]
    channel_id: Option<u64>,
    #[serde(default, deserialize_with = "wire::opt_snowflake")]
    guild_id: Option<u64>,
    #[serde(default, deserialize_with = "wire::opt_snowflake")]
    user_id: Option<u64>,
    user: Option<wire::User>,
}

/// What the client held before it processed the dispatch.
#[derive(Debug, Clone, Default)]
struct Before {
    message: Option<api::Message>,
    existed: bool,
    relationship: Option<api::RelationshipKind>,
    voice_channel: Option<u64>,
    typing: Vec<u64>,
    read_state: Option<api::ReadState>,
    /// The user's own status, for `SESSIONS_REPLACE`.
    status: String,
}

/// One dispatch on its way through the client.
#[derive(Debug, Clone)]
pub struct Dispatch {
    pub t: String,
    /// What the client processes. A filter that replaced a message's
    /// content has written it here.
    pub d: Value,
    before: Before,
}

impl Dispatch {
    /// Read from `client` what this dispatch's events need from before the
    /// change. Call it before the client processes the dispatch.
    pub fn new(t: &str, d: Value, client: &dyn ClientBackend) -> Self {
        let ids: Ids = parse(&d).unwrap_or_default();
        let user = ids.user_id.or(ids.user.as_ref().map(|u| u.id));
        let mut b = Before::default();
        match t {
            "MESSAGE_UPDATE" | "MESSAGE_DELETE" => {
                if let (Some(c), Some(id)) = (ids.channel_id, ids.id) {
                    b.message = client.message(c, id);
                }
            }
            "MESSAGE_CREATE" | "MESSAGE_ACK" | "TYPING_START" => {
                if let Some(c) = ids.channel_id {
                    b.typing = client.typing(c);
                    b.read_state = client.read_state(c);
                }
            }
            "GUILD_CREATE" | "GUILD_UPDATE" | "GUILD_DELETE" => {
                b.existed = ids.id.and_then(|g| client.guild(g)).is_some();
            }
            _ if t.starts_with("CHANNEL_") || t.starts_with("THREAD_") => {
                b.existed = ids.id.and_then(|c| client.channel(c)).is_some();
            }
            "GUILD_MEMBER_ADD" | "GUILD_MEMBER_UPDATE" => {
                b.existed = matches!((ids.guild_id, user), (Some(g), Some(u)) if client.member(g, u).is_some());
            }
            "SESSIONS_REPLACE" => b.status = client.my_status(),
            "VOICE_STATE_UPDATE" => {
                b.voice_channel = user.and_then(|u| client.voice_state(u)).map(|v| v.channel);
            }
            _ if t.starts_with("RELATIONSHIP_") => {
                b.relationship = ids
                    .id
                    .and_then(|u| client.relationships().into_iter().find(|r| r.user == u))
                    .map(|r| r.kind);
            }
            _ => {}
        }
        Self {
            t: t.to_string(),
            d,
            before: b,
        }
    }

    /// Run the dispatch past the plugins' `event-filter`s. `None`: a plugin
    /// dropped it, and the client must not process it.
    ///
    /// A filter sees the event the dispatch causes, built from the dispatch
    /// and the state before it. When no active plugin filters its kind,
    /// nothing is built. A replaced message's content is written into
    /// [`d`](Self::d); a replacement of another kind isn't carried into the
    /// dispatch, which passes as it was.
    pub fn filter(
        mut self,
        rt: &mut crate::runtime::Runtime,
        client: &dyn ClientBackend,
    ) -> Option<Self> {
        let Some(kind) = dispatch_kind(&self.t).filter(|k| rt.filters_kind(*k)) else {
            return Some(self);
        };
        let Some(ev) = self
            .preview(client)
            .filter(|e| crate::runtime::event_kind(e) == Some(kind))
        else {
            return Some(self);
        };
        match rt.intercept(ev.clone())? {
            Event::MessageCreated(m) | Event::MessageEdited((m, _)) if self.d.is_object() => {
                self.d["content"] = Value::String(m.content);
                Some(self)
            }
            _ => Some(self),
        }
    }

    /// The events this dispatch caused. Call it after the client processed
    /// [`d`](Self::d); the records come from the client, so a query made
    /// while handling one agrees with it.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per dispatch name, read as a table"
    )]
    pub fn events(&self, client: &dyn ClientBackend, now_ms: u64) -> Vec<Event> {
        let (t, d, b) = (self.t.as_str(), &self.d, &self.before);
        let mut out = Vec::new();
        match t {
            "READY" => out.push(Event::Connection(api::ConnectionState::Connected)),
            "RESUMED" => out.push(Event::Connection(api::ConnectionState::Resumed)),
            "READY_SUPPLEMENTAL"
            | "THREAD_LIST_SYNC"
            | "GUILD_MEMBERS_CHUNK"
            | "USER_UPDATE"
            | "USER_NOTE_UPDATE" => {}
            "GUILD_CREATE" => {
                if let Some(g) = parse::<wire::Guild>(d).filter(|g| !g.unavailable)
                    && let Some(guild) = client.guild(g.id)
                {
                    let loaded =
                        b.existed || guild.joined_ms.is_some_and(|j| j + JOIN_WINDOW_MS < now_ms);
                    out.push(if loaded {
                        Event::GuildUpdated(guild)
                    } else {
                        Event::GuildJoined(guild)
                    });
                }
            }
            "GUILD_UPDATE" => {
                if let Some(g) = parse::<wire::GuildProperties>(d)
                    .and_then(|p| p.id)
                    .and_then(|id| client.guild(id))
                {
                    out.push(Event::GuildUpdated(g));
                }
            }
            "GUILD_DELETE" => {
                if let Some(g) = parse::<wire::GuildDelete>(d).filter(|_| b.existed) {
                    out.push(guild_left(&g));
                }
            }
            "CHANNEL_CREATE" | "THREAD_CREATE" | "CHANNEL_UPDATE" | "THREAD_UPDATE" => {
                if let Some(c) = parse::<wire::Channel>(d).and_then(|c| client.channel(c.id)) {
                    out.push(if b.existed {
                        Event::ChannelUpdated(c)
                    } else {
                        Event::ChannelCreated(c)
                    });
                }
            }
            "CHANNEL_DELETE" | "THREAD_DELETE" => {
                if let Some(c) = parse::<wire::Channel>(d) {
                    out.push(Event::ChannelDeleted((c.id, c.guild_id)));
                }
            }
            "USER_GUILD_SETTINGS_UPDATE" => {
                if let Some(g) = parse::<wire::GuildSettings>(d)
                    .and_then(|s| s.guild_id)
                    .and_then(|g| client.guild(g))
                {
                    out.push(Event::GuildUpdated(g));
                }
            }
            "GUILD_ROLE_CREATE" | "GUILD_ROLE_UPDATE" => {
                if let Some(r) = parse::<wire::GuildRole>(d)
                    && let Some(role) = r.role.and_then(|role| client.role(r.guild_id, role.id))
                {
                    out.push(Event::RoleUpdated(role));
                }
            }
            "GUILD_MEMBER_ADD" | "GUILD_MEMBER_UPDATE" => {
                if let Some(m) = parse::<wire::Member>(d)
                    && let (Some(g), Some(u)) = (m.guild_id, m.user_id())
                    && let Some(member) = client.member(g, u)
                {
                    let updated = b.existed && t == "GUILD_MEMBER_UPDATE";
                    out.push(if updated {
                        Event::MemberUpdated(member)
                    } else {
                        Event::MemberJoined(member)
                    });
                }
            }
            "PRESENCE_UPDATE" => {
                if let Some(p) = parse::<wire::Presence>(d) {
                    let fresh = p.user_id().and_then(|u| client.presence(u, p.guild_id));
                    if let Some(p) = fresh.or_else(|| convert::presence(&p)) {
                        out.push(Event::Presence(p));
                    }
                }
            }
            "SESSIONS_REPLACE" => {
                let status = client.my_status();
                if status != b.status {
                    out.push(Event::MyPresence(status));
                }
            }
            "TYPING_START" => {
                if let Some(ty) = parse::<wire::Typing>(d) {
                    out.push(Event::Typing((ty.channel_id, client.typing(ty.channel_id))));
                }
            }
            "VOICE_STATE_UPDATE" => {
                if let Some(v) = parse::<wire::VoiceState>(d) {
                    let mine = v.user_id == client.me().map_or(0, |u| u.id);
                    match client
                        .voice_state(v.user_id)
                        .filter(|_| v.channel_id.is_some())
                    {
                        Some(state) => {
                            if let Some(mv) = client.my_voice().filter(|_| mine) {
                                out.push(Event::MyVoice(mv));
                            }
                            out.push(Event::VoiceState(state));
                        }
                        None => {
                            if let Some(channel) = b.voice_channel {
                                if mine {
                                    out.push(Event::MyVoiceLeft);
                                }
                                out.push(Event::VoiceLeft((v.user_id, channel)));
                            }
                        }
                    }
                }
            }
            "MESSAGE_CREATE"
            | "MESSAGE_UPDATE"
            | "MESSAGE_DELETE"
            | "MESSAGE_DELETE_BULK"
            | "MESSAGE_ACK"
            | "CHANNEL_PINS_UPDATE"
            | "MESSAGE_POLL_VOTE_ADD"
            | "MESSAGE_POLL_VOTE_REMOVE" => {
                if let Some(ev) = self.preview_with(client, true) {
                    out.push(ev);
                }
                if let Some(c) = parse::<Ids>(d)
                    .and_then(|i| i.channel_id)
                    .filter(|_| matches!(t, "MESSAGE_CREATE" | "MESSAGE_ACK"))
                {
                    let typing = client.typing(c);
                    if typing != b.typing {
                        out.push(Event::Typing((c, typing)));
                    }
                    if let Some(rs) = client
                        .read_state(c)
                        .filter(|rs| Some(rs) != b.read_state.as_ref())
                    {
                        out.push(Event::ReadState(rs));
                    }
                }
            }
            _ if t.starts_with("MESSAGE_REACTION_") => out.extend(self.preview(client)),
            "GUILD_ROLE_DELETE"
            | "GUILD_MEMBER_REMOVE"
            | "GUILD_EMOJIS_UPDATE"
            | "GUILD_STICKERS_UPDATE" => out.extend(self.preview(client)),
            _ if t.starts_with("RELATIONSHIP_") => {
                if let Some(r) = parse::<wire::Relationship>(d) {
                    let new = client
                        .relationships()
                        .into_iter()
                        .find(|x| x.user == r.id)
                        .map(|x| x.kind);
                    if new != b.relationship {
                        out.push(Event::Relationship(relationship_change(
                            r.id,
                            b.relationship,
                            new,
                        )));
                    }
                }
            }
            name if never_forwarded(name) => {}
            name => {
                let mut v = d.clone();
                crate::runtime::redact(&mut v);
                out.push(Event::Raw((name.to_string(), v.to_string())));
            }
        }
        out
    }

    /// The event a filter sees: built from the dispatch and the state
    /// before it, since the client hasn't processed it yet.
    fn preview(&self, client: &dyn ClientBackend) -> Option<Event> {
        self.preview_with(client, false)
    }

    /// `after`: the client has processed the dispatch, so a message is read
    /// back from it.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per dispatch name, read as a table"
    )]
    fn preview_with(&self, client: &dyn ClientBackend, after: bool) -> Option<Event> {
        let (t, d, b) = (self.t.as_str(), &self.d, &self.before);
        let names = |g: Option<u64>, u: u64| display_name(client, g, u);
        let author_of = |c: u64, id: u64| client.message(c, id).map(|m| m.author.id);
        let mref = |channel: u64, id: u64, guild: Option<u64>, hint: Option<u64>| api::MessageRef {
            id,
            channel,
            guild,
            author: author_of(channel, id).or(hint).unwrap_or(0),
        };
        Some(match t {
            "MESSAGE_CREATE" => {
                let m = parse::<wire::Message>(d)?;
                let fresh = if after {
                    client.message(m.channel_id, m.id)
                } else {
                    None
                };
                let guild = m
                    .guild_id
                    .or_else(|| client.channel(m.channel_id).and_then(|c| c.guild));
                Event::MessageCreated(
                    fresh.unwrap_or_else(|| convert::message(&m, guild, None, &names, &author_of)),
                )
            }
            "MESSAGE_UPDATE" => {
                let m = parse::<wire::Message>(d)?;
                let guild = m
                    .guild_id
                    .or_else(|| client.channel(m.channel_id).and_then(|c| c.guild));
                let merged = convert::message(&m, guild, b.message.as_ref(), &names, &author_of);
                // An unfurl that only adds embeds isn't an edit.
                let edit = (m.content.is_some() || m.edited_timestamp.is_some())
                    && b.message.as_ref().is_none_or(|o| {
                        o.content != merged.content || o.edited_ms != merged.edited_ms
                    });
                if !edit {
                    return None;
                }
                let fresh = if after {
                    client.message(m.channel_id, m.id)
                } else {
                    None
                };
                Event::MessageEdited((
                    fresh.unwrap_or(merged),
                    b.message.as_ref().map(|o| o.content.clone()),
                ))
            }
            "MESSAGE_DELETE" => {
                let m = parse::<wire::MessageDelete>(d)?;
                Event::MessageDeleted(api::DeletedMessage {
                    channel: m.channel_id,
                    id: m.id,
                    guild: m.guild_id,
                    cached: b.message.clone(),
                })
            }
            "MESSAGE_DELETE_BULK" => {
                let m = parse::<wire::MessageDeleteBulk>(d)?;
                Event::MessagesBulkDeleted((m.channel_id, m.ids))
            }
            "MESSAGE_ACK" => {
                let a = parse::<wire::MessageAck>(d)?;
                if after {
                    return None;
                }
                Event::ReadState(api::ReadState {
                    channel: a.channel_id,
                    unread: false,
                    mentions: a.mention_count.unwrap_or(0),
                    last_read: a.message_id,
                    manual: a.manual,
                })
            }
            "CHANNEL_PINS_UPDATE" => Event::Pins(parse::<wire::PinsUpdate>(d)?.channel_id),
            "MESSAGE_REACTION_ADD" | "MESSAGE_REACTION_REMOVE" => {
                let r = parse::<wire::ReactionEvent>(d)?;
                Event::Reaction(api::ReactionChange {
                    message: mref(r.channel_id, r.message_id, r.guild_id, r.message_author_id),
                    user: r.user_id?,
                    emoji: convert::emoji_text(r.emoji.as_ref()?),
                    burst: r.burst || r.kind == 1,
                    added: t == "MESSAGE_REACTION_ADD",
                })
            }
            "MESSAGE_REACTION_REMOVE_ALL" | "MESSAGE_REACTION_REMOVE_EMOJI" => {
                let r = parse::<wire::ReactionEvent>(d)?;
                let emoji = r.emoji.as_ref().map(convert::emoji_text);
                Event::ReactionsCleared((
                    mref(r.channel_id, r.message_id, r.guild_id, r.message_author_id),
                    emoji,
                ))
            }
            "MESSAGE_POLL_VOTE_ADD" | "MESSAGE_POLL_VOTE_REMOVE" => {
                let v = parse::<wire::PollVote>(d)?;
                Event::PollVote((
                    mref(v.channel_id, v.message_id, v.guild_id, None),
                    v.user_id,
                    v.answer_id,
                    t == "MESSAGE_POLL_VOTE_ADD",
                ))
            }
            "TYPING_START" => {
                let ty = parse::<wire::Typing>(d)?;
                let mut users = b.typing.clone();
                if !users.contains(&ty.user_id) {
                    users.push(ty.user_id);
                }
                Event::Typing((ty.channel_id, users))
            }
            "PRESENCE_UPDATE" => Event::Presence(convert::presence(&parse(d)?)?),
            "VOICE_STATE_UPDATE" => {
                let v = parse::<wire::VoiceState>(d)?;
                match convert::voice_state(&v) {
                    Some(state) => Event::VoiceState(state),
                    None => Event::VoiceLeft((v.user_id, b.voice_channel?)),
                }
            }
            "GUILD_MEMBER_ADD" | "GUILD_MEMBER_UPDATE" => {
                let m = parse::<wire::Member>(d)?;
                let member = convert::member(m.guild_id?, &m)?;
                if b.existed && t == "GUILD_MEMBER_UPDATE" {
                    Event::MemberUpdated(member)
                } else {
                    Event::MemberJoined(member)
                }
            }
            "GUILD_MEMBER_REMOVE" => {
                let m = parse::<wire::MemberRemove>(d)?;
                Event::MemberLeft((m.guild_id, m.user.id))
            }
            "GUILD_ROLE_CREATE" | "GUILD_ROLE_UPDATE" => {
                let r = parse::<wire::GuildRole>(d)?;
                Event::RoleUpdated(convert::role(r.role?, r.guild_id))
            }
            "GUILD_ROLE_DELETE" => {
                let r = parse::<wire::GuildRole>(d)?;
                Event::RoleDeleted((r.guild_id, r.role_id?))
            }
            "GUILD_EMOJIS_UPDATE" => Event::EmojiUpdated(parse::<wire::GuildEmojis>(d)?.guild_id),
            "GUILD_STICKERS_UPDATE" => {
                Event::EmojiUpdated(parse::<wire::GuildStickers>(d)?.guild_id)
            }
            "GUILD_DELETE" => guild_left(&parse::<wire::GuildDelete>(d)?),
            "CHANNEL_DELETE" | "THREAD_DELETE" => {
                let c = parse::<wire::Channel>(d)?;
                Event::ChannelDeleted((c.id, c.guild_id))
            }
            _ if t.starts_with("RELATIONSHIP_") => {
                let r = parse::<wire::Relationship>(d)?;
                let new = if t == "RELATIONSHIP_REMOVE" {
                    None
                } else {
                    convert::relationship(&r).map(|x| x.kind)
                };
                Event::Relationship(relationship_change(r.id, b.relationship, new))
            }
            // Guild, channel and role creates and updates: a filter sees
            // the client's record from before the change.
            "GUILD_CREATE" | "GUILD_UPDATE" => {
                let id = parse::<Ids>(d)?.id?;
                Event::GuildUpdated(client.guild(id)?)
            }
            _ if t.starts_with("CHANNEL_") || t.starts_with("THREAD_") => {
                let id = parse::<Ids>(d)?.id?;
                Event::ChannelUpdated(client.channel(id)?)
            }
            _ => return None,
        })
    }
}

/// `guild-left` for a `GUILD_DELETE`: an outage if the guild is only
/// unavailable, otherwise the user left or was removed.
fn guild_left(g: &wire::GuildDelete) -> Event {
    let reason = if g.unavailable {
        api::GuildLeftReason::Unavailable
    } else {
        api::GuildLeftReason::Left
    };
    Event::GuildLeft((g.id, reason))
}

fn relationship_change(
    user: u64,
    old: Option<api::RelationshipKind>,
    new: Option<api::RelationshipKind>,
) -> api::RelationshipChange {
    use api::RelationshipKind as K;
    // Removals don't say who removed it; additions the user made do.
    let by_you = new == Some(K::Outgoing)
        || new == Some(K::Blocked)
        || (new == Some(K::Friend) && old == Some(K::Incoming));
    api::RelationshipChange {
        user,
        old,
        new,
        by_you,
    }
}

/// Nickname in this guild, else global name, else username.
fn display_name(client: &dyn ClientBackend, guild: Option<u64>, user: u64) -> String {
    let nick = guild
        .and_then(|g| client.member(g, user))
        .and_then(|m| m.nick);
    let u = client.user(user);
    nick.or(u.as_ref().and_then(|u| u.global_name.clone()))
        .or(u.map(|u| u.username))
        .unwrap_or_default()
}

/// The interceptable event kind a dispatch causes, if any.
fn dispatch_kind(t: &str) -> Option<api::EventKind> {
    use api::EventKind as K;
    Some(match t {
        "MESSAGE_CREATE" | "MESSAGE_UPDATE" | "MESSAGE_DELETE" | "MESSAGE_DELETE_BULK" => {
            K::Message
        }
        _ if t.starts_with("MESSAGE_REACTION_") => K::Reaction,
        "MESSAGE_POLL_VOTE_ADD" | "MESSAGE_POLL_VOTE_REMOVE" => K::PollVote,
        "CHANNEL_PINS_UPDATE" => K::Pins,
        "TYPING_START" => K::Typing,
        "PRESENCE_UPDATE" => K::Presence,
        "VOICE_STATE_UPDATE" => K::VoiceState,
        "GUILD_MEMBER_ADD" | "GUILD_MEMBER_UPDATE" | "GUILD_MEMBER_REMOVE" => K::Member,
        _ if t.starts_with("RELATIONSHIP_") => K::Relationship,
        "GUILD_CREATE" | "GUILD_UPDATE" | "GUILD_DELETE" => K::Guild,
        _ if t.starts_with("CHANNEL_") || t.starts_with("THREAD_") => K::Channel,
        _ if t.starts_with("GUILD_ROLE_") => K::Role,
        _ => return None,
    })
}

/// One `presence` per user and one `typing` per channel within a host
/// tick's events: the latest, kept where the first one was.
pub fn coalesce(events: &[Event]) -> Vec<Event> {
    let mut latest_presence = BTreeMap::new();
    let mut latest_typing = BTreeMap::new();
    for (i, e) in events.iter().enumerate() {
        match e {
            Event::Presence(p) => {
                latest_presence.insert(p.user, i);
            }
            Event::Typing((c, _)) => {
                latest_typing.insert(*c, i);
            }
            _ => {}
        }
    }
    let (mut seen_presence, mut seen_typing) = (BTreeSet::new(), BTreeSet::new());
    let mut out = Vec::with_capacity(events.len());
    for e in events {
        match e {
            Event::Presence(p) => {
                if seen_presence.insert(p.user) {
                    out.push(events[latest_presence[&p.user]].clone());
                }
            }
            Event::Typing((c, _)) => {
                if seen_typing.insert(*c) {
                    out.push(events[latest_typing[c]].clone());
                }
            }
            e => out.push(e.clone()),
        }
    }
    out
}

#[cfg(test)]
mod tests;
