//! Messages and what they carry: attachments, embeds, reactions, polls,
//! references and forwarded snapshots.

use serde::Deserialize;

use super::{Emoji, Member, User, nullable, opt_snowflake, opt_snowflakes, snowflake};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Attachment {
    #[serde(deserialize_with = "snowflake")]
    pub id: u64,
    #[serde(default)]
    pub filename: String,
    pub content_type: Option<String>,
    #[serde(default)]
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub description: Option<String>,
    pub duration_secs: Option<f32>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub flags: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbedMedia {
    #[serde(default)]
    pub url: String,
    #[serde(default, deserialize_with = "nullable")]
    pub width: u32,
    #[serde(default, deserialize_with = "nullable")]
    pub height: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Named {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbedField {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Embed {
    #[serde(rename = "type", default)]
    pub kind: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub color: Option<u32>,
    pub author: Option<Named>,
    pub provider: Option<Named>,
    pub footer: Option<Named>,
    #[serde(default, deserialize_with = "nullable")]
    pub fields: Vec<EmbedField>,
    pub image: Option<EmbedMedia>,
    pub thumbnail: Option<EmbedMedia>,
    pub video: Option<EmbedMedia>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CountDetails {
    #[serde(default)]
    pub burst: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Reaction {
    pub emoji: Emoji,
    #[serde(default)]
    pub count: u32,
    #[serde(default)]
    pub count_details: CountDetails,
    #[serde(default)]
    pub me: bool,
    #[serde(default)]
    pub me_burst: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PollMedia {
    pub text: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PollAnswer {
    #[serde(default)]
    pub answer_id: u32,
    #[serde(default)]
    pub poll_media: PollMedia,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PollAnswerCount {
    pub id: u32,
    #[serde(default)]
    pub count: u32,
    #[serde(default)]
    pub me_voted: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PollResults {
    #[serde(default)]
    pub is_finalized: bool,
    #[serde(default)]
    pub answer_counts: Vec<PollAnswerCount>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Poll {
    #[serde(default)]
    pub question: PollMedia,
    #[serde(default)]
    pub answers: Vec<PollAnswer>,
    pub expiry: Option<String>,
    #[serde(default)]
    pub allow_multiselect: bool,
    pub results: Option<PollResults>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MessageReference {
    #[serde(rename = "type", default)]
    pub kind: u8,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub message_id: Option<u64>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub channel_id: Option<u64>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub guild_id: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub content: String,
    #[serde(default, deserialize_with = "nullable")]
    pub attachments: Vec<Attachment>,
    #[serde(default, deserialize_with = "nullable")]
    pub embeds: Vec<Embed>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MessageSnapshot {
    pub message: Snapshot,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StickerItem {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct IdOnly {
    #[serde(deserialize_with = "snowflake")]
    pub id: u64,
}

/// `MESSAGE_CREATE`, and `MESSAGE_UPDATE`, where every field but the ids may
/// be missing (an embed unfurl sends only `embeds`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Message {
    #[serde(deserialize_with = "snowflake")]
    pub id: u64,
    #[serde(deserialize_with = "snowflake")]
    pub channel_id: u64,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub guild_id: Option<u64>,
    pub author: Option<User>,
    pub member: Option<Member>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    pub webhook_id: Option<u64>,
    #[serde(rename = "type")]
    pub kind: Option<u16>,
    pub flags: Option<u64>,
    pub content: Option<String>,
    pub edited_timestamp: Option<String>,
    pub pinned: Option<bool>,
    pub tts: Option<bool>,
    pub mention_everyone: Option<bool>,
    pub mentions: Option<Vec<User>>,
    #[serde(default, deserialize_with = "opt_snowflakes")]
    pub mention_roles: Option<Vec<u64>>,
    pub message_reference: Option<MessageReference>,
    pub referenced_message: Option<Box<Message>>,
    pub attachments: Option<Vec<Attachment>>,
    pub embeds: Option<Vec<Embed>>,
    pub reactions: Option<Vec<Reaction>>,
    pub sticker_items: Option<Vec<StickerItem>>,
    pub poll: Option<Poll>,
    pub thread: Option<IdOnly>,
    pub message_snapshots: Option<Vec<MessageSnapshot>>,
}
