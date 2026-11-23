//! [`World`]: the client state the mock answers queries from, and what a
//! plugin's actions change in it.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::api::{
    self, Attachment, Channel, CustomStatus, Emoji, Guild, GuildFolder, Message, MessageRef,
    MyVoice, Overwrite, Presence, Reaction, ReadState, Relationship, Role, Selection, Sticker,
    User, VoiceState,
};
use crate::backend::fixtures;
use crate::model::{Action, ActionError, OutgoingFile};

/// Every permission bit Discord defines (through bit 53).
pub(crate) const ALL_PERMISSIONS: u64 = (1 << 54) - 1;
pub(crate) const ADMINISTRATOR: u64 = 1 << 3;
pub(crate) const VIEW_CHANNEL: u64 = 1 << 10;

/// Channel types the mock tells apart.
pub(super) const DM: u16 = 1;
pub(super) const GUILD_CATEGORY: u16 = 4;

/// Announcement, public and private threads (channel types 10 to 12).
pub(super) fn is_thread(kind: u16) -> bool {
    (10..=12).contains(&kind)
}

/// The client state the mock answers queries from. Fields are public so
/// tests and `rekindle dev` can shape the world; [`sample_world`] is the
/// default.
///
/// [`sample_world`]: crate::backend::sample_world
#[derive(Debug, Clone, Serialize)]
pub struct World {
    pub me: u64,
    pub status: String,
    pub custom_status: Option<CustomStatus>,
    pub selection: Selection,
    pub users: BTreeMap<u64, User>,
    /// Sidebar order.
    pub guilds: Vec<Guild>,
    pub folders: Vec<GuildFolder>,
    /// Every channel, DMs included.
    pub channels: Vec<Channel>,
    pub roles: Vec<Role>,
    pub emoji: Vec<Emoji>,
    pub frequent_emoji: Vec<Emoji>,
    pub stickers: Vec<Sticker>,
    pub members: Vec<api::Member>,
    pub presences: BTreeMap<u64, Presence>,
    pub voice: Vec<VoiceState>,
    pub my_voice: Option<MyVoice>,
    pub speaking: BTreeSet<u64>,
    /// channel -> users typing, oldest first.
    pub typing: BTreeMap<u64, Vec<u64>>,
    pub relationships: Vec<Relationship>,
    pub notes: BTreeMap<u64, String>,
    /// channel -> loaded messages, oldest first.
    pub messages: BTreeMap<u64, Vec<Message>>,
    /// Messages not loaded, but fetchable through a message link.
    pub remote_messages: Vec<Message>,
    /// On screen, top to bottom.
    pub visible: Vec<MessageRef>,
    pub read_states: BTreeMap<u64, ReadState>,
    pub overwrites: BTreeMap<u64, Vec<Overwrite>>,
    /// The user's (curated) settings.
    pub settings: BTreeMap<String, String>,
    /// The user's permission bits per channel or guild id. A channel
    /// without an entry has its guild's, and a guild without one has all.
    pub permissions: BTreeMap<u64, u64>,
    /// What's in the composer (`insert-text`).
    pub composer: String,
    /// The reply the composer is set up for: (message, mention).
    pub pending_reply: Option<(MessageRef, bool)>,
    /// The message open for editing.
    pub editing: Option<MessageRef>,
}

impl World {
    /// An empty client (no guilds, no messages), logged in as user 1.
    pub fn empty() -> Self {
        Self {
            me: 1,
            status: "online".into(),
            custom_status: None,
            selection: Selection {
                guild: None,
                channel: None,
                voice_channel: None,
                side_thread: None,
                focused: true,
                idle: false,
            },
            users: BTreeMap::new(),
            guilds: vec![],
            folders: vec![],
            channels: vec![],
            roles: vec![],
            emoji: vec![],
            frequent_emoji: vec![],
            stickers: vec![],
            members: vec![],
            presences: BTreeMap::new(),
            voice: vec![],
            my_voice: None,
            speaking: BTreeSet::new(),
            typing: BTreeMap::new(),
            relationships: vec![],
            notes: BTreeMap::new(),
            messages: BTreeMap::new(),
            remote_messages: vec![],
            visible: vec![],
            read_states: BTreeMap::new(),
            overwrites: BTreeMap::new(),
            settings: BTreeMap::new(),
            permissions: BTreeMap::new(),
            composer: String::new(),
            pending_reply: None,
            editing: None,
        }
    }

    pub fn find_message(&self, channel: u64, id: u64) -> Option<&Message> {
        self.messages.get(&channel)?.iter().find(|m| m.id == id)
    }

    pub fn find_message_mut(&mut self, channel: u64, id: u64) -> Option<&mut Message> {
        self.messages
            .get_mut(&channel)?
            .iter_mut()
            .find(|m| m.id == id)
    }

    /// Append a message to a channel (as if it arrived) and show it.
    pub fn push_message(&mut self, m: Message) {
        self.visible.push(message_ref(&m));
        self.messages.entry(m.channel).or_default().push(m);
    }

    pub(super) fn channel(&self, id: u64) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }

    pub(super) fn member(&self, guild: u64, user: u64) -> Option<&api::Member> {
        self.members
            .iter()
            .find(|m| m.guild == guild && m.user == user)
    }

    /// Take a loaded message out of its channel and off the screen.
    pub(super) fn remove_message(&mut self, channel: u64, id: u64) -> Option<Message> {
        let list = self.messages.get_mut(&channel)?;
        let i = list.iter().position(|m| m.id == id)?;
        self.visible.retain(|r| r.id != id);
        Some(list.remove(i))
    }

    /// Drop a channel and everything the client holds about it.
    pub(super) fn remove_channel(&mut self, id: u64) {
        self.channels.retain(|c| c.id != id);
        self.messages.remove(&id);
        self.visible.retain(|r| r.channel != id);
        self.overwrites.remove(&id);
        self.permissions.remove(&id);
        self.read_states.remove(&id);
        self.typing.remove(&id);
    }

    // ---------------- permissions ----------------

    /// The user's permission bits in a channel or guild.
    pub(super) fn my_permissions(&self, id: u64) -> u64 {
        if let Some(p) = self.permissions.get(&id) {
            return *p;
        }
        self.channel(id)
            .and_then(|c| c.guild)
            .and_then(|g| self.permissions.get(&g))
            .copied()
            .unwrap_or(ALL_PERMISSIONS)
    }

    /// A member's permission bits in a guild: its `@everyone` role and
    /// `roles` together. The owner and administrators have every bit.
    pub(super) fn guild_permissions(&self, guild: &Guild, user: u64, roles: &[u64]) -> u64 {
        if guild.owner == user {
            return ALL_PERMISSIONS;
        }
        let role_bits = |id: u64| {
            self.roles
                .iter()
                .find(|r| r.guild == guild.id && r.id == id)
                .map_or(0, |r| r.permissions)
        };
        let bits = roles
            .iter()
            .fold(role_bits(guild.id), |bits, r| bits | role_bits(*r));
        if bits & ADMINISTRATOR == 0 {
            bits
        } else {
            ALL_PERMISSIONS
        }
    }

    /// `base` (a member's [guild permissions](Self::guild_permissions)) with
    /// a channel's overwrites applied, in Discord's order: the `@everyone`
    /// overwrite, then the member's roles' together, then the member's own.
    pub(super) fn channel_permissions(
        &self,
        guild: u64,
        channel: u64,
        base: u64,
        user: u64,
        roles: &[u64],
    ) -> u64 {
        if base & ADMINISTRATOR != 0 {
            return ALL_PERMISSIONS;
        }
        let overwrites = self.overwrites.get(&channel).map_or(&[][..], Vec::as_slice);
        let apply = |bits: u64, allow: u64, deny: u64| (bits & !deny) | allow;
        let mut bits = base;
        if let Some(o) = overwrites.iter().find(|o| !o.member && o.id == guild) {
            bits = apply(bits, o.allow, o.deny);
        }
        let (allow, deny) = overwrites
            .iter()
            .filter(|o| !o.member && roles.contains(&o.id))
            .fold((0, 0), |(a, d), o| (a | o.allow, d | o.deny));
        bits = apply(bits, allow, deny);
        if let Some(o) = overwrites.iter().find(|o| o.member && o.id == user) {
            bits = apply(bits, o.allow, o.deny);
        }
        bits
    }

    // ---------------- actions ----------------

    /// Change the world the way the client does when the user takes
    /// `action`. Actions that change nothing the mock's queries answer
    /// (volume, local mutes, opening links, ...) succeed and do nothing.
    #[expect(clippy::too_many_lines, reason = "one arm per action, read as a table")]
    pub(super) fn perform(&mut self, action: &Action) -> Result<(), ActionError> {
        match action {
            Action::MarkRead { channel } => self.mark_read(|c| c == *channel),
            Action::MarkGuildRead { guild } => {
                let channels: BTreeSet<u64> = self
                    .channels
                    .iter()
                    .filter(|c| c.guild == Some(*guild))
                    .map(|c| c.id)
                    .collect();
                self.mark_read(|c| channels.contains(&c));
            }
            Action::MarkAllRead => self.mark_read(|_| true),
            Action::MarkUnread { message } => {
                let r = self
                    .read_states
                    .entry(message.channel)
                    .or_insert(ReadState {
                        channel: message.channel,
                        unread: true,
                        mentions: 0,
                        last_read: None,
                        manual: true,
                    });
                r.unread = true;
                r.manual = true;
            }
            Action::StartReply { message, mention } => {
                self.find_message(message.channel, message.id)
                    .ok_or(ActionError::NotFound)?;
                self.pending_reply = Some((*message, *mention));
            }
            Action::StartEdit { message } => {
                if message.author != self.me {
                    return Err(ActionError::Forbidden);
                }
                self.editing = Some(*message);
            }
            Action::InsertText { text } => self.composer.push_str(text),
            Action::SetNote { user, note } => {
                if note.is_empty() {
                    self.notes.remove(user);
                } else {
                    self.notes.insert(*user, note.clone());
                }
            }
            Action::React {
                message,
                emoji,
                burst,
            } => apply_reaction(
                &mut self.message_mut(message)?.reactions,
                emoji,
                *burst,
                true,
                true,
            ),
            Action::Unreact {
                message,
                emoji,
                burst,
            } => apply_reaction(
                &mut self.message_mut(message)?.reactions,
                emoji,
                *burst,
                true,
                false,
            ),
            Action::SetPinned { message, pinned } => self.message_mut(message)?.pinned = *pinned,
            Action::DeleteMessage { message, .. } => {
                self.remove_message(message.channel, message.id)
                    .ok_or(ActionError::NotFound)?;
            }
            Action::EditMessage { message, content } => {
                let me = self.me;
                let m = self.message_mut(message)?;
                if m.author.id != me {
                    return Err(ActionError::Forbidden);
                }
                m.content.clone_from(content);
                m.edited_ms = Some(m.edited_ms.unwrap_or(0) + 1);
            }
            Action::SuppressEmbeds { message } => self.message_mut(message)?.embeds.clear(),
            Action::Vote { message, answers } => {
                let poll = self
                    .message_mut(message)?
                    .poll
                    .as_mut()
                    .ok_or(ActionError::Invalid)?;
                poll.my_votes.clone_from(answers);
            }
            Action::SendMessage {
                channel,
                content,
                files,
            } => self.send_message(*channel, content, files)?,
            Action::JoinVoice { channel } => self.join_voice(*channel),
            Action::SetSelfMute { muted } => {
                if let Some(v) = &mut self.my_voice {
                    v.self_mute = *muted;
                }
            }
            Action::SetSelfDeaf { deafened } => {
                if let Some(v) = &mut self.my_voice {
                    v.self_deaf = *deafened;
                }
            }
            Action::SetStatus { status } => self.status.clone_from(status),
            Action::SetCustomStatus {
                text,
                emoji,
                expires_ms,
            } => {
                self.custom_status = (text.is_some() || emoji.is_some()).then(|| CustomStatus {
                    text: text.clone(),
                    emoji: emoji.clone(),
                    expires_ms: *expires_ms,
                });
            }
            Action::SetNickname { guild, nick } => {
                let me = self.me;
                if let Some(m) = self
                    .members
                    .iter_mut()
                    .find(|m| m.guild == *guild && m.user == me)
                {
                    m.nick.clone_from(nick);
                }
            }
            Action::Navigate {
                to: api::Target::Channel(c),
            } => {
                self.selection.channel = Some(c.id);
                self.selection.guild = c.guild;
            }
            Action::BulkDelete { channel, messages } => {
                if let Some(all) = self.messages.get_mut(channel) {
                    all.retain(|m| !messages.contains(&m.id));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn message_mut(&mut self, r: &MessageRef) -> Result<&mut Message, ActionError> {
        self.find_message_mut(r.channel, r.id)
            .ok_or(ActionError::NotFound)
    }

    fn mark_read(&mut self, channel: impl Fn(u64) -> bool) {
        for r in self.read_states.values_mut().filter(|r| channel(r.channel)) {
            r.unread = false;
            r.mentions = 0;
        }
    }

    fn send_message(
        &mut self,
        channel: u64,
        content: &str,
        files: &[OutgoingFile],
    ) -> Result<(), ActionError> {
        let guild = self.channel(channel).ok_or(ActionError::NotFound)?.guild;
        let me = self.me;
        let name = self
            .users
            .get(&me)
            .map(|u| u.username.clone())
            .unwrap_or_default();
        let list = self.messages.entry(channel).or_default();
        let id = list.iter().map(|m| m.id).max().unwrap_or(0) + 1;
        let mut m = fixtures::message(channel, guild, id, (me, &name), content);
        m.attachments = (id + 1..)
            .zip(files)
            .map(|(attachment, f)| Attachment {
                id: attachment,
                filename: f.filename.clone(),
                content_type: f.content_type.clone(),
                size: f.size,
                width: None,
                height: None,
                spoiler: f.spoiler,
                description: None,
                duration_secs: None,
                url: format!(
                    "https://cdn.discordapp.com/attachments/{channel}/{attachment}/{}",
                    f.filename
                ),
            })
            .collect();
        list.push(m);
        Ok(())
    }

    fn join_voice(&mut self, channel: Option<u64>) {
        self.my_voice = channel.map(|c| MyVoice {
            channel: c,
            guild: self.channel(c).and_then(|x| x.guild),
            // The mock has no clock.
            since_ms: fixtures::T0,
            self_mute: false,
            self_deaf: false,
            self_video: false,
            streaming: false,
            connection: "connected".into(),
            ping_ms: Some(42),
        });
        self.selection.voice_channel = channel;
    }
}

/// A reference to `m`.
pub(crate) fn message_ref(m: &Message) -> MessageRef {
    MessageRef {
        id: m.id,
        channel: m.channel,
        guild: m.guild,
        author: m.author.id,
    }
}

/// Add one reaction to a message's `reactions`, or take one away. A
/// reaction of the user's own (`mine`) that they already have, or don't,
/// changes nothing.
pub(super) fn apply_reaction(
    reactions: &mut Vec<Reaction>,
    emoji: &str,
    burst: bool,
    mine: bool,
    added: bool,
) {
    let i = match reactions.iter().position(|r| r.emoji == emoji) {
        Some(i) => i,
        None if added => {
            reactions.push(Reaction {
                emoji: emoji.to_string(),
                count: 0,
                burst_count: 0,
                me: false,
                me_burst: false,
            });
            reactions.len() - 1
        }
        None => return,
    };
    let r = &mut reactions[i];
    if mine {
        let flag = if burst { &mut r.me_burst } else { &mut r.me };
        if *flag == added {
            return;
        }
        *flag = added;
    }
    let step = |n: u32| {
        if added {
            n.saturating_add(1)
        } else {
            n.saturating_sub(1)
        }
    };
    r.count = step(r.count);
    if burst {
        r.burst_count = step(r.burst_count);
    }
    if r.count == 0 {
        reactions.remove(i);
    }
}
