//! What rekindle offers the client's registries, as the plugin runtime sets
//! it: switcher commands, chat-bar buttons, context-menu rows, profile
//! sections and the modal. rekindle's providers read it whenever the client
//! asks.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rekindle_native::Node;

use crate::panel::{self, PanelItem};

/// A command rekindle contributes to the client's switcher.
#[derive(Clone)]
pub struct Command {
    pub key: String,
    pub section: String,
    pub glyph: String,
    pub title: String,
    pub detail: String,
}

/// A chat-bar button rekindle contributes.
#[derive(Clone)]
pub struct Button {
    pub key: String,
    pub glyph: String,
    pub tooltip: String,
}

/// A modal rekindle contributes: a title, and a plugin panel as its body.
#[derive(Clone)]
pub struct Modal {
    pub plugin: String,
    pub key: String,
    pub title: String,
    /// The modal's panel: its id (interactions are reported against it) and
    /// widgets, drawn as the settings page draws a panel's.
    pub panel: PanelItem,
}

/// A profile section rekindle contributes.
#[derive(Clone)]
pub struct ProfileSectionItem {
    pub plugin: String,
    pub id: String,
    pub title: String,
    pub lines: Vec<String>,
}

/// What kind of thing a menu was opened on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    Message,
    User,
    Channel,
}

/// A context-menu row rekindle contributes. `key` is a rekindle key
/// (plugin + item id).
#[derive(Clone, Debug)]
pub struct MenuItem {
    pub key: String,
    pub kind: MenuKind,
    pub label: String,
    pub danger: bool,
}

/// The target of a clicked menu row, read back from the client's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuTargetRef {
    Message { id: u64, channel: u64, author: u64 },
    User { id: u64 },
    Channel { id: u64 },
}

impl MenuTargetRef {
    pub(super) fn kind(self) -> MenuKind {
        match self {
            MenuTargetRef::Message { .. } => MenuKind::Message,
            MenuTargetRef::User { .. } => MenuKind::User,
            MenuTargetRef::Channel { .. } => MenuKind::Channel,
        }
    }
}

static COMMANDS: Mutex<Vec<Command>> = Mutex::new(Vec::new());
static BUTTONS: Mutex<Vec<Button>> = Mutex::new(Vec::new());
static MENU_ITEMS: Mutex<Vec<MenuItem>> = Mutex::new(Vec::new());
static PROFILES: Mutex<Vec<ProfileSectionItem>> = Mutex::new(Vec::new());
static MODAL: Mutex<Option<Modal>> = Mutex::new(None);

/// How often the client rendered rekindle's modal body and profile
/// sections, and called its profile `opened` and menu `invoke`: the drive
/// checks the client calls them.
pub(crate) static MODAL_RENDERS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_RENDERS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROFILE_OPENS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static MENU_INVOKES: AtomicUsize = AtomicUsize::new(0);

/// Set the switcher commands rekindle offers.
pub fn set_commands(commands: Vec<Command>) {
    *COMMANDS.lock().unwrap() = commands;
}

/// Set the chat-bar buttons rekindle offers, in order.
pub fn set_buttons(buttons: Vec<Button>) {
    *BUTTONS.lock().unwrap() = buttons;
}

/// Set the rows rekindle's menu section offers (filtered per target kind).
pub fn set_menu_items(items: Vec<MenuItem>) {
    *MENU_ITEMS.lock().unwrap() = items;
}

/// Set the profile sections rekindle's provider splices in.
pub fn set_profiles(sections: Vec<ProfileSectionItem>) {
    *PROFILES.lock().unwrap() = sections;
}

/// Set (or clear) the modal rekindle's provider presents.
pub fn set_modal(modal: Option<Modal>) {
    *MODAL.lock().unwrap() = modal;
}

pub(super) fn commands() -> Vec<Command> {
    COMMANDS.lock().unwrap().clone()
}

/// The buttons rekindle offers the client's chat bar, in its order.
pub(crate) fn buttons() -> Vec<Button> {
    BUTTONS.lock().unwrap().clone()
}

pub(super) fn menu_items() -> Vec<MenuItem> {
    MENU_ITEMS.lock().unwrap().clone()
}

pub(super) fn profiles() -> Vec<ProfileSectionItem> {
    PROFILES.lock().unwrap().clone()
}

pub(super) fn modal() -> Option<Modal> {
    MODAL.lock().unwrap().clone()
}

/// The title of the modal rekindle is presenting, if any.
pub(crate) fn modal_title() -> Option<String> {
    MODAL.lock().unwrap().as_ref().map(|m| m.title.clone())
}

/// The plugin whose modal rekindle is presenting, if any.
pub fn modal_plugin() -> Option<String> {
    MODAL.lock().unwrap().as_ref().map(|m| m.plugin.clone())
}

/// The panel of the modal rekindle is presenting, if any.
pub fn modal_panel() -> Option<PanelItem> {
    MODAL.lock().unwrap().as_ref().map(|m| m.panel.clone())
}

/// rekindle's render body for a modal: its panel's widgets, interactive
/// (the client's modal draws the title).
pub(super) fn modal_body(modal: &Modal) -> [usize; 2] {
    let panel = modal.panel.clone();
    rekindle_native::render_body(move |ctx| {
        MODAL_RENDERS.fetch_add(1, Ordering::SeqCst);
        panel::panel_body(&panel, ctx)
    })
}

/// rekindle's render body for a profile section: its lines.
pub(super) fn profile_body(item: &ProfileSectionItem) -> [usize; 2] {
    let lines = item.lines.clone();
    rekindle_native::render_body(move |_| {
        PROFILE_RENDERS.fetch_add(1, Ordering::SeqCst);
        panel::column(4.0, lines.iter().cloned().map(Node::text).collect())
    })
}
