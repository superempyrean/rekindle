//! Slots: the places in the client's UI a plugin adds items to, and the
//! items.

use serde::{Deserialize, Serialize};

/// Slot names (`ui` in the WIT; design doc §8.2).
pub const SLOT_MESSAGE_MENU: &str = "message.context-menu";
pub const SLOT_MESSAGE_HOVER_BAR: &str = "message.hover-bar";
pub const SLOT_USER_MENU: &str = "user.context-menu";
pub const SLOT_CHANNEL_MENU: &str = "channel.context-menu";
pub const SLOT_THREAD_MENU: &str = "thread.context-menu";
pub const SLOT_GUILD_MENU: &str = "guild.context-menu";
pub const SLOT_ROLE_MENU: &str = "role.context-menu";
pub const SLOT_MEDIA_MENU: &str = "media.context-menu";
pub const SLOT_LINK_MENU: &str = "link.context-menu";
pub const SLOT_SELECTION_MENU: &str = "selection.context-menu";
pub const SLOT_ATTACH_MENU: &str = "composer.attach-menu";
pub const SLOT_SETTINGS_COG_MENU: &str = "settings.cog-menu";
pub const SLOT_CHAT_BAR_BUTTON: &str = "chat-bar.button";
pub const SLOT_CHANNEL_TOOLBAR: &str = "channel.toolbar";
pub const SLOT_USER_AREA_BUTTON: &str = "user-area.button";
pub const SLOT_VOICE_PANEL_BUTTON: &str = "voice-panel.button";
pub const SLOT_GUILD_LIST_BUTTON: &str = "guild-list.button";
pub const SLOT_DM_LIST_BUTTON: &str = "dm-list.button";
pub const SLOT_VOICE_PANEL_STATUS: &str = "voice-panel.status";
pub const SLOT_PROFILE_SECTION: &str = "profile.section";
pub const SLOT_MEDIA_VIEWER_MENU: &str = "media-viewer.menu";

/// What kind of item each slot takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SlotKind {
    Menu,
    Button,
    List,
}

/// Every slot, with its item kind.
pub const ALL_SLOTS: [(&str, SlotKind); 21] = [
    (SLOT_MESSAGE_MENU, SlotKind::Menu),
    (SLOT_MESSAGE_HOVER_BAR, SlotKind::Button),
    (SLOT_USER_MENU, SlotKind::Menu),
    (SLOT_CHANNEL_MENU, SlotKind::Menu),
    (SLOT_THREAD_MENU, SlotKind::Menu),
    (SLOT_GUILD_MENU, SlotKind::Menu),
    (SLOT_ROLE_MENU, SlotKind::Menu),
    (SLOT_MEDIA_MENU, SlotKind::Menu),
    (SLOT_LINK_MENU, SlotKind::Menu),
    (SLOT_SELECTION_MENU, SlotKind::Menu),
    (SLOT_ATTACH_MENU, SlotKind::Menu),
    (SLOT_SETTINGS_COG_MENU, SlotKind::Menu),
    (SLOT_CHAT_BAR_BUTTON, SlotKind::Button),
    (SLOT_CHANNEL_TOOLBAR, SlotKind::Button),
    (SLOT_USER_AREA_BUTTON, SlotKind::Button),
    (SLOT_VOICE_PANEL_BUTTON, SlotKind::Button),
    (SLOT_GUILD_LIST_BUTTON, SlotKind::Button),
    (SLOT_DM_LIST_BUTTON, SlotKind::Button),
    (SLOT_VOICE_PANEL_STATUS, SlotKind::List),
    (SLOT_PROFILE_SECTION, SlotKind::List),
    (SLOT_MEDIA_VIEWER_MENU, SlotKind::Menu),
];

/// The item kind a slot takes, if it's a known slot.
pub fn slot_kind(slot: &str) -> Option<SlotKind> {
    ALL_SLOTS.iter().find(|(s, _)| *s == slot).map(|(_, k)| *k)
}

/// Host-defined icons, drawn with the client's own icon set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Icon {
    Star,
    Bookmark,
    Note,
    Palette,
    Bolt,
    Heart,
    Bell,
    Gear,
    Link,
    Search,
    Sparkle,
    Eye,
    EyeOff,
    Clock,
    Pin,
    Puzzle,
    Translate,
    Microphone,
    Headphones,
    Camera,
    Shield,
    Lock,
    User,
    Users,
    Chat,
    Reply,
    Copy,
    Trash,
    Edit,
    Check,
    Cross,
    Info,
    Warning,
    Download,
    Upload,
    Globe,
    Code,
    Hash,
    At,
    Smile,
    Image,
    Music,
    Play,
    Pause,
}

/// Host-evaluated menu-item predicates (`ui.menu-predicate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MenuPredicate {
    OwnMessage,
    OthersMessage,
    HasAttachments,
    HasEmbeds,
    InGuild,
    InDm,
    TargetIsBot,
    TargetIsSelf,
    TargetIsFriend,
    HasContent,
    CanManageMessages,
    CanPin,
    CanEdit,
    CanReply,
    CanReact,
    CanDelete,
}

/// An item in a slot. Which variant a slot takes is [`slot_kind`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SlotItem {
    MenuItem {
        id: String,
        label: String,
        icon: Option<Icon>,
        danger: bool,
        /// rekindle's group name within the menu; `None` = the plugin group.
        group: Option<String>,
        /// A check mark (toggle items).
        checked: Option<bool>,
        when: Vec<MenuPredicate>,
    },
    IconButton {
        id: String,
        icon: Icon,
        /// A registered plugin image instead of the icon.
        image: Option<String>,
        tooltip: String,
        active: bool,
    },
    ListItem {
        id: String,
        title: String,
    },
}

impl SlotItem {
    pub fn id(&self) -> &str {
        match self {
            SlotItem::MenuItem { id, .. }
            | SlotItem::IconButton { id, .. }
            | SlotItem::ListItem { id, .. } => id,
        }
    }

    pub fn kind(&self) -> SlotKind {
        match self {
            SlotItem::MenuItem { .. } => SlotKind::Menu,
            SlotItem::IconButton { .. } => SlotKind::Button,
            SlotItem::ListItem { .. } => SlotKind::List,
        }
    }

    /// The user-visible text, for validation.
    pub fn label(&self) -> &str {
        match self {
            SlotItem::MenuItem { label, .. } => label,
            SlotItem::IconButton { tooltip, .. } => tooltip,
            SlotItem::ListItem { title, .. } => title,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SlotError {
    #[error("slot unavailable on this client build")]
    Unavailable,
    #[error("invalid slot item")]
    Invalid,
    #[error("too many items in this slot")]
    Limit,
}
