//! What tests and `rekindle dev` read back from a [`MockBackend`].

use super::{MockBackend, RegisteredFont};
use crate::api;
use crate::model::{
    ClickBinding, Command, Decoration, DmSection, Element, GhostMessage, Keybind, Notice,
    OverlayImage, OverlaySprite, Panel, SlotItem, SurfaceStyle, Tint, Toast,
};
use crate::tokens;

impl MockBackend {
    /// Bumped on every change; cheap change detection for UIs.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// A user's global name, else their username; empty if not loaded.
    pub fn name_of(&self, id: u64) -> String {
        self.world
            .users
            .get(&id)
            .map(|u| u.global_name.clone().unwrap_or_else(|| u.username.clone()))
            .unwrap_or_default()
    }

    /// Which plugin currently controls a token, if any.
    pub fn override_owner(&self, token: &str) -> Option<&str> {
        self.overrides
            .get(tokens::canonical(token))
            .and_then(|s| s.last())
            .map(|(p, _)| p.as_str())
    }

    /// The effective tint and the plugin that set it.
    pub fn tint(&self) -> Option<(&str, Tint)> {
        self.tints.last().map(|(p, t)| (p.as_str(), *t))
    }

    pub fn fonts(&self) -> &[RegisteredFont] {
        &self.fonts
    }

    /// The effective style of a named surface, and which plugin set it.
    pub fn surface_style(&self, surface: &str) -> Option<(&str, SurfaceStyle)> {
        self.surface_styles
            .get(surface)
            .and_then(|s| s.last())
            .map(|(p, st)| (p.as_str(), *st))
    }

    pub fn panels(&self) -> impl Iterator<Item = (&str, &Panel)> {
        self.panels.iter().map(|(p, panel)| (p.as_str(), panel))
    }

    pub fn panel(&self, plugin: &str, id: &str) -> Option<&Panel> {
        self.panels
            .iter()
            .find(|(p, panel)| p == plugin && panel.id == id)
            .map(|(_, panel)| panel)
    }

    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    /// Items in one slot, as (plugin, item).
    pub fn slot_items(&self, slot: &str) -> Vec<(&str, &SlotItem)> {
        self.slot_items
            .iter()
            .filter(|(_, s, _)| s == slot)
            .map(|(p, _, i)| (p.as_str(), i))
            .collect()
    }

    pub fn is_active(&self, plugin: &str, slot: &str, id: &str) -> Option<bool> {
        self.active
            .get(&(plugin.to_string(), slot.to_string(), id.to_string()))
            .copied()
    }

    pub fn profile_section(&self, plugin: &str, section: &str, user: u64) -> Option<&[Element]> {
        self.profile_sections
            .get(&(plugin.to_string(), section.to_string(), user))
            .map(Vec::as_slice)
    }

    /// Toasts shown, oldest first.
    pub fn toasts(&self) -> &[Toast] {
        &self.toasts
    }

    /// Recent toasts, oldest first, each with a sequence number that only grows.
    pub fn recent_toasts(&self) -> impl Iterator<Item = (u64, &Toast)> {
        self.toast_log.iter().map(|(s, t)| (*s, t))
    }

    pub fn modal(&self, plugin: &str) -> Option<&Panel> {
        self.modals
            .iter()
            .find(|(p, _)| p == plugin)
            .map(|(_, m)| m)
    }

    pub fn modals(&self) -> impl Iterator<Item = (&str, &Panel)> {
        self.modals.iter().map(|(p, m)| (p.as_str(), m))
    }

    /// The last clipboard write, as (plugin, text).
    pub fn clipboard(&self) -> Option<(&str, &str)> {
        self.clipboard
            .as_ref()
            .map(|(p, t)| (p.as_str(), t.as_str()))
    }

    /// A plugin's overlay sprite by instance id.
    pub fn overlay_sprite(&self, plugin: &str, instance: &str) -> Option<&OverlaySprite> {
        self.overlay_sprites
            .iter()
            .find(|s| s.plugin == plugin && s.instance == instance)
    }

    /// Registered overlay images.
    pub fn overlay_images(&self) -> &[OverlayImage] {
        &self.overlay_images
    }

    /// A decoration, by (plugin, kind, key).
    pub fn decoration(&self, plugin: &str, kind: &str, key: u64) -> Option<&Decoration> {
        self.decorations
            .get(&(plugin.to_string(), kind.to_string(), key))
    }

    /// Every decoration of a kind, as (plugin, key, decoration).
    pub fn decorations(&self, kind: &str) -> Vec<(&str, u64, &Decoration)> {
        self.decorations
            .iter()
            .filter(|((_, k, _), _)| k == kind)
            .map(|((p, _, key), d)| (p.as_str(), *key, d))
            .collect()
    }

    pub fn accessory(&self, plugin: &str, message: u64) -> Option<&[Element]> {
        self.accessories
            .get(&(plugin.to_string(), message))
            .map(Vec::as_slice)
    }

    /// The ghost rows in a channel, as (plugin, ghost).
    pub fn ghosts(&self, channel: u64) -> Vec<(&str, &GhostMessage)> {
        self.ghosts
            .iter()
            .filter(|((_, c, _), _)| *c == channel)
            .map(|((p, _, _), g)| (p.as_str(), g))
            .collect()
    }

    pub fn dm_sections(&self, plugin: &str) -> &[DmSection] {
        self.dm_sections
            .get(plugin)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn notice(&self, plugin: &str) -> Option<&Notice> {
        self.notices.get(plugin)
    }

    pub fn keybinds(&self) -> &[Keybind] {
        &self.keybinds
    }

    pub fn slash_commands(&self) -> impl Iterator<Item = (&str, &api::SlashCommand)> {
        self.slash_commands.iter().map(|(p, c)| (p.as_str(), c))
    }

    pub fn click_bindings(&self) -> impl Iterator<Item = (&str, &ClickBinding)> {
        self.click_bindings.iter().map(|(p, c)| (p.as_str(), c))
    }

    /// A plugin's voice panel status line: (text, since).
    pub fn voice_status(&self, plugin: &str, id: &str) -> Option<(&str, Option<u64>)> {
        self.voice_status
            .get(&(plugin.to_string(), id.to_string()))
            .map(|(t, s)| (t.as_str(), *s))
    }

    pub fn settings_extra(&self, plugin: &str) -> &[Element] {
        self.settings_extra
            .get(plugin)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}
