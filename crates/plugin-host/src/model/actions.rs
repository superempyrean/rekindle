//! What compose returns, and the actions a plugin takes as the user
//! (design doc §5).

use serde::{Deserialize, Serialize};

/// The outcome of running the user's outgoing message through `compose`
/// plugins ([`Runtime::compose`](crate::Runtime::compose)).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Composed {
    /// The text to send.
    pub text: String,
    /// A plugin split the message (`compose-control.split`): send these, in
    /// order, instead of `text`, with the reply and attachments on the
    /// first. Empty: send `text`.
    pub parts: Vec<String>,
    /// Whether a reply pings its author (`None`: as the user set it).
    pub mention: Option<bool>,
    /// Send as @silent (`None`: as the user set it).
    pub silent: Option<bool>,
    /// A plugin cancelled the send: (plugin, reason). The client keeps the
    /// draft in the composer and shows the reason.
    pub cancelled: Option<(String, String)>,
    /// Plugins that changed it, in order. The client should show this to the
    /// user (e.g. "edited by Text Replace") so a rewrite is never silent.
    pub rewritten_by: Vec<String>,
    /// Rewrites the host refused, as (plugin, reason); the text before that
    /// plugin's rewrite was kept.
    pub rejected: Vec<(String, String)>,
}

/// One of the user's attachments after `on-upload` hooks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadOutcome {
    Keep,
    Replace {
        filename: String,
        content_type: Option<String>,
        #[serde(skip)]
        data: Vec<u8>,
        by: String,
    },
    Drop {
        plugin: String,
        reason: String,
    },
}

/// A file a plugin sends ([`Action::SendMessage`]). The bytes aren't
/// serialized, only their size, so a recorded action stays small.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutgoingFile {
    pub filename: String,
    pub content_type: Option<String>,
    #[serde(skip)]
    pub data: Vec<u8>,
    pub size: u64,
    pub spoiler: bool,
}

/// An action a plugin takes as the user, during a gesture. The runtime has
/// checked the capability, the gesture budget and its rate limits; the
/// backend checks Discord permissions and performs it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum Action {
    // local-actions
    MarkRead {
        channel: u64,
    },
    MarkGuildRead {
        guild: u64,
    },
    MarkAllRead,
    MarkUnread {
        message: crate::api::MessageRef,
    },
    StartReply {
        message: crate::api::MessageRef,
        mention: bool,
    },
    StartEdit {
        message: crate::api::MessageRef,
    },
    InsertText {
        text: String,
    },
    SetUserVolume {
        user: u64,
        percent: f32,
    },
    SetUserLocalMute {
        user: u64,
        muted: bool,
    },
    SetNote {
        user: u64,
        note: String,
    },
    SaveMessage {
        message: crate::api::MessageRef,
        saved: bool,
    },
    SetCategoryCollapsed {
        category: u64,
        collapsed: bool,
    },
    SetMuted {
        target: u64,
        muted: bool,
    },
    // reactions
    React {
        message: crate::api::MessageRef,
        emoji: String,
        burst: bool,
    },
    Unreact {
        message: crate::api::MessageRef,
        emoji: String,
        burst: bool,
    },
    // actions
    SetPinned {
        message: crate::api::MessageRef,
        pinned: bool,
    },
    /// `confirm`: show the client's own confirm first (unless Shift was held).
    DeleteMessage {
        message: crate::api::MessageRef,
        confirm: bool,
    },
    EditMessage {
        message: crate::api::MessageRef,
        content: String,
    },
    SuppressEmbeds {
        message: crate::api::MessageRef,
    },
    Vote {
        message: crate::api::MessageRef,
        answers: Vec<u32>,
    },
    JoinVoice {
        channel: Option<u64>,
    },
    SetSelfMute {
        muted: bool,
    },
    SetSelfDeaf {
        deafened: bool,
    },
    SetThreadJoined {
        thread: u64,
        joined: bool,
    },
    /// A message as the user, with files the plugin made.
    SendMessage {
        channel: u64,
        content: String,
        files: Vec<OutgoingFile>,
    },
    // presence
    SetStatus {
        status: String,
    },
    SetCustomStatus {
        text: Option<String>,
        emoji: Option<String>,
        expires_ms: Option<u64>,
    },
    SetNickname {
        guild: u64,
        nick: Option<String>,
    },
    SetRichPresence {
        activity: Option<crate::api::RichPresence>,
    },
    // navigation (ui)
    Navigate {
        to: crate::api::Target,
    },
    OpenUrl {
        url: String,
    },
    OpenMedia {
        url: String,
    },
}

/// Why an action failed (mirrors `action-types.action-error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActionError {
    #[error("not inside a user gesture, or its budget is spent")]
    NoGesture,
    #[error("missing a Discord permission")]
    Forbidden,
    #[error("target not loaded")]
    NotFound,
    #[error("rate limited")]
    RateLimited,
    #[error("unavailable on this client build")]
    Unavailable,
    #[error("the user declined")]
    Declined,
    #[error("invalid argument")]
    Invalid,
    #[error("Discord refused it")]
    Failed,
}
