//! A small sample client for [`MockBackend`](super::MockBackend): two guilds,
//! a handful of channels (one hidden), roles, members, presences, a voice
//! call, a DM, a few messages and read states. Enough for plugins to have
//! something real to query under `rekindle dev` and in tests.

use std::collections::{BTreeMap, BTreeSet};

use crate::api::{
    self,
    records::{
        Activity, Channel, ClientStatus, Emoji, Guild, GuildFolder, Message, Overwrite, Presence,
        Reaction, ReadState, Relationship, RelationshipKind, Role, Selection, Sticker, User,
        VoiceState,
    },
};

use super::gateway::convert::rgb;
use super::mock::{ADMINISTRATOR, ALL_PERMISSIONS, VIEW_CHANNEL, World, message_ref};
use crate::model::{Tweak, TweakKind};

const ADD_REACTIONS: u64 = 1 << 6;
const SEND_MESSAGES: u64 = 1 << 11;

pub const ME: u64 = 100;
pub const ALICE: u64 = 101;
pub const BOT: u64 = 102;
pub const CAROL: u64 = 103;

pub const LAB: u64 = 1000;
pub const LAB_CATEGORY: u64 = 1001;
pub const GENERAL: u64 = 1002;
pub const OFF_TOPIC: u64 = 1003;
pub const LAB_VOICE: u64 = 1004;
pub const STAFF_ONLY: u64 = 1005;
pub const GAMERS: u64 = 2000;
pub const GAMERS_GENERAL: u64 = 2001;
pub const DM_ALICE: u64 = 9001;

/// 2026-01-01T00:00:00Z, the sample's notion of "a while ago".
pub(crate) const T0: u64 = 1_767_225_600_000;

fn ms_to_snowflake(ms: u64, n: u64) -> u64 {
    ((ms - 1_420_070_400_000) << 22) | n
}

/// Message ids carry real timestamps (so `host.snowflake-ms` and
/// `format-time` give sensible answers).
pub fn message_id(minutes_after_t0: u64, n: u64) -> u64 {
    ms_to_snowflake(T0 + minutes_after_t0 * 60_000, n)
}

fn user(id: u64, username: &str, global: Option<&str>, bot: bool) -> User {
    User {
        id,
        username: username.into(),
        global_name: global.map(Into::into),
        bot,
        system: false,
        public_flags: 0,
        accent: None,
        has_decoration: false,
        ..Default::default()
    }
}

fn uref(id: u64, name: &str) -> api::UserRef {
    api::UserRef {
        id,
        name: name.into(),
    }
}

fn channel(
    id: u64,
    kind: u16,
    guild: Option<u64>,
    parent: Option<u64>,
    name: &str,
    position: i32,
) -> Channel {
    Channel {
        id,
        kind,
        guild,
        parent,
        name: name.into(),
        topic: None,
        position,
        nsfw: false,
        slowmode: 0,
        last_message: None,
        recipients: vec![],
        thread: None,
        forum_tags: vec![],
        muted: false,
        visible: true,
        ..Default::default()
    }
}

fn guild(id: u64, name: &str, owner: u64, members: u32) -> Guild {
    Guild {
        id,
        name: name.into(),
        owner,
        description: None,
        features: vec![],
        premium_tier: 0,
        premium_subscribers: 0,
        verification_level: 1,
        nsfw_level: 0,
        preferred_locale: "en-US".into(),
        member_count: Some(members),
        joined_ms: Some(T0 - 400 * 86_400_000),
        muted: false,
        ..Default::default()
    }
}

fn role(
    id: u64,
    guild: u64,
    name: &str,
    color: Option<u32>,
    position: i32,
    permissions: u64,
) -> Role {
    Role {
        id,
        guild,
        name: name.into(),
        color: color.map(rgb),
        colors_extra: vec![],
        position,
        permissions,
        hoist: color.is_some(),
        mentionable: false,
        managed: false,
        unicode_emoji: None,
        ..Default::default()
    }
}

fn member(guild: u64, user: u64, nick: Option<&str>, roles: Vec<u64>) -> api::Member {
    api::Member {
        guild,
        user,
        nick: nick.map(Into::into),
        roles,
        joined_ms: Some(T0 - 200 * 86_400_000),
        boosting_since_ms: None,
        timed_out_until_ms: None,
        pending: false,
        ..Default::default()
    }
}

pub fn message(
    channel: u64,
    guild: Option<u64>,
    id: u64,
    author: (u64, &str),
    content: &str,
) -> Message {
    Message {
        id,
        channel,
        guild,
        author: uref(author.0, author.1),
        bot: author.0 == BOT,
        webhook: false,
        kind: 0,
        flag_bits: 0,
        content: content.into(),
        edited_ms: None,
        pinned: false,
        tts: false,
        mentions_everyone: false,
        mentions: vec![],
        mention_roles: vec![],
        reply_to: None,
        attachments: vec![],
        embeds: vec![],
        reactions: vec![],
        stickers: vec![],
        poll: None,
        thread: None,
        forwarded: None,
        ephemeral: false,
        sending: false,
        failed: false,
        ..Default::default()
    }
}

fn presence(
    user: u64,
    status: &str,
    desktop: &str,
    mobile: &str,
    web: &str,
    activities: Vec<Activity>,
) -> Presence {
    Presence {
        user,
        status: status.into(),
        client_status: ClientStatus {
            desktop: desktop.into(),
            mobile: mobile.into(),
            web: web.into(),
            embedded: String::new(),
        },
        activities,
    }
}

/// The sample client, logged in as [`ME`] with [`GENERAL`] open.
#[expect(
    clippy::too_many_lines,
    reason = "one literal world, read top to bottom"
)]
pub fn sample_world() -> World {
    let mut users = BTreeMap::new();
    for u in [
        user(ME, "you", Some("You"), false),
        user(ALICE, "alice", Some("Alice"), false),
        user(BOT, "helperbot", None, true),
        user(CAROL, "carol", Some("Carol"), false),
    ] {
        users.insert(u.id, u);
    }

    let mut hidden = channel(
        STAFF_ONLY,
        0,
        Some(LAB),
        Some(LAB_CATEGORY),
        "staff-only",
        3,
    );
    hidden.visible = false;
    let mut general = channel(GENERAL, 0, Some(LAB), Some(LAB_CATEGORY), "general", 0);
    general.topic = Some("Say hi".into());
    let mut dm = channel(DM_ALICE, 1, None, None, "alice", 0);
    dm.recipients = vec![uref(ALICE, "Alice")];
    let channels = vec![
        channel(LAB_CATEGORY, 4, Some(LAB), None, "Text Channels", 0),
        general,
        channel(OFF_TOPIC, 0, Some(LAB), Some(LAB_CATEGORY), "off-topic", 1),
        channel(LAB_VOICE, 2, Some(LAB), Some(LAB_CATEGORY), "Lounge", 2),
        hidden,
        channel(GAMERS_GENERAL, 0, Some(GAMERS), None, "general", 0),
        dm,
    ];

    let roles = vec![
        role(
            LAB,
            LAB,
            "@everyone",
            None,
            0,
            VIEW_CHANNEL | SEND_MESSAGES | ADD_REACTIONS,
        ),
        role(
            1100,
            LAB,
            "Moderator",
            Some(0x3ba55d),
            2,
            ALL_PERMISSIONS & !ADMINISTRATOR,
        ),
        role(
            1101,
            LAB,
            "Bots",
            Some(0x5865f2),
            1,
            VIEW_CHANNEL | SEND_MESSAGES,
        ),
        role(
            GAMERS,
            GAMERS,
            "@everyone",
            None,
            0,
            VIEW_CHANNEL | SEND_MESSAGES,
        ),
    ];

    let members = vec![
        member(LAB, ME, None, vec![1100]),
        member(LAB, ALICE, Some("Ali"), vec![]),
        member(LAB, BOT, None, vec![1101]),
        member(LAB, CAROL, None, vec![]),
        member(GAMERS, ME, None, vec![]),
        member(GAMERS, ALICE, None, vec![]),
    ];

    let playing = Activity {
        kind: 0,
        name: "Factorio".into(),
        details: Some("Building a main bus".into()),
        state: None,
        application: None,
        started_ms: Some(T0),
        ends_ms: None,
        emoji: None,
        url: None,
    };
    let mut presences = BTreeMap::new();
    presences.insert(
        ALICE,
        presence(ALICE, "online", "online", "idle", "", vec![playing]),
    );
    presences.insert(BOT, presence(BOT, "online", "", "", "online", vec![]));
    presences.insert(CAROL, presence(CAROL, "dnd", "dnd", "", "", vec![]));

    let gid = Some(LAB);
    let mut hello = message(
        GENERAL,
        gid,
        message_id(10, 1),
        (ALICE, "Ali"),
        "hello everyone!",
    );
    hello.reactions = vec![Reaction {
        emoji: "👋".into(),
        count: 2,
        burst_count: 0,
        me: false,
        me_burst: false,
    }];
    let mut reply = message(
        GENERAL,
        gid,
        message_id(12, 2),
        (ME, "You"),
        "hi alice, welcome back",
    );
    reply.reply_to = Some(api::MessageRef {
        id: hello.id,
        channel: GENERAL,
        guild: gid,
        author: ALICE,
    });
    reply.kind = 19;
    let bot_msg = message(
        GENERAL,
        gid,
        message_id(15, 3),
        (BOT, "helperbot"),
        "Reminder: the meeting is at 18:00.",
    );
    let link = format!(
        "see https://discord.com/channels/{LAB}/{OFF_TOPIC}/{} for context",
        message_id(5, 9)
    );
    let linky = message(GENERAL, gid, message_id(20, 4), (CAROL, "Carol"), &link);
    let general_msgs = vec![hello, reply, bot_msg, linky];
    let visible = general_msgs.iter().map(message_ref).collect();
    let off_topic_old = message(
        OFF_TOPIC,
        gid,
        message_id(5, 9),
        (ALICE, "Ali"),
        "the build is green again",
    );
    let dm_msgs = vec![message(
        DM_ALICE,
        None,
        message_id(30, 5),
        (ALICE, "Alice"),
        "are you around later?",
    )];

    let mut messages = BTreeMap::new();
    messages.insert(GENERAL, general_msgs);
    messages.insert(DM_ALICE, dm_msgs);

    let mut read_states = BTreeMap::new();
    read_states.insert(
        GENERAL,
        ReadState {
            channel: GENERAL,
            unread: true,
            mentions: 0,
            last_read: Some(message_id(10, 1)),
            manual: false,
        },
    );
    read_states.insert(
        DM_ALICE,
        ReadState {
            channel: DM_ALICE,
            unread: true,
            mentions: 1,
            last_read: None,
            manual: false,
        },
    );
    read_states.insert(
        OFF_TOPIC,
        ReadState {
            channel: OFF_TOPIC,
            unread: false,
            mentions: 0,
            last_read: Some(message_id(5, 9)),
            manual: false,
        },
    );

    let voice = vec![VoiceState {
        user: CAROL,
        guild: Some(LAB),
        channel: LAB_VOICE,
        mute: false,
        deaf: false,
        self_mute: true,
        self_deaf: false,
        self_video: false,
        self_stream: false,
        suppress: false,
    }];

    let mut settings = BTreeMap::new();
    for (k, v) in [
        ("appearance.message-display-compact", "false"),
        ("text-and-images.gif-auto-play", "true"),
        ("text-and-images.animate-emoji", "true"),
        ("notifications.quiet-mode", "false"),
        ("accessibility.reduced-motion", "false"),
    ] {
        settings.insert(k.to_string(), v.to_string());
    }

    let emoji = vec![
        Emoji {
            id: 5001,
            name: "rekindle".into(),
            animated: false,
            guild: Some(LAB),
            available: true,
        },
        Emoji {
            id: 5002,
            name: "partyblob".into(),
            animated: true,
            guild: Some(LAB),
            available: true,
        },
    ];
    let frequent = vec![
        Emoji {
            id: 0,
            name: "👍".into(),
            animated: false,
            guild: None,
            available: true,
        },
        Emoji {
            id: 5001,
            name: "rekindle".into(),
            animated: false,
            guild: Some(LAB),
            available: true,
        },
        Emoji {
            id: 0,
            name: "😂".into(),
            animated: false,
            guild: None,
            available: true,
        },
    ];

    World {
        me: ME,
        status: "online".into(),
        custom_status: None,
        selection: Selection {
            guild: Some(LAB),
            channel: Some(GENERAL),
            voice_channel: None,
            side_thread: None,
            focused: true,
            idle: false,
        },
        users,
        guilds: vec![
            guild(LAB, "Rekindle Lab", ME, 4),
            guild(GAMERS, "Gamers", ALICE, 2),
        ],
        folders: vec![
            GuildFolder {
                id: None,
                name: None,
                color: None,
                guilds: vec![LAB],
            },
            GuildFolder {
                id: Some(77),
                name: Some("Games".into()),
                color: Some(rgb(0xed4245)),
                guilds: vec![GAMERS],
            },
        ],
        channels,
        roles,
        emoji,
        frequent_emoji: frequent,
        stickers: vec![Sticker {
            id: 6001,
            name: "wave".into(),
            guild: Some(LAB),
            format: 1,
        }],
        members,
        presences,
        voice,
        my_voice: None,
        speaking: BTreeSet::new(),
        typing: BTreeMap::new(),
        relationships: vec![
            Relationship {
                user: ALICE,
                kind: RelationshipKind::Friend,
                nickname: None,
                since_ms: Some(T0 - 900 * 86_400_000),
            },
            Relationship {
                user: CAROL,
                kind: RelationshipKind::Incoming,
                nickname: None,
                since_ms: None,
            },
        ],
        notes: BTreeMap::from([(BOT, "reminds us about meetings".to_string())]),
        messages,
        remote_messages: vec![off_topic_old],
        visible,
        read_states,
        overwrites: BTreeMap::from([(
            STAFF_ONLY,
            vec![Overwrite {
                id: LAB,
                member: false,
                allow: 0,
                deny: VIEW_CHANNEL,
            }],
        )]),
        settings,
        permissions: BTreeMap::new(),
        composer: String::new(),
        pending_reply: None,
        editing: None,
    }
}

/// The host tweaks the mock offers, at their defaults.
pub(crate) fn sample_tweaks() -> Vec<Tweak> {
    let toggle = |id: &str, label: &str, description: &str| Tweak {
        id: id.into(),
        label: label.into(),
        description: description.into(),
        kind: TweakKind::Toggle,
        choices: vec![],
        value: "false".into(),
        available: true,
    };
    vec![
        toggle(
            "silent-typing",
            "Silent typing",
            "Don't send typing indicators.",
        ),
        toggle(
            "always-animate",
            "Always animate",
            "Animate avatars and emoji without hovering.",
        ),
        toggle(
            "no-typing-animation",
            "No typing animation",
            "Show the typing indicator without animating it.",
        ),
        toggle(
            "no-reply-mention",
            "Don't mention on reply",
            "Replies don't ping by default.",
        ),
        toggle(
            "no-mosaic",
            "No attachment mosaic",
            "Show attachments one per row.",
        ),
        toggle(
            "disable-call-idle",
            "Stay in calls",
            "Don't leave calls when idle.",
        ),
        Tweak {
            id: "gif-picker-size".into(),
            label: "GIF picker".into(),
            description: "Size of the GIF picker.".into(),
            kind: TweakKind::Choice,
            choices: ["default", "compact", "large"].map(String::from).to_vec(),
            value: "default".into(),
            available: true,
        },
        toggle(
            "member-join-dates",
            "Member join dates",
            "Show when members joined in their profile.",
        ),
    ]
}
