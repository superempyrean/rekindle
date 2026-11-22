//! Desktop notifications, tweaks, picked files and overlay sprites.

use serde::{Deserialize, Serialize};

/// A desktop notification a plugin posts (`notify.desktop-notification`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesktopNotification {
    pub plugin: String,
    pub title: String,
    pub body: String,
    pub icon: Option<String>,
    pub on_click: Option<crate::api::Target>,
    pub sound: Option<String>,
}

/// A host tweak (`tweaks.tweak-info`), as the backend reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tweak {
    pub id: String,
    pub label: String,
    pub description: String,
    pub kind: TweakKind,
    pub choices: Vec<String>,
    pub value: String,
    pub available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TweakKind {
    Toggle,
    Choice,
    Number,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TweakError {
    #[error("unknown tweak")]
    Unknown,
    #[error("tweak unavailable on this build")]
    Unavailable,
    #[error("invalid value")]
    InvalidValue,
    #[error("the user set this by hand")]
    UserLocked,
}

/// A file the user picked in an Open dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickedFile {
    pub name: String,
    pub content_type: Option<String>,
    #[serde(skip)]
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileError {
    #[error("needs a user gesture")]
    NoGesture,
    #[error("the user cancelled")]
    Cancelled,
    #[error("too large")]
    TooLarge,
    #[error("failed")]
    Failed,
}

/// A live overlay sprite: a sub-rect of an image blitted to a position, both in
/// the units of `overlay.sprite` (source pixels, logical dest pixels).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OverlaySprite {
    pub plugin: String,
    pub instance: String,
    pub image: String,
    pub sx: u32,
    pub sy: u32,
    pub sw: u32,
    pub sh: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
