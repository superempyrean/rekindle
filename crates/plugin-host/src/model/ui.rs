//! The UI a plugin changes through the backend: palette commands,
//! decorations, ghost messages, notices, click bindings, keybinds,
//! surface styles, toasts, overlay images, and [`UiOp`], which carries
//! the newer ones to the backend.

use serde::{Deserialize, Serialize};

use super::{Color, Element, Icon, MenuPredicate, SlotError, Span};

/// A palette command a plugin registered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub plugin: String,
    pub id: String,
    pub title: String,
}

/// Errors from the newer UI surfaces (mirrors `ui.ui-error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiError {
    #[error("unavailable on this client build")]
    Unavailable,
    #[error("invalid")]
    Invalid,
    #[error("limit reached")]
    Limit,
    #[error("needs a user gesture")]
    NoGesture,
    #[error("not loaded")]
    NotFound,
    #[error("capability not granted")]
    NotGranted,
}

impl From<SlotError> for UiError {
    fn from(e: SlotError) -> Self {
        match e {
            SlotError::Unavailable => UiError::Unavailable,
            SlotError::Invalid => UiError::Invalid,
            SlotError::Limit => UiError::Limit,
        }
    }
}

pub const DECORATION_KINDS: [(&str, DecorationKey); 10] = [
    ("user.name-badge", DecorationKey::User),
    ("user.profile-badge", DecorationKey::User),
    ("user.avatar-overlay", DecorationKey::User),
    ("member-list.trailing", DecorationKey::User),
    ("member-list.subtext", DecorationKey::User),
    ("dm-list.trailing", DecorationKey::User),
    ("channel-list.trailing", DecorationKey::Channel),
    ("channel-list.subtitle", DecorationKey::Channel),
    ("guild-list.overlay", DecorationKey::Guild),
    ("message.trailing", DecorationKey::Message),
];

/// What a decoration kind is keyed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DecorationKey {
    User,
    Channel,
    Guild,
    Message,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decoration {
    pub icon: Option<Icon>,
    pub image: Option<String>,
    pub text: Option<String>,
    pub tooltip: Option<String>,
    pub color: Option<Color>,
    pub clickable: bool,
}

/// A plugin-drawn row standing in for a deleted/edited message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GhostMessage {
    pub channel: u64,
    pub id: u64,
    pub author: crate::api::UserRef,
    pub content: Vec<Span>,
    /// "deleted", "edited" or "note".
    pub style: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DmSection {
    pub id: String,
    pub name: String,
    pub color: Option<Color>,
    pub channels: Vec<u64>,
    pub collapsed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notice {
    pub text: String,
    pub kind: ToastKind,
    /// Optional button label (clicks arrive as `on-ui` surface `notice`).
    pub button: Option<String>,
}

/// Where a click binding applies on a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClickArea {
    Content,
    Avatar,
    AuthorName,
    Timestamp,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Mods {
    pub fn any(self) -> bool {
        self.shift || self.ctrl || self.alt || self.meta
    }
}

/// The gesture a click binding claims: (area, click count, modifiers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ClickGesture {
    pub area: ClickArea,
    pub clicks: u8,
    pub mods: Mods,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClickBinding {
    pub id: String,
    pub label: String,
    pub gesture: ClickGesture,
    pub when: Vec<MenuPredicate>,
}

/// A plugin's keybind, as registered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keybind {
    pub plugin: String,
    pub id: String,
    pub label: String,
    pub default: Option<String>,
}

/// A restyle of a named layout surface (`theming.surface-style`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SurfaceStyle {
    pub hidden: bool,
    pub opacity: Option<f32>,
    pub radius: Option<f32>,
    pub background: Option<Color>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Background {
    pub image: String,
    pub opacity: f32,
    pub blur: f32,
}

/// The named layout surfaces a plugin may hide or restyle.
pub const SURFACES: [&str; 16] = [
    "guild-list",
    "channel-list",
    "member-list",
    "user-area",
    "title-bar",
    "title-bar.help",
    "title-bar.inbox",
    "chat.gift-button",
    "chat.gif-button",
    "chat.sticker-button",
    "chat.emoji-button",
    "chat.app-launcher",
    "channel.header",
    "voice-panel",
    "activity-panel",
    "nitro-upsell",
];

/// The client strings a plugin may replace (`theming.set-string`), by
/// rekindle's own names; a backend maps each to its build's string.
pub const STRINGS: [&str; 6] = [
    "composer.placeholder",
    "channel-list.text-channels",
    "channel-list.voice-channels",
    "member-list.online",
    "member-list.offline",
    "chat.welcome",
];

/// UI updates for the 0.8.0 surfaces, applied by [`ClientBackend::ui_update`]
/// (one method, so a backend handles the ones its build supports and
/// reports `unavailable` for the rest).
///
/// [`ClientBackend::ui_update`]: crate::backend::ClientBackend::ui_update
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum UiOp {
    RegisterImage {
        id: String,
        #[serde(skip)]
        png: Vec<u8>,
        width: u32,
        height: u32,
    },
    SetActive {
        slot: String,
        id: String,
        active: bool,
    },
    SetVoiceStatus {
        id: String,
        text: String,
        since_ms: Option<u64>,
    },
    SetSettingsExtra {
        elements: Vec<Element>,
    },
    SetDecoration {
        kind: String,
        key: u64,
        decoration: Option<Decoration>,
    },
    SetDecorations {
        kind: String,
        items: Vec<(u64, Decoration)>,
    },
    ClearDecorations {
        kind: String,
    },
    SetMessageAccessory {
        message: crate::api::MessageRef,
        elements: Option<Vec<Element>>,
    },
    SetGhostMessage {
        ghost: GhostMessage,
    },
    RemoveGhostMessage {
        channel: u64,
        id: u64,
    },
    SetDmSections {
        sections: Vec<DmSection>,
    },
    ShowNotice {
        notice: Notice,
    },
    DismissNotice,
    RegisterKeybind {
        id: String,
        label: String,
        default: Option<String>,
    },
    UnregisterKeybind {
        id: String,
    },
    RegisterSlashCommand {
        command: crate::api::SlashCommand,
    },
    UnregisterSlashCommand {
        name: String,
    },
    RegisterClick {
        binding: ClickBinding,
    },
    UnregisterClick {
        id: String,
    },
    SetSurface {
        surface: String,
        style: SurfaceStyle,
    },
    ResetSurface {
        surface: String,
    },
    SetBackground {
        background: Option<Background>,
    },
    /// Another avatar or banner for `user` on this client (registered image
    /// ids); both `None` shows theirs again.
    SetProfileArt {
        user: u64,
        avatar: Option<String>,
        banner: Option<String>,
    },
    SetWindowOnTop {
        on_top: bool,
    },
    /// 0.3 to 1.
    SetWindowOpacity {
        opacity: f32,
    },
    /// One of the client's own strings, by key; `None` restores it.
    SetString {
        key: String,
        text: Option<String>,
    },
    /// The account changed: drop this plugin's per-account contributions
    /// (decorations, accessories, ghost messages, DM sections).
    ClearAccountState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toast {
    pub plugin: String,
    pub text: String,
    pub kind: ToastKind,
}

/// A PNG atlas a plugin registered for the overlay layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OverlayImage {
    pub plugin: String,
    pub id: String,
    #[serde(skip)]
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}
