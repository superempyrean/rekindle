//! Discord's own numbering, which the contract passes through as integers.
//! Discord adds values often, and unknown ones pass through untouched.

/// Channel types (`channel.kind`).
#[expect(missing_docs, reason = "Discord's names for its channel types")]
pub mod channel_kind {
    pub const TEXT: u16 = 0;
    pub const DM: u16 = 1;
    pub const VOICE: u16 = 2;
    pub const GROUP_DM: u16 = 3;
    pub const CATEGORY: u16 = 4;
    pub const ANNOUNCEMENT: u16 = 5;
    pub const ANNOUNCEMENT_THREAD: u16 = 10;
    pub const PUBLIC_THREAD: u16 = 11;
    pub const PRIVATE_THREAD: u16 = 12;
    pub const STAGE: u16 = 13;
    pub const DIRECTORY: u16 = 14;
    pub const FORUM: u16 = 15;
    pub const MEDIA: u16 = 16;

    /// Whether `kind` is one of the three thread types.
    pub fn is_thread(kind: u16) -> bool {
        matches!(kind, ANNOUNCEMENT_THREAD | PUBLIC_THREAD | PRIVATE_THREAD)
    }

    /// Whether `kind` is a DM or a group DM.
    pub fn is_private(kind: u16) -> bool {
        matches!(kind, DM | GROUP_DM)
    }
}

/// Message types (`message.kind`).
#[expect(missing_docs, reason = "Discord's names for its message types")]
pub mod message_kind {
    pub const DEFAULT: u16 = 0;
    pub const RECIPIENT_ADD: u16 = 1;
    pub const RECIPIENT_REMOVE: u16 = 2;
    pub const CALL: u16 = 3;
    pub const CHANNEL_PINNED_MESSAGE: u16 = 6;
    pub const USER_JOIN: u16 = 7;
    pub const GUILD_BOOST: u16 = 8;
    pub const THREAD_CREATED: u16 = 18;
    pub const REPLY: u16 = 19;
    pub const CHAT_INPUT_COMMAND: u16 = 20;
    pub const THREAD_STARTER_MESSAGE: u16 = 21;
    pub const CONTEXT_MENU_COMMAND: u16 = 23;
    pub const AUTO_MODERATION_ACTION: u16 = 24;
    pub const POLL_RESULT: u16 = 46;
}

/// Message flag bits (`message.flag-bits`).
#[expect(missing_docs, reason = "Discord's names for its message flags")]
pub mod message_flags {
    pub const CROSSPOSTED: u64 = 1 << 0;
    pub const IS_CROSSPOST: u64 = 1 << 1;
    pub const SUPPRESS_EMBEDS: u64 = 1 << 2;
    pub const URGENT: u64 = 1 << 4;
    pub const HAS_THREAD: u64 = 1 << 5;
    pub const EPHEMERAL: u64 = 1 << 6;
    pub const LOADING: u64 = 1 << 7;
    pub const SUPPRESS_NOTIFICATIONS: u64 = 1 << 12;
    pub const IS_VOICE_MESSAGE: u64 = 1 << 13;
    pub const HAS_SNAPSHOT: u64 = 1 << 14;
}

/// Permission bits (`role.permissions`, [`state::my_permissions`]), each
/// documented with its name in the client's role settings.
///
/// [`state::my_permissions`]: crate::state::my_permissions
pub mod perms {
    /// Each permission's bit and the name the client shows for it.
    macro_rules! permissions {
        ($($name:ident = $bit:literal, $label:literal;)*) => {
            $(
                #[doc = $label]
                pub const $name: u64 = 1 << $bit;
            )*

            const ALL: &[(u64, &str)] = &[$(($name, $label)),*];
        };
    }

    permissions! {
        CREATE_INSTANT_INVITE = 0, "Create Invite";
        KICK_MEMBERS = 1, "Kick Members";
        BAN_MEMBERS = 2, "Ban Members";
        ADMINISTRATOR = 3, "Administrator";
        MANAGE_CHANNELS = 4, "Manage Channels";
        MANAGE_GUILD = 5, "Manage Server";
        ADD_REACTIONS = 6, "Add Reactions";
        VIEW_AUDIT_LOG = 7, "View Audit Log";
        PRIORITY_SPEAKER = 8, "Priority Speaker";
        STREAM = 9, "Video";
        VIEW_CHANNEL = 10, "View Channel";
        SEND_MESSAGES = 11, "Send Messages";
        SEND_TTS_MESSAGES = 12, "Send TTS Messages";
        MANAGE_MESSAGES = 13, "Manage Messages";
        EMBED_LINKS = 14, "Embed Links";
        ATTACH_FILES = 15, "Attach Files";
        READ_MESSAGE_HISTORY = 16, "Read Message History";
        MENTION_EVERYONE = 17, "Mention Everyone";
        USE_EXTERNAL_EMOJIS = 18, "Use External Emoji";
        VIEW_GUILD_INSIGHTS = 19, "View Server Insights";
        CONNECT = 20, "Connect";
        SPEAK = 21, "Speak";
        MUTE_MEMBERS = 22, "Mute Members";
        DEAFEN_MEMBERS = 23, "Deafen Members";
        MOVE_MEMBERS = 24, "Move Members";
        USE_VAD = 25, "Use Voice Activity";
        CHANGE_NICKNAME = 26, "Change Nickname";
        MANAGE_NICKNAMES = 27, "Manage Nicknames";
        MANAGE_ROLES = 28, "Manage Roles";
        MANAGE_WEBHOOKS = 29, "Manage Webhooks";
        MANAGE_GUILD_EXPRESSIONS = 30, "Manage Expressions";
        USE_APPLICATION_COMMANDS = 31, "Use Application Commands";
        REQUEST_TO_SPEAK = 32, "Request to Speak";
        MANAGE_EVENTS = 33, "Manage Events";
        MANAGE_THREADS = 34, "Manage Threads";
        CREATE_PUBLIC_THREADS = 35, "Create Public Threads";
        CREATE_PRIVATE_THREADS = 36, "Create Private Threads";
        USE_EXTERNAL_STICKERS = 37, "Use External Stickers";
        SEND_MESSAGES_IN_THREADS = 38, "Send Messages in Threads";
        USE_EMBEDDED_ACTIVITIES = 39, "Use Activities";
        MODERATE_MEMBERS = 40, "Timeout Members";
        VIEW_CREATOR_MONETIZATION_ANALYTICS = 41, "View Monetization Analytics";
        USE_SOUNDBOARD = 42, "Use Soundboard";
        CREATE_GUILD_EXPRESSIONS = 43, "Create Expressions";
        CREATE_EVENTS = 44, "Create Events";
        USE_EXTERNAL_SOUNDS = 45, "Use External Sounds";
        SEND_VOICE_MESSAGES = 46, "Send Voice Messages";
        SEND_POLLS = 49, "Create Polls";
        USE_EXTERNAL_APPS = 50, "Use External Apps";
        PIN_MESSAGES = 51, "Pin Messages";
        BYPASS_SLOWMODE = 52, "Bypass Slowmode";
    }

    /// The client's names for the bits set in `bits`, lowest bit first.
    pub fn names(bits: u64) -> Vec<&'static str> {
        ALL.iter()
            .filter(|(b, _)| bits & b != 0)
            .map(|(_, n)| *n)
            .collect()
    }
}

/// Public user flag bits (`user.public-flags`), for badge plugins.
#[expect(missing_docs, reason = "Discord's names for its user flags")]
pub mod user_flags {
    pub const STAFF: u64 = 1 << 0;
    pub const PARTNER: u64 = 1 << 1;
    pub const HYPESQUAD: u64 = 1 << 2;
    pub const BUG_HUNTER_LEVEL_1: u64 = 1 << 3;
    pub const HYPESQUAD_BRAVERY: u64 = 1 << 6;
    pub const HYPESQUAD_BRILLIANCE: u64 = 1 << 7;
    pub const HYPESQUAD_BALANCE: u64 = 1 << 8;
    pub const PREMIUM_EARLY_SUPPORTER: u64 = 1 << 9;
    pub const BUG_HUNTER_LEVEL_2: u64 = 1 << 14;
    pub const VERIFIED_BOT: u64 = 1 << 16;
    pub const VERIFIED_DEVELOPER: u64 = 1 << 17;
    pub const CERTIFIED_MODERATOR: u64 = 1 << 18;
    pub const ACTIVE_DEVELOPER: u64 = 1 << 22;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_permission_bit_has_a_name() {
        assert_eq!(perms::names(perms::BYPASS_SLOWMODE), ["Bypass Slowmode"]);
        assert_eq!(
            perms::names(perms::KICK_MEMBERS | perms::PIN_MESSAGES),
            ["Kick Members", "Pin Messages"]
        );
        // Bits 47 and 48 aren't permissions.
        let named = (0..64).filter(|b| !perms::names(1 << b).is_empty()).count();
        assert_eq!(named, 51);
    }

    #[test]
    fn threads_and_private_channels() {
        assert!(channel_kind::is_thread(channel_kind::PUBLIC_THREAD));
        assert!(!channel_kind::is_thread(channel_kind::FORUM));
        assert!(channel_kind::is_private(channel_kind::GROUP_DM));
        assert!(!channel_kind::is_private(channel_kind::TEXT));
    }
}
