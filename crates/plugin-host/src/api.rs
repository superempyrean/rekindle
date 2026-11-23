//! The contract's own records, for backends.
//!
//! Client state, events, compose drafts, slash commands, render inputs and
//! transforms cross the plugin boundary unchanged, so backends build and
//! read the generated types directly (re-exported here, with serde) instead
//! of a hand-written mirror that could drift. UI contributions and theming
//! keep host-side types in [`crate::model`], which clients render.

pub use crate::bindings::wit::{
    action_types, actions, audio, command_types, compose_types, events, files, host, local_actions,
    notification_types, notify, presence, records, render_types, theming, tweaks, types, ui,
};

pub use crate::bindings::ex::ui_handler::{SlotClick, SlotEvent};
pub use crate::bindings::wit::command_types::{
    CommandOption, CommandReply, Invocation, OptionKind, OptionValue, SlashCommand,
};
pub use crate::bindings::wit::compose_types::{
    AttachmentInfo, ComposeKind, ComposeResult, Draft, ReplyInfo, Rewrite, Upload, UploadResult,
};
pub use crate::bindings::wit::events::{
    ConnectionState, DeletedMessage, Event, EventKind, EventPatch, GuildLeftReason, ReactionChange,
    RelationshipChange, SelectionChange,
};
pub use crate::bindings::wit::host::{ClientInfo, TimeStyle};
pub use crate::bindings::wit::notification_types::{
    ClientNotification, NotificationDecision, NotificationKind,
};
pub use crate::bindings::wit::presence::RichPresence;
pub use crate::bindings::wit::records::*;
pub use crate::bindings::wit::render_types::{Badge, Highlight, RenderInput, Transform};
pub use crate::bindings::wit::types::{
    AvatarDecoration, ChannelRef, Color as WitColor, DisplayNameStyle, GuildRef, MediaRef, Member,
    MessageRef, Modifiers, Nameplate, PrimaryGuild, RoleRef, Snowflake, Span, SpanColor, Target,
    UserRef,
};
pub use crate::bindings::wit::view::{MemberField, UserField, Users};

// Defaults for the records the host builds field by field, so a new field
// in the contract is empty until a backend fills it.
#[expect(
    clippy::derivable_impls,
    clippy::default_trait_access,
    reason = "bindgen generates these types, so they can't take a derive; the field list is the contract's"
)]
mod defaults {
    use super::{Channel, Guild, Member, Message, Role, User, UserRef};

    impl Default for User {
        fn default() -> Self {
            Self {
                id: Default::default(),
                username: Default::default(),
                discriminator: Default::default(),
                global_name: Default::default(),
                bot: Default::default(),
                system: Default::default(),
                public_flags: Default::default(),
                flag_bits: Default::default(),
                premium_type: Default::default(),
                avatar: Default::default(),
                banner: Default::default(),
                accent: Default::default(),
                has_decoration: Default::default(),
                avatar_decoration: Default::default(),
                nameplate: Default::default(),
                display_name_style: Default::default(),
                primary_guild: Default::default(),
                bio: Default::default(),
                pronouns: Default::default(),
                theme_colors: Default::default(),
                profile_effect: Default::default(),
                locale: Default::default(),
                verified: Default::default(),
                mfa_enabled: Default::default(),
                email: Default::default(),
                phone: Default::default(),
                nsfw_allowed: Default::default(),
                age_verification_status: Default::default(),
                purchased_flags: Default::default(),
                authenticator_types: Default::default(),
                store_country: Default::default(),
            }
        }
    }

    impl Default for Channel {
        fn default() -> Self {
            Self {
                id: Default::default(),
                kind: Default::default(),
                guild: Default::default(),
                parent: Default::default(),
                name: Default::default(),
                topic: Default::default(),
                position: Default::default(),
                nsfw: Default::default(),
                slowmode: Default::default(),
                last_message: Default::default(),
                recipients: Default::default(),
                owner: Default::default(),
                icon: Default::default(),
                bitrate: Default::default(),
                user_limit: Default::default(),
                rtc_region: Default::default(),
                video_quality: Default::default(),
                flag_bits: Default::default(),
                default_auto_archive_mins: Default::default(),
                default_thread_slowmode: Default::default(),
                default_reaction: Default::default(),
                default_sort: Default::default(),
                default_layout: Default::default(),
                last_pin_ms: Default::default(),
                message_request: Default::default(),
                spam: Default::default(),
                thread: Default::default(),
                forum_tags: Default::default(),
                muted: Default::default(),
                visible: Default::default(),
            }
        }
    }

    impl Default for Guild {
        fn default() -> Self {
            Self {
                id: Default::default(),
                name: Default::default(),
                owner: Default::default(),
                description: Default::default(),
                features: Default::default(),
                premium_tier: Default::default(),
                premium_subscribers: Default::default(),
                verification_level: Default::default(),
                nsfw_level: Default::default(),
                preferred_locale: Default::default(),
                icon: Default::default(),
                banner: Default::default(),
                splash: Default::default(),
                discovery_splash: Default::default(),
                vanity_url_code: Default::default(),
                afk_channel: Default::default(),
                afk_timeout_secs: Default::default(),
                system_channel: Default::default(),
                rules_channel: Default::default(),
                public_updates_channel: Default::default(),
                safety_alerts_channel: Default::default(),
                system_channel_flags: Default::default(),
                mfa_level: Default::default(),
                explicit_content_filter: Default::default(),
                default_message_notifications: Default::default(),
                premium_progress_bar: Default::default(),
                max_members: Default::default(),
                max_video_channel_users: Default::default(),
                application: Default::default(),
                large: Default::default(),
                unavailable: Default::default(),
                member_count: Default::default(),
                joined_ms: Default::default(),
                muted: Default::default(),
            }
        }
    }

    impl Default for Role {
        fn default() -> Self {
            Self {
                id: Default::default(),
                guild: Default::default(),
                name: Default::default(),
                color: Default::default(),
                colors_extra: Default::default(),
                position: Default::default(),
                permissions: Default::default(),
                hoist: Default::default(),
                mentionable: Default::default(),
                managed: Default::default(),
                unicode_emoji: Default::default(),
                icon: Default::default(),
                flag_bits: Default::default(),
                bot: Default::default(),
                integration: Default::default(),
                premium_subscriber: Default::default(),
                subscription_listing: Default::default(),
                available_for_purchase: Default::default(),
                guild_connections: Default::default(),
            }
        }
    }

    impl Default for Member {
        fn default() -> Self {
            Self {
                guild: Default::default(),
                user: Default::default(),
                nick: Default::default(),
                roles: Default::default(),
                joined_ms: Default::default(),
                boosting_since_ms: Default::default(),
                timed_out_until_ms: Default::default(),
                pending: Default::default(),
                avatar: Default::default(),
                banner: Default::default(),
                bio: Default::default(),
                avatar_decoration: Default::default(),
                flag_bits: Default::default(),
                mute: Default::default(),
                deaf: Default::default(),
                unusual_dm_activity_until_ms: Default::default(),
            }
        }
    }

    impl Default for Message {
        fn default() -> Self {
            Self {
                id: Default::default(),
                channel: Default::default(),
                guild: Default::default(),
                author: Default::default(),
                bot: Default::default(),
                webhook: Default::default(),
                kind: Default::default(),
                flag_bits: Default::default(),
                content: Default::default(),
                edited_ms: Default::default(),
                pinned: Default::default(),
                tts: Default::default(),
                mentions_everyone: Default::default(),
                mentions: Default::default(),
                mention_roles: Default::default(),
                reply_to: Default::default(),
                attachments: Default::default(),
                embeds: Default::default(),
                reactions: Default::default(),
                stickers: Default::default(),
                poll: Default::default(),
                thread: Default::default(),
                forwarded: Default::default(),
                nonce: Default::default(),
                webhook_id: Default::default(),
                application_id: Default::default(),
                interaction: Default::default(),
                mention_channels: Default::default(),
                position: Default::default(),
                call: Default::default(),
                components_json: Default::default(),
                reply_deleted: Default::default(),
                role_subscription: Default::default(),
                ephemeral: Default::default(),
                sending: Default::default(),
                failed: Default::default(),
            }
        }
    }

    impl Default for UserRef {
        fn default() -> Self {
            Self {
                id: Default::default(),
                name: Default::default(),
            }
        }
    }
}
