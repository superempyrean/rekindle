//! Slots: items in the client's own UI (requires `ui`).
//!
//! The host defines a fixed set of named places, and the plugin fills them
//! with small, typed items that the host renders natively: menu items in
//! menus, icon buttons in bars, list items in lists. Clicks arrive in
//! [`Plugin::on_slot`](crate::Plugin::on_slot) as `SlotEvent::Clicked`, with
//! the slot, the item's id, the target (a message, user, channel, ...) and
//! the modifier keys held.
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! # struct Bookmarks;
//! # impl Plugin for Bookmarks {
//! fn activate(_: &Granted) -> Result<Self, String> {
//!     let item = MenuItem::new("bookmark", "Bookmark").icon(Icon::Bookmark);
//!     slots::add(slots::MESSAGE_MENU, item).map_err(|e| format!("{e:?}"))?;
//!     Ok(Bookmarks)
//! }
//!
//! fn on_slot(&mut self, ev: SlotEvent) {
//!     if let SlotEvent::Clicked(SlotClick { item, target: Target::Message(m), .. }) = ev
//!         && item == "bookmark"
//!     {
//!         info!("bookmarked {} in {}", m.id, m.channel);
//!     }
//! }
//! # }
//! ```
//!
//! A slot can be missing on a client build, when the host couldn't find
//! its hook point. Adding to it then fails with [`UiError::Unavailable`], so
//! check [`available`] and carry on without it.

use crate::raw::rekindle::plugin::ui as raw;
use crate::ui::Element;
use crate::{Icon, Modifiers};
pub use raw::{ClickArea, MenuPredicate, UiError};

// Menus, which take a `MenuItem` through `add`.

/// The right-click menu of a message. Target: the message.
pub const MESSAGE_MENU: &str = "message.context-menu";
/// The right-click menu of a user, member or DM user. Target: the user.
pub const USER_MENU: &str = "user.context-menu";
/// The right-click menu of a channel or DM row. Target: the channel.
pub const CHANNEL_MENU: &str = "channel.context-menu";
/// The right-click menu of a thread. Target: the thread's channel.
pub const THREAD_MENU: &str = "thread.context-menu";
/// The server icon's menu and the server header's popout. Target: the guild.
pub const GUILD_MENU: &str = "guild.context-menu";
/// The right-click menu of a role pill. Target: the role.
pub const ROLE_MENU: &str = "role.context-menu";
/// The right-click menu of an image, video or avatar. Target: the media.
pub const MEDIA_MENU: &str = "media.context-menu";
/// The full-size image and video viewer's "More" menu. Target: the media.
pub const MEDIA_VIEWER_MENU: &str = "media-viewer.menu";
/// The right-click menu of a link. Target: the link.
pub const LINK_MENU: &str = "link.context-menu";
/// The menu on selected text in the composer. Target: the selection.
pub const SELECTION_MENU: &str = "selection.context-menu";
/// The composer's "+" upload menu. Target: the channel.
pub const ATTACH_MENU: &str = "composer.attach-menu";
/// The settings cog's right-click menu. Target: none.
pub const SETTINGS_COG_MENU: &str = "settings.cog-menu";

// Bars, which take an icon button through `button`.

/// A message's hover action bar. Target: the message.
pub const MESSAGE_HOVER_BAR: &str = "message.hover-bar";
/// Beside the composer's emoji and GIF buttons. Target: the channel.
pub const CHAT_BAR: &str = "chat-bar.button";
/// The channel header, beside threads, pins and members. Target: the
/// channel.
pub const CHANNEL_TOOLBAR: &str = "channel.toolbar";
/// The user area's mute, deafen and settings row. Target: none.
pub const USER_AREA: &str = "user-area.button";
/// The voice connection panel. Target: the voice channel.
pub const VOICE_PANEL: &str = "voice-panel.button";
/// The top of the server list. Target: none.
pub const GUILD_LIST: &str = "guild-list.button";
/// Above the DM list. Target: none.
pub const DM_LIST: &str = "dm-list.button";

// Lists, each filled through its own function.

/// A line of text in the voice panel, set with [`set_voice_status`].
pub const VOICE_PANEL_STATUS: &str = "voice-panel.status";
/// A section in user profiles, added with [`profile_section`].
pub const PROFILE_SECTION: &str = "profile.section";
/// A tab in the client's emoji picker, beside Emoji, GIFs and Stickers,
/// added with [`picker_tab`].
pub const EXPRESSION_PICKER_TAB: &str = "expression-picker.tab";

/// No modifier keys.
pub const NO_MODS: Modifiers = Modifiers {
    shift: false,
    ctrl: false,
    alt: false,
    meta: false,
};

/// The slot names that work on this client build.
pub fn available() -> Vec<String> {
    raw::available_slots()
}

/// Whether `slot` works on this client build.
pub fn is_available(slot: &str) -> bool {
    available().iter().any(|s| s == slot)
}

/// A menu item, built up: `MenuItem::new("id", "Label").icon(Icon::Copy).group("copy")`.
#[derive(Debug, Clone)]
pub struct MenuItem(raw::MenuItem);

impl MenuItem {
    /// An item with a label and nothing else, shown on every target.
    pub fn new(id: &str, label: &str) -> Self {
        Self(raw::MenuItem {
            id: id.into(),
            label: label.into(),
            icon: None,
            danger: false,
            group: None,
            checked: None,
            when: Vec::new(),
        })
    }

    /// Shows one of the client's icons.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.0.icon = Some(icon);
        self
    }

    /// Uses the destructive style.
    pub fn danger(mut self) -> Self {
        self.0.danger = true;
        self
    }

    /// Joins one of the menu's groups (`"copy"`, `"reply"`, `"moderation"`,
    /// `"developer"`, ...) in place of the plugin group at the end.
    pub fn group(mut self, group: &str) -> Self {
        self.0.group = Some(group.into());
        self
    }

    /// Shows a check mark, for a toggle.
    pub fn checked(mut self, on: bool) -> Self {
        self.0.checked = Some(on);
        self
    }

    /// Shows the item only when `p` holds; several calls require them all.
    /// The host evaluates them, so a menu opens without calling the plugin.
    pub fn when(mut self, p: MenuPredicate) -> Self {
        self.0.when.push(p);
        self
    }
}

/// Adds a menu item to a menu slot, or replaces the item with its id.
pub fn add(slot: &str, item: MenuItem) -> Result<(), UiError> {
    raw::add_item(slot, &raw::SlotItem::MenuItem(item.0))
}

/// Adds an icon button to a bar slot ([`CHAT_BAR`], [`MESSAGE_HOVER_BAR`],
/// [`CHANNEL_TOOLBAR`], ...), or replaces the button with its id. `active`
/// highlights it, for a toggle that's on.
pub fn button(
    slot: &str,
    id: &str,
    icon: Icon,
    tooltip: &str,
    active: bool,
) -> Result<(), UiError> {
    add_button(slot, id, icon, None, tooltip, active)
}

/// [`button`], showing one of the plugin's registered images in place of an
/// icon.
pub fn image_button(
    slot: &str,
    id: &str,
    image: &str,
    tooltip: &str,
    active: bool,
) -> Result<(), UiError> {
    add_button(slot, id, Icon::Puzzle, Some(image), tooltip, active)
}

/// Sets a button's `active` state without adding it again.
pub fn set_active(slot: &str, id: &str, active: bool) -> Result<(), UiError> {
    raw::set_active(slot, id, active)
}

/// Adds a section to user profiles. Fill it for each user with
/// [`set_profile_section`] when `SlotEvent::ProfileOpened` arrives.
pub fn profile_section(id: &str, title: &str) -> Result<(), UiError> {
    raw::add_item(
        PROFILE_SECTION,
        &raw::SlotItem::ListItem(raw::ListItem {
            id: id.into(),
            title: title.into(),
        }),
    )
}

/// Adds a tab to the client's emoji picker. The tab's body is the plugin's
/// panel with the same id: keep it current with `ui::show_panel`. A press
/// on the tab arrives as `SlotEvent::Clicked` on [`EXPRESSION_PICKER_TAB`],
/// and presses in the body arrive through `on_ui` with that panel's id as
/// the surface.
pub fn picker_tab(id: &str, title: &str) -> Result<(), UiError> {
    raw::add_item(
        EXPRESSION_PICKER_TAB,
        &raw::SlotItem::ListItem(raw::ListItem {
            id: id.into(),
            title: title.into(),
        }),
    )
}

/// The content of the plugin's profile section for one user.
pub fn set_profile_section(section_id: &str, user: u64, elements: &[Element]) {
    raw::set_profile_section(section_id, user, elements);
}

/// A line in the voice panel ("Call 1:02:33"). With `since_ms`, the host
/// shows the live elapsed time after `text`, so the plugin needs no timer.
/// An empty `text` removes the line.
pub fn set_voice_status(id: &str, text: &str, since_ms: Option<u64>) -> Result<(), UiError> {
    raw::set_voice_status(id, text, since_ms)
}

/// Removes an item the plugin added to `slot`.
pub fn remove(slot: &str, id: &str) {
    raw::remove_item(slot, id);
}

/// Binds a click on a message: `clicks` (1 or 2; a single click needs a
/// modifier) on `area` with `modifiers` held, on messages where every
/// predicate in `when` holds. A click fires `SlotEvent::MessageClicked((id,
/// message))`, which is a gesture. If two plugins claim the same click, the
/// user picks the owner in rekindle's settings.
pub fn bind_click(
    id: &str,
    label: &str,
    area: ClickArea,
    clicks: u8,
    modifiers: Modifiers,
    when: &[MenuPredicate],
) -> Result<(), UiError> {
    raw::register_click(&raw::ClickBinding {
        id: id.into(),
        label: label.into(),
        area,
        clicks,
        modifiers,
        when: when.to_vec(),
    })
}

/// Removes a click binding.
pub fn unbind_click(id: &str) {
    raw::unregister_click(id);
}

/// The modifier keys `shift`, `ctrl` and `alt`, without meta.
pub fn mods(shift: bool, ctrl: bool, alt: bool) -> Modifiers {
    Modifiers {
        shift,
        ctrl,
        alt,
        meta: false,
    }
}

/// `image` replaces `icon` when it's set; the WIT record needs an icon
/// either way.
fn add_button(
    slot: &str,
    id: &str,
    icon: Icon,
    image: Option<&str>,
    tooltip: &str,
    active: bool,
) -> Result<(), UiError> {
    raw::add_item(
        slot,
        &raw::SlotItem::IconButton(raw::IconButton {
            id: id.into(),
            icon,
            image: image.map(Into::into),
            tooltip: tooltip.into(),
            active,
        }),
    )
}
