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
    ConnectionState, DeletedMessage, Event, EventKind, GuildLeftReason, ReactionChange,
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
    ChannelRef, Color as WitColor, GuildRef, MediaRef, Member, MessageRef, Modifiers, RoleRef,
    Snowflake, Span, SpanColor, Target, UserRef,
};
