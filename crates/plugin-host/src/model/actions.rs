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
    ForwardMessage {
        message: crate::api::MessageRef,
        to_channel: u64,
    },
    SetTyping {
        channel: u64,
    },
    SetFriend {
        user: u64,
        add: bool,
    },
    /// Returns [`ActionValue::Code`].
    CreateInvite {
        channel: u64,
        max_age_secs: u32,
        max_uses: u32,
    },
    /// Returns [`ActionValue::Id`]: the guild.
    JoinGuild {
        invite_code: String,
    },
    /// Returns [`ActionValue::Id`]: the DM channel.
    OpenDm {
        user: u64,
    },
    // moderation
    Kick {
        guild: u64,
        user: u64,
        reason: Option<String>,
    },
    Ban {
        guild: u64,
        user: u64,
        delete_message_secs: u32,
        reason: Option<String>,
    },
    Unban {
        guild: u64,
        user: u64,
    },
    Timeout {
        guild: u64,
        user: u64,
        until_ms: Option<u64>,
        reason: Option<String>,
    },
    SetVoiceMute {
        guild: u64,
        user: u64,
        muted: bool,
    },
    SetVoiceDeaf {
        guild: u64,
        user: u64,
        deafened: bool,
    },
    MoveMember {
        guild: u64,
        user: u64,
        channel: Option<u64>,
    },
    BulkDelete {
        channel: u64,
        messages: Vec<u64>,
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

impl Action {
    /// Its kind, as its `action` tag serializes: `react`, `send-message`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::MarkRead { .. } => "mark-read",
            Self::MarkGuildRead { .. } => "mark-guild-read",
            Self::MarkAllRead { .. } => "mark-all-read",
            Self::MarkUnread { .. } => "mark-unread",
            Self::StartReply { .. } => "start-reply",
            Self::StartEdit { .. } => "start-edit",
            Self::InsertText { .. } => "insert-text",
            Self::SetUserVolume { .. } => "set-user-volume",
            Self::SetUserLocalMute { .. } => "set-user-local-mute",
            Self::SetNote { .. } => "set-note",
            Self::SaveMessage { .. } => "save-message",
            Self::SetCategoryCollapsed { .. } => "set-category-collapsed",
            Self::SetMuted { .. } => "set-muted",
            Self::React { .. } => "react",
            Self::Unreact { .. } => "unreact",
            Self::SetPinned { .. } => "set-pinned",
            Self::DeleteMessage { .. } => "delete-message",
            Self::EditMessage { .. } => "edit-message",
            Self::SuppressEmbeds { .. } => "suppress-embeds",
            Self::Vote { .. } => "vote",
            Self::JoinVoice { .. } => "join-voice",
            Self::SetSelfMute { .. } => "set-self-mute",
            Self::SetSelfDeaf { .. } => "set-self-deaf",
            Self::SetThreadJoined { .. } => "set-thread-joined",
            Self::SendMessage { .. } => "send-message",
            Self::ForwardMessage { .. } => "forward-message",
            Self::SetTyping { .. } => "set-typing",
            Self::SetFriend { .. } => "set-friend",
            Self::CreateInvite { .. } => "create-invite",
            Self::JoinGuild { .. } => "join-guild",
            Self::OpenDm { .. } => "open-dm",
            Self::Kick { .. } => "kick",
            Self::Ban { .. } => "ban",
            Self::Unban { .. } => "unban",
            Self::Timeout { .. } => "timeout",
            Self::SetVoiceMute { .. } => "set-voice-mute",
            Self::SetVoiceDeaf { .. } => "set-voice-deaf",
            Self::MoveMember { .. } => "move-member",
            Self::BulkDelete { .. } => "bulk-delete",
            Self::SetStatus { .. } => "set-status",
            Self::SetCustomStatus { .. } => "set-custom-status",
            Self::SetNickname { .. } => "set-nickname",
            Self::SetRichPresence { .. } => "set-rich-presence",
            Self::Navigate { .. } => "navigate",
            Self::OpenUrl { .. } => "open-url",
            Self::OpenMedia { .. } => "open-media",
        }
    }
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

/// What an action that returns something returned
/// ([`ClientBackend::perform_value`](crate::ClientBackend::perform_value)).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActionValue {
    None,
    Id(u64),
    Code(String),
}

/// One plugin's `view` overrides: fields of users and members, and setting
/// keys, in the order it set them. The backend lays them over what it reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewOverrides {
    pub users: Vec<(crate::api::Users, crate::api::UserField)>,
    /// (guild, or every guild; who; field).
    pub members: Vec<(Option<u64>, crate::api::Users, crate::api::MemberField)>,
    /// Setting key to its string form.
    pub settings: std::collections::BTreeMap<String, String>,
}

/// Whether `who` names user `id`, the current user being `me`.
pub fn names(who: &crate::api::Users, me: u64, id: u64) -> bool {
    use crate::api::Users as U;
    match who {
        U::Me => id == me,
        U::Others => id != me,
        U::Everyone => true,
        U::One(u) => *u == id,
    }
}

impl ViewOverrides {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Set `field` for `who`, replacing this plugin's earlier override of
    /// the same field for the same `who`.
    pub fn set_user(&mut self, who: crate::api::Users, field: crate::api::UserField) {
        let kind = std::mem::discriminant(&field);
        self.users
            .retain(|(w, f)| !(*w == who && std::mem::discriminant(f) == kind));
        self.users.push((who, field));
    }

    pub fn set_member(
        &mut self,
        guild: Option<u64>,
        who: crate::api::Users,
        field: crate::api::MemberField,
    ) {
        let kind = std::mem::discriminant(&field);
        self.members
            .retain(|(g, w, f)| !(*g == guild && *w == who && std::mem::discriminant(f) == kind));
        self.members.push((guild, who, field));
    }

    /// Apply the user overrides that name `user`.
    pub fn apply_user(&self, me: u64, user: &mut crate::api::User) {
        for (who, f) in &self.users {
            if names(who, me, user.id) {
                apply_user_field(user, f);
            }
        }
    }

    /// Apply the member overrides that name `member`.
    pub fn apply_member(&self, me: u64, member: &mut crate::api::Member) {
        for (guild, who, f) in &self.members {
            if guild.is_none_or(|g| g == member.guild) && names(who, me, member.user) {
                apply_member_field(member, f);
            }
        }
    }
}

fn apply_user_field(u: &mut crate::api::User, f: &crate::api::UserField) {
    use crate::api::UserField as F;
    match f.clone() {
        F::Username(v) => u.username = v,
        F::Discriminator(v) => u.discriminator = v,
        F::GlobalName(v) => u.global_name = v,
        F::Bot(v) => u.bot = v,
        F::PublicFlags(v) => u.public_flags = v,
        F::FlagBits(v) => u.flag_bits = v,
        F::PremiumType(v) => u.premium_type = v,
        F::Avatar(v) => u.avatar = v,
        F::Banner(v) => u.banner = v,
        F::Accent(v) => u.accent = v,
        F::AvatarDecoration(v) => {
            u.avatar_decoration = v;
            u.has_decoration = u.avatar_decoration.is_some() || u.nameplate.is_some();
        }
        F::Nameplate(v) => {
            u.nameplate = v;
            u.has_decoration = u.avatar_decoration.is_some() || u.nameplate.is_some();
        }
        F::DisplayNameStyle(v) => u.display_name_style = v,
        F::PrimaryGuild(v) => u.primary_guild = v,
        F::Bio(v) => u.bio = v,
        F::Pronouns(v) => u.pronouns = v,
        F::ThemeColors(v) => u.theme_colors = v,
        F::ProfileEffect(v) => u.profile_effect = v,
        F::Locale(v) => u.locale = v,
        F::Verified(v) => u.verified = v,
        F::MfaEnabled(v) => u.mfa_enabled = v,
        F::NsfwAllowed(v) => u.nsfw_allowed = v,
        F::AgeVerificationStatus(v) => u.age_verification_status = v,
    }
}

fn apply_member_field(m: &mut crate::api::Member, f: &crate::api::MemberField) {
    use crate::api::MemberField as F;
    match f.clone() {
        F::Nick(v) => m.nick = v,
        F::Roles(v) => m.roles = v,
        F::Avatar(v) => m.avatar = v,
        F::Banner(v) => m.banner = v,
        F::Bio(v) => m.bio = v,
        F::AvatarDecoration(v) => m.avatar_decoration = v,
        F::FlagBits(v) => m.flag_bits = v,
        F::TimedOutUntilMs(v) => m.timed_out_until_ms = v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_is_the_serialized_tag() {
        let all = [
            Action::MarkAllRead,
            Action::SetTyping { channel: 1 },
            Action::OpenUrl {
                url: "https://example.com".into(),
            },
            Action::SetSelfDeaf { deafened: true },
        ];
        for a in all {
            let v = serde_json::to_value(&a).unwrap();
            assert_eq!(v["action"], a.kind());
        }
    }
}
