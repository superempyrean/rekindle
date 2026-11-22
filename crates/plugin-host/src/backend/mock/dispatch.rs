//! The mock client's handling of gateway dispatches.
//!
//! This is the simulated client, not something rekindle keeps beside a
//! real one: [`MockBackend::process_dispatch`](super::MockBackend::process_dispatch)
//! applies a dispatch to the mock's [`World`] the way Discord's client
//! applies it to its stores, so the gateway translator
//! ([`crate::backend::gateway`]) can be tested against a client that
//! changes the way a real one does. Against Phoenix, the client's own
//! stores do this.

use std::collections::{BTreeMap, BTreeSet};

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::world::{self, VIEW_CHANNEL, World, is_thread};
use crate::api;
use crate::backend::gateway::{convert, settings, wire};

/// How many messages a channel keeps.
const MESSAGES_PER_CHANNEL: usize = 500;

/// How long a typing indicator lasts without a new `TYPING_START`.
const TYPING_MS: u64 = 10_000;

/// What the simulated client keeps beside its world.
#[derive(Debug, Clone, Default)]
pub(super) struct Stores {
    /// Muted guilds (key 0: DMs) and channels.
    muted_guilds: BTreeSet<u64>,
    muted_channels: BTreeSet<u64>,
    /// (channel, user) -> typing until, epoch ms.
    typing_until: BTreeMap<(u64, u64), u64>,
    /// channel -> (last read message, mentions, marked unread by hand).
    last_read: BTreeMap<u64, (Option<u64>, u32, bool)>,
}

/// The world and the stores, borrowed together while a dispatch applies.
pub(super) struct Sim<'a> {
    world: &'a mut World,
    stores: &'a mut Stores,
}

impl<'a> Sim<'a> {
    pub(super) fn new(world: &'a mut World, stores: &'a mut Stores) -> Self {
        Self { world, stores }
    }

    /// End the typing indicators that have run out by `now_ms`.
    pub(super) fn tick(&mut self, now_ms: u64) {
        let expired: Vec<(u64, u64)> = self
            .stores
            .typing_until
            .iter()
            .filter(|(_, until)| **until <= now_ms)
            .map(|(k, _)| *k)
            .collect();
        for (channel, user) in expired {
            self.stop_typing(channel, user);
        }
    }

    /// Apply one dispatch. One that doesn't decode changes nothing.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per dispatch name, read as a table"
    )]
    pub(super) fn apply(&mut self, t: &str, d: &Value, now: u64) {
        match t {
            "READY" => self.on(d, Self::ready),
            "READY_SUPPLEMENTAL" => self.on(d, |s, r| s.ready_supplemental(&r)),
            "GUILD_CREATE" => self.on(d, |s, g: wire::Guild| {
                // A "partial" guild is the client's cached copy, unchanged.
                if !g.unavailable && g.data_mode.as_deref() != Some("partial") {
                    s.load_guild(g, &[]);
                }
            }),
            "GUILD_UPDATE" => self.on(d, Self::guild_update),
            "GUILD_DELETE" => self.on(d, |s, g: wire::GuildDelete| s.remove_guild(g.id)),
            "CHANNEL_CREATE" | "THREAD_CREATE" | "CHANNEL_UPDATE" | "THREAD_UPDATE" => {
                self.on(d, |s, c: wire::Channel| {
                    s.put_channel(&c, c.guild_id);
                    if let Some(g) = c.guild_id {
                        s.recompute(g);
                    }
                });
            }
            "CHANNEL_DELETE" | "THREAD_DELETE" => {
                self.on(d, |s, c: wire::Channel| s.world.remove_channel(c.id));
            }
            "THREAD_LIST_SYNC" => self.on(d, |s, sync: wire::ThreadListSync| {
                for t in &sync.threads {
                    s.put_channel(t, Some(sync.guild_id));
                }
                s.recompute(sync.guild_id);
            }),
            "GUILD_ROLE_CREATE" | "GUILD_ROLE_UPDATE" => self.on(d, Self::role_update),
            "GUILD_ROLE_DELETE" => self.on(d, |s, r| s.role_delete(&r)),
            "GUILD_EMOJIS_UPDATE" => self.on(d, |s, e: wire::GuildEmojis| {
                let guild = e.guild_id;
                s.world.emoji.retain(|x| x.guild != Some(guild));
                s.world.emoji.extend(
                    e.emojis
                        .into_iter()
                        .filter_map(|x| convert::emoji(x, Some(guild))),
                );
            }),
            "GUILD_STICKERS_UPDATE" => self.on(d, |s, e: wire::GuildStickers| {
                let guild = e.guild_id;
                s.world.stickers.retain(|x| x.guild != Some(guild));
                s.world
                    .stickers
                    .extend(e.stickers.into_iter().map(|x| convert::sticker(x, guild)));
            }),
            "GUILD_MEMBER_ADD" | "GUILD_MEMBER_UPDATE" => self.on(d, |s, m: wire::Member| {
                if let (Some(guild), Some(user)) = (m.guild_id, m.user_id()) {
                    s.put_member(guild, &m);
                    if user == s.world.me {
                        s.recompute(guild);
                    }
                }
            }),
            "GUILD_MEMBER_REMOVE" => self.on(d, |s, m: wire::MemberRemove| {
                s.world
                    .members
                    .retain(|x| !(x.guild == m.guild_id && x.user == m.user.id));
            }),
            "GUILD_MEMBERS_CHUNK" => self.on(d, |s, c: wire::MembersChunk| {
                for m in &c.members {
                    s.put_member(c.guild_id, m);
                }
                for p in &c.presences {
                    s.put_presence(p);
                }
            }),
            "PRESENCE_UPDATE" => self.on(d, |s, p: wire::Presence| {
                if let Some(p) = s.put_presence(&p)
                    && p.user == s.world.me
                {
                    s.world.status = p.status;
                }
            }),
            "SESSIONS_REPLACE" => self.on(d, |s, sessions: Vec<wire::Session>| {
                if let Some(active) = sessions.iter().find(|x| x.active).or(sessions.first())
                    && !active.status.is_empty()
                {
                    s.world.status.clone_from(&active.status);
                }
            }),
            "USER_UPDATE" => self.on(d, |s, u: wire::User| s.put_user(&u)),
            "USER_NOTE_UPDATE" => self.on(d, |s, n: wire::NoteUpdate| {
                if n.note.is_empty() {
                    s.world.notes.remove(&n.id);
                } else {
                    s.world.notes.insert(n.id, n.note);
                }
            }),
            "USER_SETTINGS_PROTO_UPDATE" => self.on(d, |s, u: wire::SettingsProtoUpdate| {
                if u.settings.kind == 1 {
                    s.put_settings(&u.settings.proto);
                }
            }),
            "USER_GUILD_SETTINGS_UPDATE" => self.on(d, |s, g: wire::GuildSettings| {
                s.put_guild_settings(&g);
                s.apply_mutes();
            }),
            "TYPING_START" => self.on(d, |s, ty| s.typing_start(&ty, now)),
            "VOICE_STATE_UPDATE" => self.on(d, |s, v| s.voice_state(&v, now)),
            "MESSAGE_CREATE" => self.on(d, |s, m| s.message_create(&m)),
            "MESSAGE_UPDATE" => self.on(d, |s, m| s.message_update(&m)),
            "MESSAGE_DELETE" => self.on(d, |s, m: wire::MessageDelete| {
                s.world.remove_message(m.channel_id, m.id);
            }),
            "MESSAGE_DELETE_BULK" => self.on(d, |s, m: wire::MessageDeleteBulk| {
                for id in m.ids {
                    s.world.remove_message(m.channel_id, id);
                }
            }),
            "MESSAGE_REACTION_ADD" => self.on(d, |s, r| s.reaction(&r, true)),
            "MESSAGE_REACTION_REMOVE" => self.on(d, |s, r| s.reaction(&r, false)),
            "MESSAGE_REACTION_REMOVE_ALL" | "MESSAGE_REACTION_REMOVE_EMOJI" => {
                self.on(d, |s, r: wire::ReactionEvent| {
                    let emoji = r.emoji.as_ref().map(convert::emoji_text);
                    if let Some(m) = s.world.find_message_mut(r.channel_id, r.message_id) {
                        match &emoji {
                            Some(e) => m.reactions.retain(|x| &x.emoji != e),
                            None => m.reactions.clear(),
                        }
                    }
                });
            }
            "MESSAGE_POLL_VOTE_ADD" => self.on(d, |s, v| s.poll_vote(&v, true)),
            "MESSAGE_POLL_VOTE_REMOVE" => self.on(d, |s, v| s.poll_vote(&v, false)),
            "MESSAGE_ACK" => self.on(d, |s, a: wire::MessageAck| {
                s.stores.last_read.insert(
                    a.channel_id,
                    (a.message_id, a.mention_count.unwrap_or(0), a.manual),
                );
                s.refresh_read_state(a.channel_id);
            }),
            "RELATIONSHIP_ADD" | "RELATIONSHIP_UPDATE" => {
                self.on(d, |s, r: wire::Relationship| s.put_relationship(&r));
            }
            "RELATIONSHIP_REMOVE" => self.on(d, |s, r: wire::Relationship| {
                s.world.relationships.retain(|x| x.user != r.id);
            }),
            _ => {}
        }
    }

    /// Decode `d` as `T` and hand it to `f`.
    fn on<T: DeserializeOwned>(&mut self, d: &Value, f: impl FnOnce(&mut Self, T)) {
        if let Some(v) = wire::parse(d) {
            f(self, v);
        }
    }

    // ---------------- READY ----------------

    fn ready(&mut self, r: wire::Ready) {
        *self.world = World::empty();
        self.world.me = r.user.id;
        *self.stores = Stores::default();

        self.put_user(&r.user);
        for u in &r.users {
            self.put_user(u);
        }
        for s in r
            .user_guild_settings
            .map(wire::Versioned::into_vec)
            .unwrap_or_default()
        {
            self.put_guild_settings(&s);
        }
        // Read state type 0 is a channel's; the others aren't channels.
        for e in r
            .read_state
            .map(wire::Versioned::into_vec)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.read_state_type == 0)
        {
            self.stores
                .last_read
                .insert(e.id, (e.last_message_id, e.mention_count, e.flags & 1 != 0));
        }
        for (i, g) in r.guilds.into_iter().enumerate() {
            if !g.unavailable {
                let members = r.merged_members.get(i).map_or(&[][..], Vec::as_slice);
                self.load_guild(g, members);
            }
        }
        for c in &r.private_channels {
            self.put_channel(c, None);
        }
        for rel in &r.relationships {
            self.put_relationship(rel);
        }
        for (id, note) in r.notes {
            if let Ok(id) = id.parse() {
                self.world.notes.insert(id, note);
            }
        }
        if let Some(proto) = &r.user_settings_proto {
            self.put_settings(proto);
        }
        self.apply_mutes();
        let channels: Vec<u64> = self.world.channels.iter().map(|c| c.id).collect();
        for c in channels {
            self.refresh_read_state(c);
        }
    }

    fn ready_supplemental(&mut self, r: &wire::ReadySupplemental) {
        for p in &r.merged_presences.friends {
            self.put_presence(p);
        }
        // `merged_presences.guilds` and `merged_members` are indexed like
        // `guilds`.
        for (i, g) in r.guilds.iter().enumerate() {
            for p in r.merged_presences.guilds.get(i).into_iter().flatten() {
                self.put_presence(p);
            }
            for m in r.merged_members.get(i).into_iter().flatten() {
                self.put_member(g.id, m);
            }
            for v in &g.voice_states {
                self.put_voice_state(v, Some(g.id));
            }
        }
    }

    // ---------------- guilds ----------------

    fn load_guild(&mut self, g: wire::Guild, merged_members: &[wire::Member]) {
        let id = g.id;
        self.remove_guild(id);
        let props = g.properties.unwrap_or_default();
        self.world.guilds.push(api::Guild {
            id,
            name: props.name,
            owner: props.owner_id.unwrap_or(0),
            description: props.description,
            features: props.features,
            premium_tier: props.premium_tier,
            premium_subscribers: g.premium_subscription_count.unwrap_or(0),
            verification_level: props.verification_level,
            nsfw_level: props.nsfw_level,
            preferred_locale: props.preferred_locale.unwrap_or_else(|| "en-US".into()),
            member_count: g.member_count,
            joined_ms: g.joined_at.as_deref().and_then(convert::iso_ms),
            muted: false,
        });
        self.world
            .roles
            .extend(g.roles.into_iter().map(|r| convert::role(r, id)));
        self.world.emoji.extend(
            g.emojis
                .into_iter()
                .filter_map(|e| convert::emoji(e, Some(id))),
        );
        self.world
            .stickers
            .extend(g.stickers.into_iter().map(|s| convert::sticker(s, id)));
        for c in g.channels.iter().chain(&g.threads) {
            self.put_channel(c, Some(id));
        }
        for m in g.members.iter().chain(merged_members) {
            self.put_member(id, m);
        }
        for p in &g.presences {
            self.put_presence(p);
        }
        for v in &g.voice_states {
            self.put_voice_state(v, Some(id));
        }
        self.sort_guilds();
        self.apply_mutes();
        self.recompute(id);
    }

    fn guild_update(&mut self, p: wire::GuildProperties) {
        let Some(id) = p.id else {
            return;
        };
        if let Some(g) = self.world.guilds.iter_mut().find(|g| g.id == id) {
            g.name = p.name;
            if let Some(o) = p.owner_id {
                g.owner = o;
            }
            g.description = p.description;
            g.features = p.features;
            g.premium_tier = p.premium_tier;
            g.verification_level = p.verification_level;
            g.nsfw_level = p.nsfw_level;
            if let Some(l) = p.preferred_locale {
                g.preferred_locale = l;
            }
        }
        self.recompute(id);
    }

    /// Drop a guild and everything in it.
    fn remove_guild(&mut self, id: u64) {
        let w = &mut *self.world;
        let channels: Vec<u64> = w
            .channels
            .iter()
            .filter(|c| c.guild == Some(id))
            .map(|c| c.id)
            .collect();
        for c in channels {
            w.remove_channel(c);
        }
        w.guilds.retain(|g| g.id != id);
        w.roles.retain(|r| r.guild != id);
        w.emoji.retain(|e| e.guild != Some(id));
        w.stickers.retain(|s| s.guild != Some(id));
        w.members.retain(|m| m.guild != id);
        w.voice.retain(|v| v.guild != Some(id));
        w.permissions.remove(&id);
    }

    fn role_update(&mut self, r: wire::GuildRole) {
        let Some(role) = r.role else {
            return;
        };
        let role = convert::role(role, r.guild_id);
        self.world
            .roles
            .retain(|x| !(x.guild == role.guild && x.id == role.id));
        self.world.roles.push(role);
        self.recompute(r.guild_id);
    }

    fn role_delete(&mut self, r: &wire::GuildRole) {
        let Some(id) = r.role_id else {
            return;
        };
        let guild = r.guild_id;
        self.world
            .roles
            .retain(|x| !(x.guild == guild && x.id == id));
        for m in self.world.members.iter_mut().filter(|m| m.guild == guild) {
            m.roles.retain(|x| *x != id);
        }
        self.recompute(guild);
    }

    // ---------------- channels ----------------

    fn put_channel(&mut self, c: &wire::Channel, guild: Option<u64>) {
        let guild = c.guild_id.or(guild);
        for u in &c.recipients {
            self.put_user(u);
        }
        let recipients: Vec<api::UserRef> = c
            .recipient_ids
            .iter()
            .chain(c.recipients.iter().map(|u| &u.id))
            .map(|id| api::UserRef {
                id: *id,
                name: self.display_name(guild, *id),
            })
            .collect();
        let old = self.world.channel(c.id);
        // A DM without a name is called after its recipients.
        let name = c
            .name
            .clone()
            .or_else(|| old.map(|o| o.name.clone()))
            .unwrap_or_else(|| {
                recipients
                    .iter()
                    .map(|r| r.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            });
        let thread = c.thread_metadata.as_ref().map(|t| api::ThreadMeta {
            owner: c.owner_id.unwrap_or(0),
            archived: t.archived,
            locked: t.locked,
            message_count: c.message_count,
            member_count: c.member_count,
            applied_tags: c.applied_tags.clone(),
            joined: c.member.is_some()
                || old
                    .and_then(|o| o.thread.as_ref())
                    .is_some_and(|t| t.joined),
        });
        let channel = api::Channel {
            id: c.id,
            kind: c.kind,
            guild,
            parent: c.parent_id,
            name,
            topic: c.topic.clone(),
            position: c.position,
            nsfw: c.nsfw,
            slowmode: c.rate_limit_per_user,
            last_message: c.last_message_id.or(old.and_then(|o| o.last_message)),
            recipients,
            thread,
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
            muted: self.stores.muted_channels.contains(&c.id),
            visible: old.is_none_or(|o| o.visible),
        };
        // Threads have no overwrites of their own.
        if guild.is_some() && !is_thread(c.kind) {
            let overwrites = c
                .permission_overwrites
                .iter()
                .map(|o| api::Overwrite {
                    id: o.id,
                    member: o.kind == 1,
                    allow: o.allow,
                    deny: o.deny,
                })
                .collect();
            self.world.overwrites.insert(c.id, overwrites);
        }
        self.world.channels.retain(|x| x.id != c.id);
        self.world.channels.push(channel);
    }

    // ---------------- people ----------------

    /// Record a user. A partial user (no `username`) keeps what the client
    /// knew of them.
    fn put_user(&mut self, u: &wire::User) {
        let old = self.world.users.get(&u.id);
        let full = u.username.is_some();
        let user = api::User {
            id: u.id,
            username: u
                .username
                .clone()
                .or(old.map(|o| o.username.clone()))
                .unwrap_or_default(),
            global_name: if full {
                u.global_name.clone()
            } else {
                old.and_then(|o| o.global_name.clone())
            },
            bot: u.bot || old.is_some_and(|o| o.bot),
            system: u.system,
            public_flags: if full {
                u.public_flags
            } else {
                old.map_or(0, |o| o.public_flags)
            },
            accent: u.accent_color.map(convert::rgb),
            has_decoration: u
                .avatar_decoration_data
                .as_ref()
                .is_some_and(|v| !v.is_null()),
        };
        self.world.users.insert(u.id, user);
    }

    fn put_member(&mut self, guild: u64, m: &wire::Member) {
        if let Some(u) = &m.user {
            self.put_user(u);
        }
        let Some(member) = convert::member(guild, m) else {
            return;
        };
        self.world
            .members
            .retain(|x| !(x.guild == guild && x.user == member.user));
        self.world.members.push(member);
    }

    /// Record a presence. An offline user with no activities has none.
    fn put_presence(&mut self, p: &wire::Presence) -> Option<api::Presence> {
        let presence = convert::presence(p)?;
        if let Some(u) = p.user.as_ref().filter(|u| u.username.is_some()) {
            self.put_user(u);
        }
        if presence.status == "offline" && presence.activities.is_empty() {
            self.world.presences.remove(&presence.user);
        } else {
            self.world.presences.insert(presence.user, presence.clone());
        }
        Some(presence)
    }

    fn put_relationship(&mut self, r: &wire::Relationship) {
        if let Some(u) = &r.user {
            self.put_user(u);
        }
        if let Some(rel) = convert::relationship(r) {
            self.world.relationships.retain(|x| x.user != r.id);
            self.world.relationships.push(rel);
        }
    }

    /// Nickname here, else global name, else username.
    fn display_name(&self, guild: Option<u64>, user: u64) -> String {
        let nick = guild
            .and_then(|g| self.world.member(g, user))
            .and_then(|m| m.nick.clone());
        let u = self.world.users.get(&user);
        nick.or(u.and_then(|u| u.global_name.clone()))
            .or(u.map(|u| u.username.clone()))
            .unwrap_or_default()
    }

    fn typing_start(&mut self, ty: &wire::Typing, now: u64) {
        if let (Some(m), Some(g)) = (&ty.member, ty.guild_id) {
            self.put_member(g, m);
        }
        let until = ty.timestamp.saturating_mul(1000).max(now) + TYPING_MS;
        self.stores
            .typing_until
            .insert((ty.channel_id, ty.user_id), until);
        let list = self.world.typing.entry(ty.channel_id).or_default();
        if !list.contains(&ty.user_id) {
            list.push(ty.user_id);
        }
    }

    fn stop_typing(&mut self, channel: u64, user: u64) {
        self.stores.typing_until.remove(&(channel, user));
        let Some(list) = self.world.typing.get_mut(&channel) else {
            return;
        };
        list.retain(|u| *u != user);
        if list.is_empty() {
            self.world.typing.remove(&channel);
        }
    }

    // ---------------- voice ----------------

    /// Record where a user is in voice; `None` if they left. `guild` is
    /// the guild whose list the state came in, if any.
    fn put_voice_state(
        &mut self,
        v: &wire::VoiceState,
        guild: Option<u64>,
    ) -> Option<api::VoiceState> {
        self.world.voice.retain(|x| x.user != v.user_id);
        let mut state = convert::voice_state(v)?;
        if guild.is_some() {
            state.guild = guild;
        }
        self.world.voice.push(state);
        Some(state)
    }

    fn voice_state(&mut self, v: &wire::VoiceState, now: u64) {
        if let (Some(m), Some(g)) = (&v.member, v.guild_id) {
            self.put_member(g, m);
        }
        let before = self
            .world
            .voice
            .iter()
            .find(|x| x.user == v.user_id)
            .map(|x| x.channel);
        let mine = v.user_id == self.world.me;
        match self.put_voice_state(v, None) {
            Some(state) if mine => {
                // Moving within the same channel keeps the call's start.
                let since_ms = match (&self.world.my_voice, before) {
                    (Some(mv), Some(b)) if b == state.channel => mv.since_ms,
                    _ => now,
                };
                self.world.my_voice = Some(api::MyVoice {
                    channel: state.channel,
                    guild: state.guild,
                    since_ms,
                    self_mute: state.self_mute,
                    self_deaf: state.self_deaf,
                    self_video: state.self_video,
                    streaming: state.self_stream,
                    // The RTC connection isn't on the gateway.
                    connection: "connecting".into(),
                    ping_ms: None,
                });
            }
            None if before.is_some() => {
                if mine {
                    self.world.my_voice = None;
                }
                self.world.speaking.remove(&v.user_id);
            }
            _ => {}
        }
    }

    // ---------------- messages ----------------

    fn message_create(&mut self, m: &wire::Message) {
        let guild = m.guild_id.or_else(|| self.channel_guild(m.channel_id));
        if let (Some(member), Some(g), Some(a)) = (&m.member, guild, &m.author) {
            let mut member = member.clone();
            member.user_id = Some(a.id);
            self.put_member(g, &member);
        }
        if let Some(a) = &m.author {
            self.put_user(a);
        }
        for u in m.mentions.iter().flatten() {
            self.put_user(u);
        }
        let msg = convert::message(m, guild, None, &|g, u| self.display_name(g, u), &|c, i| {
            self.author_of(c, i)
        });
        let me = self.world.me;
        let author = msg.author.id;
        let mentions_me = msg.mentions.contains(&me)
            || msg.mentions_everyone
            || guild
                .and_then(|g| self.world.member(g, me))
                .is_some_and(|mine| msg.mention_roles.iter().any(|r| mine.roles.contains(r)));
        let channel = m.channel_id;
        if let Some(c) = self.world.channels.iter_mut().find(|c| c.id == channel) {
            c.last_message = Some(msg.id);
        }
        let list = self.world.messages.entry(channel).or_default();
        list.retain(|x| x.id != msg.id);
        list.push(msg.clone());
        if list.len() > MESSAGES_PER_CHANNEL {
            let excess = list.len() - MESSAGES_PER_CHANNEL;
            list.drain(..excess);
        }
        // The user's own message reads the channel; every DM counts as a
        // mention.
        if author == me {
            self.stores
                .last_read
                .insert(channel, (Some(msg.id), 0, false));
        } else if mentions_me || guild.is_none() {
            let e = self
                .stores
                .last_read
                .entry(channel)
                .or_insert((None, 0, false));
            e.1 += 1;
        }
        self.stop_typing(channel, author);
        self.refresh_read_state(channel);
    }

    fn message_update(&mut self, m: &wire::Message) {
        if let Some(a) = &m.author {
            self.put_user(a);
        }
        let guild = m.guild_id.or_else(|| self.channel_guild(m.channel_id));
        let old = self.world.find_message(m.channel_id, m.id).cloned();
        let msg = convert::message(
            m,
            guild,
            old.as_ref(),
            &|g, u| self.display_name(g, u),
            &|c, i| self.author_of(c, i),
        );
        if let Some(slot) = self.world.find_message_mut(m.channel_id, m.id) {
            *slot = msg;
        }
    }

    fn author_of(&self, channel: u64, id: u64) -> Option<u64> {
        self.world.find_message(channel, id).map(|m| m.author.id)
    }

    fn channel_guild(&self, channel: u64) -> Option<u64> {
        self.world.channel(channel).and_then(|c| c.guild)
    }

    fn reaction(&mut self, r: &wire::ReactionEvent, added: bool) {
        let (Some(user), Some(emoji)) = (r.user_id, &r.emoji) else {
            return;
        };
        let mine = user == self.world.me;
        if let Some(m) = self.world.find_message_mut(r.channel_id, r.message_id) {
            // `type` 1 marks a burst reaction on removes.
            let burst = r.burst || r.kind == 1;
            world::apply_reaction(
                &mut m.reactions,
                &convert::emoji_text(emoji),
                burst,
                mine,
                added,
            );
        }
    }

    fn poll_vote(&mut self, v: &wire::PollVote, added: bool) {
        let me = self.world.me;
        let Some(poll) = self
            .world
            .find_message_mut(v.channel_id, v.message_id)
            .and_then(|m| m.poll.as_mut())
        else {
            return;
        };
        if let Some((_, _, votes)) = poll.answers.iter_mut().find(|a| a.0 == v.answer_id) {
            *votes = if added {
                votes.saturating_add(1)
            } else {
                votes.saturating_sub(1)
            };
        }
        if v.user_id == me {
            poll.my_votes.retain(|x| *x != v.answer_id);
            if added {
                poll.my_votes.push(v.answer_id);
            }
        }
    }

    // ---------------- read states, mutes, permissions ----------------

    /// Recompute a channel's read state from its read position and last
    /// message.
    fn refresh_read_state(&mut self, channel: u64) {
        let last = self.world.channel(channel).and_then(|c| c.last_message);
        let entry = self.stores.last_read.get(&channel).copied();
        let (read, mentions, manual) = entry.unwrap_or((None, 0, false));
        let unread = match (last, read) {
            (Some(l), Some(r)) => l > r,
            // A channel the user never read is unread once it has a
            // message and the client tracks it.
            (Some(_), None) => entry.is_some(),
            (None, _) => false,
        };
        if unread || mentions > 0 || read.is_some() {
            self.world.read_states.insert(
                channel,
                api::ReadState {
                    channel,
                    unread,
                    mentions,
                    last_read: read,
                    manual,
                },
            );
        } else {
            self.world.read_states.remove(&channel);
        }
    }

    /// The settings protobuf: guild folders, which also order the guilds.
    /// Settings without folders (a partial update) leave them as they were.
    fn put_settings(&mut self, b64: &str) {
        let Some(folders) = settings::decode(b64).as_ref().and_then(settings::folders) else {
            return;
        };
        self.world.folders = folders;
        self.sort_guilds();
    }

    /// Guilds in sidebar order: the folders' order, then any guild they
    /// don't list.
    fn sort_guilds(&mut self) {
        let order: Vec<u64> = self
            .world
            .folders
            .iter()
            .flat_map(|f| f.guilds.iter().copied())
            .collect();
        self.world
            .guilds
            .sort_by_key(|g| order.iter().position(|x| *x == g.id).unwrap_or(usize::MAX));
    }

    fn put_guild_settings(&mut self, s: &wire::GuildSettings) {
        let set = |set: &mut BTreeSet<u64>, id: u64, muted: bool| {
            if muted {
                set.insert(id);
            } else {
                set.remove(&id);
            }
        };
        set(
            &mut self.stores.muted_guilds,
            s.guild_id.unwrap_or(0),
            s.muted,
        );
        for o in &s.channel_overrides {
            set(&mut self.stores.muted_channels, o.channel_id, o.muted);
        }
    }

    fn apply_mutes(&mut self) {
        for g in &mut self.world.guilds {
            g.muted = self.stores.muted_guilds.contains(&g.id);
        }
        for c in &mut self.world.channels {
            c.muted = self.stores.muted_channels.contains(&c.id);
        }
    }

    /// The user's permissions in a guild and each of its channels, and
    /// which channels they can see.
    fn recompute(&mut self, guild: u64) {
        let w = &*self.world;
        let me = w.me;
        let Some(g) = w.guilds.iter().find(|g| g.id == guild) else {
            return;
        };
        let roles = w.member(guild, me).map_or(&[][..], |m| m.roles.as_slice());
        let base = w.guild_permissions(g, me, roles);
        let mut perms = BTreeMap::from([(guild, base)]);
        let channels = w.channels.iter().filter(|c| c.guild == Some(guild));
        for c in channels.clone().filter(|c| !is_thread(c.kind)) {
            perms.insert(c.id, w.channel_permissions(guild, c.id, base, me, roles));
        }
        // A thread has its parent's permissions.
        for c in channels.filter(|c| is_thread(c.kind)) {
            let p = c
                .parent
                .and_then(|p| perms.get(&p).copied())
                .unwrap_or(base);
            perms.insert(c.id, p);
        }
        for c in self
            .world
            .channels
            .iter_mut()
            .filter(|c| c.guild == Some(guild))
        {
            c.visible = perms.get(&c.id).is_some_and(|p| p & VIEW_CHANNEL != 0);
        }
        self.world.permissions.extend(perms);
    }
}
