//! Gateway records into the contract's records.

use super::wire;
use crate::api;

pub fn rgb(c: u32) -> api::WitColor {
    api::WitColor {
        r: (c >> 16) as u8,
        g: (c >> 8) as u8,
        b: c as u8,
        a: 255,
    }
}

/// Unicode emoji, or `name:id` for a custom one.
pub fn emoji_text(e: &wire::Emoji) -> String {
    let name = e.name.clone().unwrap_or_default();
    match e.id {
        Some(id) => format!("{name}:{id}"),
        None => name,
    }
}

pub fn role(r: wire::Role, guild: u64) -> api::Role {
    let primary = r
        .colors
        .as_ref()
        .and_then(|c| c.primary_color)
        .unwrap_or(r.color);
    let extra = r
        .colors
        .as_ref()
        .map(|c| {
            [c.secondary_color, c.tertiary_color]
                .into_iter()
                .flatten()
                .map(rgb)
                .collect()
        })
        .unwrap_or_default();
    api::Role {
        id: r.id,
        guild,
        name: r.name,
        color: (primary != 0).then(|| rgb(primary)),
        colors_extra: extra,
        position: r.position,
        permissions: r.permissions,
        hoist: r.hoist,
        mentionable: r.mentionable,
        managed: r.managed,
        unicode_emoji: r.unicode_emoji,
    }
}

pub fn emoji(e: wire::Emoji, guild: Option<u64>) -> Option<api::Emoji> {
    Some(api::Emoji {
        id: e.id?,
        name: e.name.unwrap_or_default(),
        animated: e.animated,
        guild,
        available: e.available,
    })
}

pub fn sticker(s: wire::Sticker, guild: u64) -> api::Sticker {
    api::Sticker {
        id: s.id,
        name: s.name,
        guild: Some(guild),
        format: s.format_type,
    }
}

pub fn relationship(r: &wire::Relationship) -> Option<api::Relationship> {
    use api::RelationshipKind as K;
    let kind = match r.kind {
        1 => K::Friend,
        2 => K::Blocked,
        3 => K::Incoming,
        4 => K::Outgoing,
        5 => K::Implicit,
        _ if r.user_ignored => K::Ignored,
        _ => return None,
    };
    Some(api::Relationship {
        user: r.id,
        kind,
        nickname: r.nickname.clone(),
        since_ms: r.since.as_deref().and_then(iso_ms),
    })
}

pub fn activity(a: &wire::Activity) -> api::Activity {
    api::Activity {
        kind: a.kind,
        name: a.name.clone(),
        details: a.details.clone(),
        state: a.state.clone(),
        application: a.application_id,
        started_ms: a.timestamps.as_ref().and_then(|t| t.start),
        ends_ms: a.timestamps.as_ref().and_then(|t| t.end),
        emoji: a.emoji.as_ref().map(emoji_text),
        url: a.url.clone(),
    }
}

pub fn attachment(a: &wire::Attachment) -> api::Attachment {
    api::Attachment {
        id: a.id,
        filename: a.filename.clone(),
        content_type: a.content_type.clone(),
        size: a.size,
        width: a.width,
        height: a.height,
        spoiler: a.filename.starts_with("SPOILER_") || a.flags & (1 << 3) != 0,
        description: a.description.clone(),
        duration_secs: a.duration_secs,
        url: a.url.clone(),
    }
}

pub fn embed(e: &wire::Embed) -> api::Embed {
    let media = |m: &Option<wire::EmbedMedia>| {
        m.as_ref().map(|m| api::EmbedMedia {
            url: m.url.clone(),
            width: m.width,
            height: m.height,
        })
    };
    api::Embed {
        kind: e.kind.clone(),
        title: e.title.clone(),
        description: e.description.clone(),
        url: e.url.clone(),
        color: e.color.map(rgb),
        author: e.author.as_ref().map(|a| a.name.clone()),
        provider: e.provider.as_ref().map(|a| a.name.clone()),
        footer: e.footer.as_ref().map(|a| a.text.clone()),
        fields: e
            .fields
            .iter()
            .map(|f| (f.name.clone(), f.value.clone()))
            .collect(),
        image: media(&e.image),
        thumbnail: media(&e.thumbnail),
        video: media(&e.video),
    }
}

pub fn poll(p: &wire::Poll) -> api::Poll {
    let counts = p
        .results
        .as_ref()
        .map(|r| r.answer_counts.clone())
        .unwrap_or_default();
    api::Poll {
        question: p.question.text.clone().unwrap_or_default(),
        answers: p
            .answers
            .iter()
            .map(|a| {
                let votes = counts
                    .iter()
                    .find(|c| c.id == a.answer_id)
                    .map_or(0, |c| c.count);
                (
                    a.answer_id,
                    a.poll_media.text.clone().unwrap_or_default(),
                    votes,
                )
            })
            .collect(),
        expires_ms: p.expiry.as_deref().and_then(iso_ms),
        multiselect: p.allow_multiselect,
        finalized: p.results.as_ref().is_some_and(|r| r.is_finalized),
        my_votes: counts.iter().filter(|c| c.me_voted).map(|c| c.id).collect(),
    }
}

/// An ISO 8601 timestamp (`2026-11-10T12:34:56.789000+00:00`) as epoch ms.
pub fn iso_ms(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse().ok() };
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut i = 19;
    let mut ms = 0i64;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        let frac = &s[start..i];
        let digits: String = frac.chars().chain(std::iter::repeat('0')).take(3).collect();
        ms = digits.parse().ok()?;
    }
    let offset_min = match b.get(i) {
        None | Some(b'Z') => 0,
        Some(c @ (b'+' | b'-')) => {
            let sign = if *c == b'+' { 1 } else { -1 };
            sign * (num(i + 1..i + 3)? * 60 + num(i + 4..i + 6)?)
        }
        _ => return None,
    };
    // Days from civil (Howard Hinnant).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let total = ((days * 24 + h) * 60 + mi - offset_min) * 60 + sec;
    u64::try_from(total * 1000 + ms).ok()
}

/// A message from `MESSAGE_CREATE`, or from `MESSAGE_UPDATE` over the copy
/// the client held (`old`), whose fields the update may leave out.
/// `name` gives a user's display name in a guild, and `author_of` the
/// author of a message the client holds.
#[expect(
    clippy::too_many_lines,
    reason = "one field per line of the record, each from the update or the old copy"
)]
pub fn message(
    m: &wire::Message,
    guild: Option<u64>,
    old: Option<&api::Message>,
    name: &dyn Fn(Option<u64>, u64) -> String,
    author_of: &dyn Fn(u64, u64) -> Option<u64>,
) -> api::Message {
    let author_id = m
        .author
        .as_ref()
        .map(|a| a.id)
        .or(old.map(|o| o.author.id))
        .unwrap_or(0);
    let shown = match (&m.member, m.author.as_ref()) {
        (Some(mem), Some(a)) => mem
            .nick
            .clone()
            .or(a.global_name.clone())
            .or(a.username.clone())
            .unwrap_or_default(),
        _ => old.map_or_else(|| name(guild, author_id), |o| o.author.name.clone()),
    };
    let reply_to = m
        .message_reference
        .as_ref()
        .filter(|r| r.kind == 0)
        .and_then(|r| {
            let id = r.message_id?;
            let channel = r.channel_id.unwrap_or(m.channel_id);
            let author = m
                .referenced_message
                .as_ref()
                .and_then(|x| x.author.as_ref().map(|a| a.id))
                .or_else(|| author_of(channel, id))
                .unwrap_or(0);
            Some(api::MessageRef {
                id,
                channel,
                guild: r.guild_id.or(guild),
                author,
            })
        });
    let forwarded = m
        .message_snapshots
        .as_ref()
        .and_then(|s| s.first())
        .map(|s| api::Forwarded {
            content: s.message.content.clone(),
            attachments: s.message.attachments.iter().map(attachment).collect(),
            embeds: s.message.embeds.iter().map(embed).collect(),
            origin: m
                .message_reference
                .as_ref()
                .filter(|r| r.kind == 1)
                .and_then(|r| {
                    Some(api::MessageRef {
                        id: r.message_id?,
                        channel: r.channel_id?,
                        guild: r.guild_id,
                        author: 0,
                    })
                }),
        });
    let flags = m.flags.or(old.map(|o| o.flag_bits)).unwrap_or(0);
    api::Message {
        id: m.id,
        channel: m.channel_id,
        guild,
        author: api::UserRef {
            id: author_id,
            name: shown,
        },
        bot: m
            .author
            .as_ref()
            .map(|a| a.bot)
            .or(old.map(|o| o.bot))
            .unwrap_or(false),
        webhook: m.webhook_id.is_some() || old.is_some_and(|o| o.webhook),
        kind: m.kind.or(old.map(|o| o.kind)).unwrap_or(0),
        flag_bits: flags,
        content: m
            .content
            .clone()
            .or(old.map(|o| o.content.clone()))
            .unwrap_or_default(),
        edited_ms: m
            .edited_timestamp
            .as_deref()
            .and_then(iso_ms)
            .or(old.and_then(|o| o.edited_ms)),
        pinned: m.pinned.or(old.map(|o| o.pinned)).unwrap_or(false),
        tts: m.tts.or(old.map(|o| o.tts)).unwrap_or(false),
        mentions_everyone: m
            .mention_everyone
            .or(old.map(|o| o.mentions_everyone))
            .unwrap_or(false),
        mentions: m
            .mentions
            .as_ref()
            .map(|v| v.iter().map(|u| u.id).collect())
            .or(old.map(|o| o.mentions.clone()))
            .unwrap_or_default(),
        mention_roles: m
            .mention_roles
            .clone()
            .or(old.map(|o| o.mention_roles.clone()))
            .unwrap_or_default(),
        reply_to: reply_to.or(old.and_then(|o| o.reply_to)),
        attachments: m
            .attachments
            .as_ref()
            .map(|v| v.iter().map(attachment).collect())
            .or(old.map(|o| o.attachments.clone()))
            .unwrap_or_default(),
        embeds: m
            .embeds
            .as_ref()
            .map(|v| v.iter().map(embed).collect())
            .or(old.map(|o| o.embeds.clone()))
            .unwrap_or_default(),
        reactions: m
            .reactions
            .as_ref()
            .map(|v| {
                v.iter()
                    .map(|r| api::Reaction {
                        emoji: emoji_text(&r.emoji),
                        count: r.count,
                        burst_count: r.count_details.burst,
                        me: r.me,
                        me_burst: r.me_burst,
                    })
                    .collect()
            })
            .or(old.map(|o| o.reactions.clone()))
            .unwrap_or_default(),
        stickers: m
            .sticker_items
            .as_ref()
            .map(|v| v.iter().map(|s| s.name.clone()).collect())
            .or(old.map(|o| o.stickers.clone()))
            .unwrap_or_default(),
        poll: m
            .poll
            .as_ref()
            .map(poll)
            .or(old.and_then(|o| o.poll.clone())),
        thread: m
            .thread
            .as_ref()
            .map(|t| t.id)
            .or(old.and_then(|o| o.thread)),
        forwarded: forwarded.or(old.and_then(|o| o.forwarded.clone())),
        ephemeral: flags & (1 << 6) != 0,
        sending: false,
        failed: false,
    }
}

pub fn presence(p: &wire::Presence) -> Option<api::Presence> {
    let s = |v: &Option<String>| v.clone().unwrap_or_default();
    Some(api::Presence {
        user: p.user_id()?,
        status: p.status.clone(),
        client_status: api::ClientStatus {
            desktop: s(&p.client_status.desktop),
            mobile: s(&p.client_status.mobile),
            web: s(&p.client_status.web),
            embedded: s(&p.client_status.embedded),
        },
        activities: p.activities.iter().map(activity).collect(),
    })
}

/// `None` when the user left voice (no channel).
pub fn voice_state(v: &wire::VoiceState) -> Option<api::VoiceState> {
    Some(api::VoiceState {
        user: v.user_id,
        guild: v.guild_id,
        channel: v.channel_id?,
        mute: v.mute,
        deaf: v.deaf,
        self_mute: v.self_mute,
        self_deaf: v.self_deaf,
        self_video: v.self_video,
        self_stream: v.self_stream,
        suppress: v.suppress,
    })
}

pub fn member(guild: u64, m: &wire::Member) -> Option<api::Member> {
    Some(api::Member {
        guild,
        user: m.user_id()?,
        nick: m.nick.clone(),
        roles: m.roles.clone(),
        joined_ms: m.joined_at.as_deref().and_then(iso_ms),
        boosting_since_ms: m.premium_since.as_deref().and_then(iso_ms),
        timed_out_until_ms: m.communication_disabled_until.as_deref().and_then(iso_ms),
        pending: m.pending,
    })
}
