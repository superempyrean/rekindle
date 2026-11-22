//! Keyed decorations (requires `ui`): small host-drawn badges and text on a
//! user, channel, guild or message. The plugin pushes them ahead of time, so
//! the host never calls into it while it paints a list.
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! # fn f(user_id: u64) -> Result<(), decorations::UiError> {
//! let badge = Decoration::icon(Icon::Shield).tooltip("Moderator");
//! decorations::set(decorations::NAME_BADGE, user_id, Some(badge))?;
//! # Ok(())
//! # }
//! ```
//!
//! Each kind's key is a user id, a channel id, a guild id or a message id,
//! as its constant says. A plugin holds up to 5000 decorations. Clicks on a
//! [`clickable`](Decoration::clickable) one arrive as
//! `SlotEvent::DecorationClicked((kind, key))`.

use crate::raw::rekindle::plugin::ui as raw;
use crate::ui::Element;
use crate::{Color, Icon, MessageRef, Span, UserRef};
pub use raw::{DmSection, UiError};

/// After a user's name, in messages and the member list. Keyed by user.
pub const NAME_BADGE: &str = "user.name-badge";
/// In the badge row of a user's profile. Keyed by user.
pub const PROFILE_BADGE: &str = "user.profile-badge";
/// A small icon on the corner of a user's avatar. Keyed by user.
pub const AVATAR_OVERLAY: &str = "user.avatar-overlay";
/// At the end of a member-list row. Keyed by user.
pub const MEMBER_LIST_TRAILING: &str = "member-list.trailing";
/// The second line of a member-list row. Keyed by user.
pub const MEMBER_LIST_SUBTEXT: &str = "member-list.subtext";
/// At the end of a DM-list row. Keyed by user.
pub const DM_LIST_TRAILING: &str = "dm-list.trailing";
/// At the end of a channel-list row. Keyed by channel.
pub const CHANNEL_LIST_TRAILING: &str = "channel-list.trailing";
/// The second line of a channel-list row. Keyed by channel.
pub const CHANNEL_LIST_SUBTITLE: &str = "channel-list.subtitle";
/// A corner badge on a server icon. Keyed by guild.
pub const GUILD_LIST_OVERLAY: &str = "guild-list.overlay";
/// After a message's timestamp. Keyed by message.
pub const MESSAGE_TRAILING: &str = "message.trailing";

/// A decoration, built up: `Decoration::text("3").color(c).tooltip("3 typing")`.
#[derive(Debug, Clone)]
pub struct Decoration(pub raw::Decoration);

impl Decoration {
    /// A decoration showing one of the client's icons.
    pub fn icon(icon: Icon) -> Self {
        Self(raw::Decoration {
            icon: Some(icon),
            ..Self::empty()
        })
    }

    /// A decoration showing text.
    pub fn text(text: &str) -> Self {
        Self(raw::Decoration {
            text: Some(text.into()),
            ..Self::empty()
        })
    }

    /// A decoration showing one of the plugin's images, registered with
    /// [`ui::register_image`](crate::ui::register_image).
    pub fn image(id: &str) -> Self {
        Self(raw::Decoration {
            image: Some(id.into()),
            ..Self::empty()
        })
    }

    /// Adds text beside the icon or image.
    pub fn with_text(mut self, text: &str) -> Self {
        self.0.text = Some(text.into());
        self
    }

    /// Sets the text shown on hover.
    pub fn tooltip(mut self, tooltip: &str) -> Self {
        self.0.tooltip = Some(tooltip.into());
        self
    }

    /// Sets the colour of the icon or text.
    pub fn color(mut self, c: Color) -> Self {
        self.0.color = Some(c);
        self
    }

    /// Makes clicks on it reach [`Plugin::on_slot`](crate::Plugin::on_slot).
    pub fn clickable(mut self) -> Self {
        self.0.clickable = true;
        self
    }

    fn empty() -> raw::Decoration {
        raw::Decoration {
            icon: None,
            image: None,
            text: None,
            tooltip: None,
            color: None,
            clickable: false,
        }
    }
}

/// The decoration kinds this client build draws.
pub fn available() -> Vec<String> {
    raw::available_decorations()
}

/// Sets one decoration of `kind` on `key`; `None` removes it.
pub fn set(kind: &str, key: u64, d: Option<Decoration>) -> Result<(), UiError> {
    raw::set_decoration(kind, key, d.map(|d| d.0).as_ref())
}

/// Replaces all of the plugin's decorations of `kind` at once.
pub fn set_all(kind: &str, items: Vec<(u64, Decoration)>) -> Result<(), UiError> {
    let items: Vec<(u64, raw::Decoration)> = items.into_iter().map(|(k, d)| (k, d.0)).collect();
    raw::set_decorations(kind, &items)
}

/// Removes all of the plugin's decorations of `kind`.
pub fn clear(kind: &str) {
    raw::clear_decorations(kind);
}

/// Elements under a message, such as a translation or an edit history;
/// `None` removes them. Clicks on their buttons arrive in
/// [`Plugin::on_ui`](crate::Plugin::on_ui) with the surface
/// `message:<id>`.
pub fn set_accessory(message: &MessageRef, elements: Option<&[Element]>) -> Result<(), UiError> {
    raw::set_message_accessory(*message, elements)
}

/// A row the plugin draws where a deleted or edited message was, styled as
/// the client's own system row and labelled with the plugin's name. Needs
/// `read-messages`. `style` is `"deleted"`, `"edited"` or `"note"`.
pub fn set_ghost(
    channel: u64,
    id: u64,
    author: UserRef,
    content: Vec<Span>,
    style: &str,
) -> Result<(), UiError> {
    raw::set_ghost_message(&raw::GhostMessage {
        channel,
        id,
        author,
        content,
        style: style.into(),
    })
}

/// Removes a row added with [`set_ghost`].
pub fn remove_ghost(channel: u64, id: u64) {
    raw::remove_ghost_message(channel, id);
}

/// Replaces the plugin's pinned sections at the top of the DM list.
pub fn set_dm_sections(sections: &[DmSection]) -> Result<(), UiError> {
    raw::set_dm_sections(sections)
}
