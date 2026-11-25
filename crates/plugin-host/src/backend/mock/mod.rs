//! [`MockBackend`]: an in-memory client.
//!
//! Theme overrides and UI contributions layer per plugin: the most recent
//! writer of a token or surface wins, and unloading a plugin reveals what
//! was underneath. Queries answer from a [`World`], which plugin actions
//! ([`world`]) and gateway dispatches ([`dispatch`]) change.

mod dispatch;
mod inspect;
mod rest;
#[cfg(test)]
mod tests;
mod world;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub use world::World;
pub(crate) use world::{ADMINISTRATOR, ALL_PERMISSIONS, VIEW_CHANNEL, message_ref};

use super::ClientBackend;
use super::fixtures;
use crate::api::{
    self, Channel, CustomStatus, Emoji, Guild, GuildFolder, GuildReadState, Message, MyVoice,
    Overwrite, Presence, ReadState, Relationship, Role, Selection, Sticker, User, VoiceState,
};
use crate::model::{
    ALL_SLOTS, Action, ActionError, ClickBinding, Color, Command, DECORATION_KINDS, Decoration,
    DesktopNotification, DmSection, Element, FileError, GhostMessage, Keybind, Notice,
    OverlayImage, OverlaySprite, Panel, PickedFile, SLOT_EXPRESSION_PICKER_TAB,
    SLOT_PROFILE_SECTION, SLOT_VOICE_PANEL_STATUS, STRINGS, SURFACES, SlotError, SlotItem,
    SurfaceStyle, ThemeValue, Tint, Toast, ToastKind, Tweak, TweakError, TweakKind, UiError, UiOp,
    slot_kind,
};
use crate::tokens::{self, ThemeError, TintRole, TokenInfo};

/// Max fonts a single plugin may register.
const MAX_FONTS_PER_PLUGIN: usize = 8;
/// The client viewport in logical pixels.
const VIEWPORT: (f32, f32) = (1280.0, 800.0);
/// How many toasts [`MockBackend::toasts`] keeps.
const MAX_TOASTS: usize = 50;
/// How many toasts [`MockBackend::recent_toasts`] keeps.
const MAX_RECENT_TOASTS: usize = 20;

/// A font a plugin registered.
#[derive(Debug, Clone)]
pub struct RegisteredFont {
    pub plugin: String,
    pub family: String,
}

/// An in-memory client, for tests and `rekindle dev`.
#[derive(Debug, Clone)]
pub struct MockBackend {
    base: BTreeMap<String, (TokenInfo, ThemeValue)>,
    /// token -> stack of (plugin, value), last is effective.
    overrides: BTreeMap<String, Vec<(String, ThemeValue)>>,
    /// (plugin, tint), last is effective.
    tints: Vec<(String, Tint)>,
    fonts: Vec<RegisteredFont>,
    /// (plugin, panel) in insertion order of first show.
    panels: Vec<(String, Panel)>,
    commands: Vec<Command>,
    /// Slots this mock pretends its build supports (all by default).
    available: Vec<String>,
    /// (plugin, slot, item), in insertion order.
    slot_items: Vec<(String, String, SlotItem)>,
    /// (plugin, section id, user id) -> elements.
    profile_sections: BTreeMap<(String, String, u64), Vec<Element>>,
    /// Most recent last.
    toasts: Vec<Toast>,
    /// Recent toasts with a sequence number that only grows, so a UI
    /// polling for new ones can tell a new "Saved" from an old one.
    toast_log: VecDeque<(u64, Toast)>,
    toast_seq: u64,
    modals: Vec<(String, Panel)>,
    /// Registered overlay atlases.
    overlay_images: Vec<OverlayImage>,
    /// Live overlay sprites, in draw order.
    overlay_sprites: Vec<OverlaySprite>,
    /// Last clipboard write, as (plugin, text).
    clipboard: Option<(String, String)>,
    /// Bumped on every mutation; cheap change detection for UIs.
    revision: u64,
    /// The client the queries answer from.
    pub world: World,
    /// What the simulated client keeps beside the world to apply
    /// gateway dispatches.
    dispatch_stores: dispatch::Stores,
    /// Every action performed, as (plugin, action).
    pub performed: Vec<(String, Action)>,
    /// (plugin, kind, key) -> decoration.
    decorations: BTreeMap<(String, String, u64), Decoration>,
    /// (plugin, message id) -> accessory elements.
    accessories: BTreeMap<(String, u64), Vec<Element>>,
    /// (plugin, channel, id) -> ghost row.
    ghosts: BTreeMap<(String, u64, u64), GhostMessage>,
    dm_sections: BTreeMap<String, Vec<DmSection>>,
    notices: BTreeMap<String, Notice>,
    keybinds: Vec<Keybind>,
    slash_commands: Vec<(String, api::SlashCommand)>,
    click_bindings: Vec<(String, ClickBinding)>,
    /// (plugin, slot, id) -> active.
    active: BTreeMap<(String, String, String), bool>,
    /// (plugin, id) -> (text, since).
    voice_status: BTreeMap<(String, String), (String, Option<u64>)>,
    settings_extra: BTreeMap<String, Vec<Element>>,
    /// surface -> stack of (plugin, style).
    surface_styles: BTreeMap<String, Vec<(String, SurfaceStyle)>>,
    /// Posted desktop notifications.
    pub notifications: Vec<DesktopNotification>,
    tweaks: BTreeMap<String, TweakEntry>,
    /// `view` overrides, as (plugin, overrides), the last set last.
    views: Vec<(String, crate::model::ViewOverrides)>,
}

/// A tweak in the mock's registry.
#[derive(Debug, Clone)]
struct TweakEntry {
    info: Tweak,
    /// (plugin, value) presets; the last is effective.
    presets: Vec<(String, String)>,
    /// The value the user set by hand, which wins over presets.
    user: Option<String>,
}

impl MockBackend {
    /// A member as the plugins' `view` overrides show it.
    fn viewed(&self, mut m: api::Member) -> api::Member {
        for (_, v) in &self.views {
            v.apply_member(self.world.me, &mut m);
        }
        m
    }
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MockBackend {
    /// A client with the default tokens, every slot, and
    /// [`sample_world`](super::sample_world).
    pub fn new() -> Self {
        Self {
            base: tokens::default_tokens()
                .into_iter()
                .map(|(i, v)| (i.name.clone(), (i, v)))
                .collect(),
            overrides: BTreeMap::new(),
            tints: Vec::new(),
            fonts: Vec::new(),
            panels: Vec::new(),
            commands: Vec::new(),
            // The mock client has no emoji picker: a plugin that adds a tab
            // there falls back as on a client without one. A test turns it
            // on with `set_slot_available`.
            available: ALL_SLOTS
                .iter()
                .map(|(s, _)| s.to_string())
                .filter(|s| s != SLOT_EXPRESSION_PICKER_TAB)
                .collect(),
            slot_items: Vec::new(),
            profile_sections: BTreeMap::new(),
            toasts: Vec::new(),
            toast_log: VecDeque::new(),
            toast_seq: 0,
            modals: Vec::new(),
            overlay_images: Vec::new(),
            overlay_sprites: Vec::new(),
            clipboard: None,
            revision: 0,
            world: fixtures::sample_world(),
            dispatch_stores: dispatch::Stores::default(),
            performed: Vec::new(),
            decorations: BTreeMap::new(),
            accessories: BTreeMap::new(),
            ghosts: BTreeMap::new(),
            dm_sections: BTreeMap::new(),
            notices: BTreeMap::new(),
            views: Vec::new(),
            keybinds: Vec::new(),
            slash_commands: Vec::new(),
            click_bindings: Vec::new(),
            active: BTreeMap::new(),
            voice_status: BTreeMap::new(),
            settings_extra: BTreeMap::new(),
            surface_styles: BTreeMap::new(),
            notifications: Vec::new(),
            tweaks: fixtures::sample_tweaks()
                .into_iter()
                .map(|info| {
                    let entry = TweakEntry {
                        info,
                        presets: Vec::new(),
                        user: None,
                    };
                    (entry.info.id.clone(), entry)
                })
                .collect(),
        }
    }

    /// A mock that answers queries from `world`.
    pub fn with_world(world: World) -> Self {
        let mut b = Self::new();
        b.world = world;
        b
    }

    /// Process a gateway dispatch the way Discord's client does: apply it to
    /// the world. This is the simulated client's side of the dispatch path;
    /// [`gateway::Dispatch`](super::gateway::Dispatch) is rekindle's.
    pub fn process_dispatch(&mut self, t: &str, d: &serde_json::Value, now_ms: u64) {
        dispatch::Sim::new(&mut self.world, &mut self.dispatch_stores).apply(t, d, now_ms);
    }

    /// End the typing indicators that have lasted 10 seconds without a new
    /// `TYPING_START`.
    pub fn expire_typing(&mut self, now_ms: u64) {
        dispatch::Sim::new(&mut self.world, &mut self.dispatch_stores).tick(now_ms);
    }

    /// Pretend this build does (or doesn't) have a slot's hook point.
    pub fn set_slot_available(&mut self, slot: &str, available: bool) {
        self.available.retain(|s| s != slot);
        if available {
            self.available.push(slot.to_string());
        } else {
            self.slot_items.retain(|(_, s, _)| s != slot);
        }
        self.bump();
    }

    /// The user sets a tweak by hand (it wins over plugin presets), or
    /// clears it with `None`.
    pub fn set_tweak_by_user(&mut self, id: &str, value: Option<&str>) {
        if let Some(t) = self.tweaks.get_mut(id) {
            t.user = value.map(str::to_string);
        }
    }

    fn bump(&mut self) {
        self.revision += 1;
    }

    fn clear_account_state(&mut self, plugin: &str) {
        self.decorations.retain(|(p, _, _), _| p != plugin);
        self.accessories.retain(|(p, _), _| p != plugin);
        self.ghosts.retain(|(p, _, _), _| p != plugin);
        self.dm_sections.remove(plugin);
    }

    /// The `ui_update` operations on messages and decorations.
    fn update_decorations(&mut self, plugin: &str, op: UiOp) -> Result<(), UiError> {
        let p = plugin.to_string();
        match op {
            UiOp::SetDecoration {
                kind,
                key,
                decoration: Some(d),
            } => {
                self.decorations.insert((p, kind, key), d);
            }
            UiOp::SetDecoration {
                kind,
                key,
                decoration: None,
            } => {
                self.decorations.remove(&(p, kind, key));
            }
            UiOp::SetDecorations { kind, items } => {
                self.decorations
                    .retain(|(q, k, _), _| !(q == plugin && *k == kind));
                for (key, d) in items {
                    self.decorations.insert((p.clone(), kind.clone(), key), d);
                }
            }
            UiOp::ClearDecorations { kind } => {
                self.decorations
                    .retain(|(q, k, _), _| !(q == plugin && *k == kind));
            }
            UiOp::SetMessageAccessory {
                message,
                elements: Some(e),
            } => {
                if self
                    .world
                    .find_message(message.channel, message.id)
                    .is_none()
                {
                    return Err(UiError::NotFound);
                }
                self.accessories.insert((p, message.id), e);
            }
            UiOp::SetMessageAccessory {
                message,
                elements: None,
            } => {
                self.accessories.remove(&(p, message.id));
            }
            UiOp::SetGhostMessage { ghost } => {
                self.ghosts.insert((p, ghost.channel, ghost.id), ghost);
            }
            UiOp::RemoveGhostMessage { channel, id } => {
                self.ghosts.remove(&(p, channel, id));
            }
            UiOp::ClearAccountState => self.clear_account_state(plugin),
            _ => unreachable!("ui_update passes only decoration and message operations"),
        }
        Ok(())
    }
}

impl ClientBackend for MockBackend {
    // ---------------- theming ----------------

    fn theme_tokens(&self) -> Vec<TokenInfo> {
        self.base.values().map(|(i, _)| i.clone()).collect()
    }

    fn get_value(&self, token: &str) -> Option<ThemeValue> {
        let token = tokens::canonical(token);
        if let Some((_, v)) = self.overrides.get(token).and_then(|s| s.last()) {
            return Some(v.clone());
        }
        let (info, base) = self.base.get(token)?;
        // The most recent tint applies to base values; overrides win over it.
        let (ThemeValue::Color(c), Some((_, t))) = (base, self.tints.last()) else {
            return Some(base.clone());
        };
        let amount = match info.tint {
            TintRole::Surface => t.amount,
            TintRole::Text => t.text_amount,
            TintRole::None => 0.0,
        };
        Some(ThemeValue::Color(if amount > 0.0 {
            c.mix_oklab(t.color, amount)
        } else {
            *c
        }))
    }

    fn set_value(
        &mut self,
        plugin: &str,
        token: &str,
        value: ThemeValue,
    ) -> Result<(), ThemeError> {
        let token = tokens::canonical(token);
        let (info, _) = self.base.get(token).ok_or(ThemeError::UnknownToken)?;
        tokens::validate(info, &value)?;
        if let ThemeValue::Font(family) = &value {
            // A plugin may only name fonts it registered, or the client's own.
            let own = self
                .fonts
                .iter()
                .any(|r| r.plugin == plugin && r.family == *family);
            let builtin = self.base.values().any(|(_, v)| *v == value);
            if !own && !builtin {
                return Err(ThemeError::OutOfRange);
            }
        }
        let stack = self.overrides.entry(token.to_string()).or_default();
        stack.retain(|(p, _)| p != plugin);
        stack.push((plugin.to_string(), value));
        self.bump();
        Ok(())
    }

    fn reset_value(&mut self, plugin: &str, token: &str) {
        let token = tokens::canonical(token);
        if let Some(stack) = self.overrides.get_mut(token) {
            stack.retain(|(p, _)| p != plugin);
            if stack.is_empty() {
                self.overrides.remove(token);
            }
            self.bump();
        }
    }

    fn set_tint(&mut self, plugin: &str, tint: Option<Tint>) {
        self.tints.retain(|(p, _)| p != plugin);
        if let Some(t) = tint {
            self.tints.push((plugin.to_string(), t));
        }
        self.bump();
    }

    fn register_font(&mut self, plugin: &str, family: &str, _data: Vec<u8>) -> Result<(), String> {
        self.fonts
            .retain(|f| !(f.plugin == plugin && f.family == family));
        if self.fonts.iter().filter(|f| f.plugin == plugin).count() >= MAX_FONTS_PER_PLUGIN {
            return Err(format!("at most {MAX_FONTS_PER_PLUGIN} fonts per plugin"));
        }
        self.fonts.push(RegisteredFont {
            plugin: plugin.into(),
            family: family.into(),
        });
        self.bump();
        Ok(())
    }

    // ---------------- panels, commands, slots ----------------

    fn show_panel(&mut self, plugin: &str, panel: Panel) {
        match self
            .panels
            .iter_mut()
            .find(|(p, existing)| p == plugin && existing.id == panel.id)
        {
            Some((_, existing)) => *existing = panel,
            None => self.panels.push((plugin.to_string(), panel)),
        }
        self.bump();
    }

    fn remove_panel(&mut self, plugin: &str, panel_id: &str) {
        self.panels
            .retain(|(p, panel)| !(p == plugin && panel.id == panel_id));
        self.bump();
    }

    fn register_command(&mut self, plugin: &str, id: &str, title: &str) {
        self.unregister_command(plugin, id);
        self.commands.push(Command {
            plugin: plugin.into(),
            id: id.into(),
            title: title.into(),
        });
        self.bump();
    }

    fn unregister_command(&mut self, plugin: &str, id: &str) {
        self.commands
            .retain(|c| !(c.plugin == plugin && c.id == id));
        self.bump();
    }

    fn available_slots(&self) -> Vec<String> {
        let mut v = self.available.clone();
        v.sort();
        v
    }

    fn add_slot_item(&mut self, plugin: &str, slot: &str, item: SlotItem) -> Result<(), SlotError> {
        if !self.available.iter().any(|s| s == slot) {
            return Err(SlotError::Unavailable);
        }
        if slot_kind(slot) != Some(item.kind()) {
            return Err(SlotError::Invalid);
        }
        self.slot_items
            .retain(|(p, s, i)| !(p == plugin && s == slot && i.id() == item.id()));
        self.slot_items
            .push((plugin.to_string(), slot.to_string(), item));
        self.bump();
        Ok(())
    }

    fn remove_slot_item(&mut self, plugin: &str, slot: &str, id: &str) {
        self.slot_items
            .retain(|(p, s, i)| !(p == plugin && s == slot && i.id() == id));
        if slot == SLOT_PROFILE_SECTION {
            self.profile_sections
                .retain(|(p, s, _), _| !(p == plugin && s == id));
        }
        self.bump();
    }

    fn set_profile_section(
        &mut self,
        plugin: &str,
        section: &str,
        user: u64,
        elements: Vec<Element>,
    ) {
        self.profile_sections
            .insert((plugin.to_string(), section.to_string(), user), elements);
        self.bump();
    }

    fn show_toast(&mut self, plugin: &str, text: &str, kind: ToastKind) {
        let toast = Toast {
            plugin: plugin.into(),
            text: text.into(),
            kind,
        };
        self.toasts.push(toast.clone());
        if self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
        self.toast_seq += 1;
        self.toast_log.push_back((self.toast_seq, toast));
        if self.toast_log.len() > MAX_RECENT_TOASTS {
            self.toast_log.pop_front();
        }
        self.bump();
    }

    fn show_modal(&mut self, plugin: &str, panel: Panel) {
        self.modals.retain(|(p, _)| p != plugin);
        self.modals.push((plugin.to_string(), panel));
        self.bump();
    }

    fn close_modal(&mut self, plugin: &str) {
        self.modals.retain(|(p, _)| p != plugin);
        self.bump();
    }

    // ---------------- overlay, clipboard ----------------

    fn overlay_register_image(&mut self, plugin: &str, id: &str, png: Vec<u8>, w: u32, h: u32) {
        self.overlay_images
            .retain(|i| !(i.plugin == plugin && i.id == id));
        self.overlay_images.push(OverlayImage {
            plugin: plugin.into(),
            id: id.into(),
            png,
            width: w,
            height: h,
        });
        self.bump();
    }

    fn overlay_remove_image(&mut self, plugin: &str, id: &str) {
        self.overlay_images
            .retain(|i| !(i.plugin == plugin && i.id == id));
        self.overlay_sprites
            .retain(|s| !(s.plugin == plugin && s.image == id));
        self.bump();
    }

    fn overlay_draw(&mut self, sprite: OverlaySprite) {
        self.overlay_sprites
            .retain(|s| !(s.plugin == sprite.plugin && s.instance == sprite.instance));
        self.overlay_sprites.push(sprite);
        self.bump();
    }

    fn overlay_clear(&mut self, plugin: &str, instance: &str) {
        self.overlay_sprites
            .retain(|s| !(s.plugin == plugin && s.instance == instance));
        self.bump();
    }

    fn viewport(&self) -> (f32, f32) {
        VIEWPORT
    }

    fn write_clipboard(&mut self, plugin: &str, text: &str) {
        self.clipboard = Some((plugin.to_string(), text.to_string()));
        self.bump();
    }

    fn write_clipboard_png(&mut self, plugin: &str, png: Vec<u8>) -> bool {
        self.clipboard = Some((plugin.to_string(), format!("<png {} bytes>", png.len())));
        self.bump();
        true
    }

    fn plugin_unloaded(&mut self, plugin: &str) {
        self.slot_items.retain(|(p, _, _)| p != plugin);
        self.profile_sections.retain(|(p, _, _), _| p != plugin);
        self.modals.retain(|(p, _)| p != plugin);
        for stack in self.overrides.values_mut() {
            stack.retain(|(p, _)| p != plugin);
        }
        self.overrides.retain(|_, s| !s.is_empty());
        self.tints.retain(|(p, _)| p != plugin);
        // A real backend may keep font data until restart; the mock forgets
        // it so a reload starts clean.
        self.fonts.retain(|f| f.plugin != plugin);
        self.panels.retain(|(p, _)| p != plugin);
        self.commands.retain(|c| c.plugin != plugin);
        self.overlay_images.retain(|i| i.plugin != plugin);
        self.overlay_sprites.retain(|s| s.plugin != plugin);
        self.clear_account_state(plugin);
        self.notices.remove(plugin);
        self.views.retain(|(p, _)| p != plugin);
        self.keybinds.retain(|k| k.plugin != plugin);
        self.slash_commands.retain(|(p, _)| p != plugin);
        self.click_bindings.retain(|(p, _)| p != plugin);
        self.active.retain(|(p, _, _), _| p != plugin);
        self.voice_status.retain(|(p, _), _| p != plugin);
        self.settings_extra.remove(plugin);
        for stack in self.surface_styles.values_mut() {
            stack.retain(|(p, _)| p != plugin);
        }
        self.surface_styles.retain(|_, s| !s.is_empty());
        for t in self.tweaks.values_mut() {
            t.presets.retain(|(p, _)| p != plugin);
        }
        self.bump();
    }

    // ---------------- the client ----------------

    fn client_info(&self) -> api::ClientInfo {
        api::ClientInfo {
            host_version: String::new(),
            client: "mock".into(),
            client_build: "mock-0".into(),
            platform: std::env::consts::OS.into(),
            locale: "en-US".into(),
            base_theme: "dark".into(),
            reduced_motion: self
                .world
                .settings
                .get("accessibility.reduced-motion")
                .is_some_and(|v| v == "true"),
            streamer_mode: false,
        }
    }

    fn extra_features(&self) -> Vec<String> {
        [
            "state.client",
            "state.users",
            "state.messages",
            "messages.resolve-link",
            "inspect.raw-json",
            "history.query",
            "view",
        ]
        .map(String::from)
        .to_vec()
    }

    fn selection(&self) -> Selection {
        self.world.selection
    }

    fn me(&self) -> Option<User> {
        self.user(self.world.me)
    }

    fn my_status(&self) -> String {
        self.world.status.clone()
    }

    fn my_custom_status(&self) -> Option<CustomStatus> {
        self.world.custom_status.clone()
    }

    fn user(&self, id: u64) -> Option<User> {
        let mut u = self.world.users.get(&id).cloned()?;
        for (_, v) in &self.views {
            v.apply_user(self.world.me, &mut u);
        }
        Some(u)
    }

    fn guild(&self, id: u64) -> Option<Guild> {
        self.world.guilds.iter().find(|g| g.id == id).cloned()
    }

    fn guild_ids(&self) -> Vec<u64> {
        self.world.guilds.iter().map(|g| g.id).collect()
    }

    fn guild_folders(&self) -> Vec<GuildFolder> {
        self.world.folders.clone()
    }

    fn channel(&self, id: u64) -> Option<Channel> {
        self.world.channel(id).cloned()
    }

    /// A guild's channels in sidebar order: each category before its
    /// channels, by position. Threads are listed by [`Self::threads`].
    fn channels(&self, guild: u64, include_hidden: bool) -> Vec<Channel> {
        let mut v: Vec<Channel> = self
            .world
            .channels
            .iter()
            .filter(|c| {
                c.guild == Some(guild) && !world::is_thread(c.kind) && (include_hidden || c.visible)
            })
            .cloned()
            .collect();
        v.sort_by_key(|c| {
            (
                c.parent.unwrap_or(c.id),
                c.kind != world::GUILD_CATEGORY,
                c.position,
            )
        });
        v
    }

    fn threads(&self, parent: u64) -> Vec<Channel> {
        self.world
            .channels
            .iter()
            .filter(|c| c.parent == Some(parent) && world::is_thread(c.kind))
            .cloned()
            .collect()
    }

    fn private_channels(&self) -> Vec<Channel> {
        self.world
            .channels
            .iter()
            .filter(|c| c.guild.is_none())
            .cloned()
            .collect()
    }

    fn dm_channel(&self, user: u64) -> Option<u64> {
        self.world
            .channels
            .iter()
            .find(|c| c.kind == world::DM && c.recipients.iter().any(|r| r.id == user))
            .map(|c| c.id)
    }

    fn role(&self, guild: u64, id: u64) -> Option<Role> {
        self.world
            .roles
            .iter()
            .find(|r| r.guild == guild && r.id == id)
            .cloned()
    }

    /// Highest position first.
    fn roles(&self, guild: u64) -> Vec<Role> {
        let mut v: Vec<Role> = self
            .world
            .roles
            .iter()
            .filter(|r| r.guild == guild)
            .cloned()
            .collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.position));
        v
    }

    fn guild_emoji(&self, guild: u64) -> Vec<Emoji> {
        self.world
            .emoji
            .iter()
            .filter(|e| e.guild == Some(guild))
            .cloned()
            .collect()
    }

    fn frequent_emoji(&self, limit: u32) -> Vec<Emoji> {
        self.world
            .frequent_emoji
            .iter()
            .take(limit as usize)
            .cloned()
            .collect()
    }

    fn guild_stickers(&self, guild: u64) -> Vec<Sticker> {
        self.world
            .stickers
            .iter()
            .filter(|s| s.guild == Some(guild))
            .cloned()
            .collect()
    }

    fn my_member(&self, guild: u64) -> Option<api::Member> {
        self.member(guild, self.world.me)
    }

    fn my_permissions(&self, channel_or_guild: u64) -> u64 {
        self.world.my_permissions(channel_or_guild)
    }

    fn permission_overwrites(&self, channel: u64) -> Vec<Overwrite> {
        self.world
            .overwrites
            .get(&channel)
            .cloned()
            .unwrap_or_default()
    }

    fn read_state(&self, channel: u64) -> Option<ReadState> {
        self.world.read_states.get(&channel).copied()
    }

    fn guild_read_state(&self, guild: u64) -> GuildReadState {
        let channels: BTreeSet<u64> = self
            .world
            .channels
            .iter()
            .filter(|c| c.guild == Some(guild))
            .map(|c| c.id)
            .collect();
        let (unread, mentions) = self
            .world
            .read_states
            .values()
            .filter(|r| channels.contains(&r.channel))
            .fold((false, 0), |(unread, mentions), r| {
                (unread || r.unread, mentions + r.mentions)
            });
        GuildReadState {
            guild,
            unread,
            mentions,
        }
    }

    fn unread_channels(&self, limit: u32) -> Vec<ReadState> {
        self.world
            .read_states
            .values()
            .filter(|r| r.unread)
            .take(limit as usize)
            .copied()
            .collect()
    }

    fn my_voice(&self) -> Option<MyVoice> {
        self.world.my_voice.clone()
    }

    fn my_setting(&self, key: &str) -> Option<String> {
        let real = self.world.settings.get(key)?;
        let overridden = self
            .views
            .iter()
            .rev()
            .find_map(|(_, o)| o.settings.get(key));
        Some(overridden.unwrap_or(real).clone())
    }

    fn my_setting_keys(&self) -> Vec<String> {
        self.world.settings.keys().cloned().collect()
    }

    // ---------------- users ----------------

    fn presence(&self, user: u64, _guild: Option<u64>) -> Option<Presence> {
        self.world.presences.get(&user).cloned()
    }

    fn member(&self, guild: u64, user: u64) -> Option<api::Member> {
        self.world
            .member(guild, user)
            .cloned()
            .map(|m| self.viewed(m))
    }

    fn loaded_members(&self, guild: u64, limit: u32) -> (Vec<api::Member>, u32) {
        let all = self.world.members.iter().filter(|m| m.guild == guild);
        let total = all.clone().count() as u32;
        let shown = all.take(limit as usize).map(|m| self.viewed(m.clone()));
        (shown.collect(), total)
    }

    fn member_permissions(&self, user: u64, channel_or_guild: u64) -> Option<u64> {
        let w = &self.world;
        let channel = w.channel(channel_or_guild);
        let guild_id = channel.and_then(|c| c.guild).unwrap_or(channel_or_guild);
        let guild = w.guilds.iter().find(|g| g.id == guild_id)?;
        let member = w.member(guild_id, user)?;
        let base = w.guild_permissions(guild, user, &member.roles);
        let Some(c) = channel else {
            return Some(base);
        };
        // A thread has its parent's permissions.
        let id = match c.parent {
            Some(parent) if world::is_thread(c.kind) => parent,
            _ => c.id,
        };
        Some(w.channel_permissions(guild_id, id, base, user, &member.roles))
    }

    /// The color of the member's highest colored role.
    fn member_color(&self, guild: u64, user: u64) -> Option<Color> {
        let m = self.world.member(guild, user)?;
        self.roles(guild)
            .into_iter()
            .filter(|r| m.roles.contains(&r.id))
            .find_map(|r| r.color)
            .map(|c| Color {
                r: c.r,
                g: c.g,
                b: c.b,
                a: c.a,
            })
    }

    fn typing(&self, channel: u64) -> Vec<u64> {
        self.world.typing.get(&channel).cloned().unwrap_or_default()
    }

    fn voice_states(&self, channel: u64) -> Vec<VoiceState> {
        self.world
            .voice
            .iter()
            .filter(|v| v.channel == channel)
            .copied()
            .collect()
    }

    fn voice_state(&self, user: u64) -> Option<VoiceState> {
        self.world.voice.iter().find(|v| v.user == user).copied()
    }

    fn speaking(&self, user: u64) -> bool {
        self.world.speaking.contains(&user)
    }

    fn relationships(&self) -> Vec<Relationship> {
        self.world.relationships.clone()
    }

    fn note(&self, user: u64) -> Option<String> {
        self.world.notes.get(&user).cloned()
    }

    fn mutual_guilds(&self, user: u64) -> Vec<u64> {
        let w = &self.world;
        w.guilds
            .iter()
            .map(|g| g.id)
            .filter(|g| w.member(*g, user).is_some() && w.member(*g, w.me).is_some())
            .collect()
    }

    // ---------------- messages ----------------

    fn message(&self, channel: u64, id: u64) -> Option<Message> {
        self.world.find_message(channel, id).cloned()
    }

    fn loaded_messages(&self, channel: u64, before: Option<u64>, limit: u32) -> Vec<Message> {
        let Some(all) = self.world.messages.get(&channel) else {
            return vec![];
        };
        all.iter()
            .rev()
            .filter(|m| before.is_none_or(|b| m.id < b))
            .take(limit as usize)
            .cloned()
            .collect()
    }

    fn visible_messages(&self) -> Vec<api::MessageRef> {
        self.world.visible.clone()
    }

    fn pins(&self, channel: u64) -> Vec<api::MessageRef> {
        let Some(all) = self.world.messages.get(&channel) else {
            return vec![];
        };
        all.iter().filter(|m| m.pinned).map(message_ref).collect()
    }

    fn link_on_screen(&self, url: &str) -> bool {
        self.world.visible.iter().any(|r| {
            self.world
                .find_message(r.channel, r.id)
                .is_some_and(|m| m.content.contains(url))
        })
    }

    /// A message from [`World::remote_messages`], in a channel the user
    /// can see.
    fn fetch_linked_message(
        &mut self,
        _guild: Option<u64>,
        channel: u64,
        id: u64,
    ) -> Option<Option<Message>> {
        if !self.world.channel(channel).is_some_and(|c| c.visible) {
            return Some(None);
        }
        Some(
            self.world
                .remote_messages
                .iter()
                .find(|m| m.channel == channel && m.id == id)
                .cloned(),
        )
    }

    fn raw_json(&self, target: &api::Target) -> Option<String> {
        use api::Target as T;
        let v = match target {
            T::Message(r) => serde_json::to_value(self.message(r.channel, r.id)?),
            T::User((u, _)) => serde_json::to_value(self.user(u.id)?),
            T::Channel(c) => serde_json::to_value(self.channel(c.id)?),
            T::Guild(g) => serde_json::to_value(self.guild(g.id)?),
            T::Role(r) => serde_json::to_value(self.role(r.guild, r.id)?),
            _ => return None,
        };
        serde_json::to_string_pretty(&v.ok()?).ok()
    }

    // ---------------- acting ----------------

    fn perform(&mut self, plugin: &str, action: Action) -> Result<(), ActionError> {
        self.world.perform(&action)?;
        self.performed.push((plugin.to_string(), action));
        self.bump();
        Ok(())
    }

    fn perform_value(
        &mut self,
        plugin: &str,
        action: Action,
    ) -> Result<crate::model::ActionValue, ActionError> {
        use crate::model::ActionValue as V;
        let value = match &action {
            Action::OpenDm { user } => {
                let dm = self.dm_channel(*user).ok_or(ActionError::NotFound)?;
                self.world.selection.channel = Some(dm);
                self.world.selection.guild = None;
                V::Id(dm)
            }
            Action::CreateInvite { channel, .. } => {
                if self.world.channel(*channel).is_none() {
                    return Err(ActionError::NotFound);
                }
                V::Code(format!("mock{channel:x}"))
            }
            // The simulated client is in every guild it knows.
            Action::JoinGuild { .. } => return Err(ActionError::NotFound),
            _ => {
                self.world.perform(&action)?;
                V::None
            }
        };
        self.performed.push((plugin.to_string(), action));
        self.bump();
        Ok(value)
    }

    fn history(
        &self,
        channel: u64,
        before: Option<u64>,
        after: Option<u64>,
        limit: u32,
    ) -> Option<Vec<Message>> {
        let all = self
            .world
            .messages
            .get(&channel)
            .map_or(&[][..], |v| &v[..]);
        let page = match after {
            Some(a) => all
                .iter()
                .filter(|m| m.id > a)
                .take(limit as usize)
                .cloned()
                .collect(),
            None => all
                .iter()
                .rev()
                .filter(|m| before.is_none_or(|b| m.id < b))
                .take(limit as usize)
                .cloned()
                .collect(),
        };
        Some(page)
    }

    fn set_view(&mut self, plugin: &str, overrides: crate::model::ViewOverrides) -> bool {
        self.views.retain(|(p, _)| p != plugin);
        if !overrides.is_empty() {
            self.views.push((plugin.to_string(), overrides));
        }
        self.bump();
        true
    }

    // ---------------- UI surfaces ----------------

    fn available_decorations(&self) -> Vec<String> {
        DECORATION_KINDS.map(|(k, _)| k.to_string()).to_vec()
    }

    fn surfaces(&self) -> Vec<String> {
        SURFACES.map(String::from).to_vec()
    }

    fn strings(&self) -> Vec<String> {
        STRINGS.map(String::from).to_vec()
    }

    fn ui_update(&mut self, plugin: &str, op: UiOp) -> Result<(), UiError> {
        let p = plugin.to_string();
        match op {
            // The preview draws neither plugin images nor these looks.
            UiOp::RegisterImage { .. }
            | UiOp::SetBackground { .. }
            | UiOp::SetProfileArt { .. }
            | UiOp::SetWindowOnTop { .. }
            | UiOp::SetWindowOpacity { .. }
            | UiOp::SetString { .. } => {}
            UiOp::SetActive { slot, id, active } => {
                if !self
                    .slot_items
                    .iter()
                    .any(|(q, s, i)| q == plugin && *s == slot && i.id() == id)
                {
                    return Err(UiError::NotFound);
                }
                self.active.insert((p, slot, id), active);
            }
            UiOp::SetVoiceStatus { id, text, since_ms } => {
                if !self.available.iter().any(|s| s == SLOT_VOICE_PANEL_STATUS) {
                    return Err(UiError::Unavailable);
                }
                if text.is_empty() {
                    self.voice_status.remove(&(p, id));
                } else {
                    self.voice_status.insert((p, id), (text, since_ms));
                }
            }
            UiOp::SetSettingsExtra { elements } => {
                self.settings_extra.insert(p, elements);
            }
            UiOp::SetDmSections { sections } => {
                self.dm_sections.insert(p, sections);
            }
            UiOp::ShowNotice { notice } => {
                self.notices.insert(p, notice);
            }
            UiOp::DismissNotice => {
                self.notices.remove(&p);
            }
            UiOp::RegisterKeybind { id, label, default } => {
                self.keybinds
                    .retain(|k| !(k.plugin == plugin && k.id == id));
                self.keybinds.push(Keybind {
                    plugin: p,
                    id,
                    label,
                    default,
                });
            }
            UiOp::UnregisterKeybind { id } => self
                .keybinds
                .retain(|k| !(k.plugin == plugin && k.id == id)),
            UiOp::RegisterSlashCommand { command } => {
                self.slash_commands
                    .retain(|(q, c)| !(q == plugin && c.name == command.name));
                self.slash_commands.push((p, command));
            }
            UiOp::UnregisterSlashCommand { name } => self
                .slash_commands
                .retain(|(q, c)| !(q == plugin && c.name == name)),
            UiOp::RegisterClick { binding } => {
                self.click_bindings
                    .retain(|(q, b)| !(q == plugin && b.id == binding.id));
                self.click_bindings.push((p, binding));
            }
            UiOp::UnregisterClick { id } => self
                .click_bindings
                .retain(|(q, b)| !(q == plugin && b.id == id)),
            UiOp::SetSurface { surface, style } => {
                if !SURFACES.contains(&surface.as_str()) {
                    return Err(UiError::Unavailable);
                }
                let stack = self.surface_styles.entry(surface).or_default();
                stack.retain(|(q, _)| q != plugin);
                stack.push((p, style));
            }
            UiOp::ResetSurface { surface } => {
                if let Some(stack) = self.surface_styles.get_mut(&surface) {
                    stack.retain(|(q, _)| q != plugin);
                }
                self.surface_styles.retain(|_, s| !s.is_empty());
            }
            op @ (UiOp::SetDecoration { .. }
            | UiOp::SetDecorations { .. }
            | UiOp::ClearDecorations { .. }
            | UiOp::SetMessageAccessory { .. }
            | UiOp::SetGhostMessage { .. }
            | UiOp::RemoveGhostMessage { .. }
            | UiOp::ClearAccountState) => self.update_decorations(plugin, op)?,
        }
        self.bump();
        Ok(())
    }

    // ---------------- services ----------------

    fn post_notification(&mut self, n: DesktopNotification) -> bool {
        self.notifications.push(n);
        self.bump();
        true
    }

    fn register_sound(&mut self, _plugin: &str, _id: &str, _data: Vec<u8>) -> Result<(), String> {
        Ok(())
    }

    fn client_sounds(&self) -> Vec<String> {
        [
            "message1",
            "deafen",
            "undeafen",
            "mute",
            "unmute",
            "ptt_start",
            "ptt_stop",
            "user_join",
            "user_leave",
            "call_ringing",
        ]
        .map(String::from)
        .to_vec()
    }

    fn override_sound(&mut self, _plugin: &str, client_sound: &str, _sound: Option<&str>) -> bool {
        self.client_sounds().iter().any(|s| s == client_sound)
    }

    fn speak(&mut self, _plugin: &str, _text: &str) -> bool {
        true
    }

    fn save_file(
        &mut self,
        _plugin: &str,
        _name: &str,
        _content_type: &str,
        _data: Vec<u8>,
    ) -> Result<(), FileError> {
        Ok(())
    }

    /// The preview has no Open dialog; the user always cancels it.
    fn open_file(&mut self, _plugin: &str, _accept: &[String]) -> Result<PickedFile, FileError> {
        Err(FileError::Cancelled)
    }

    /// Each tweak's value: the user's, else the latest preset, else its
    /// default.
    fn tweaks(&self) -> Vec<Tweak> {
        self.tweaks
            .values()
            .map(|t| {
                let mut info = t.info.clone();
                if let Some(v) = t.user.as_ref().or(t.presets.last().map(|(_, v)| v)) {
                    info.value.clone_from(v);
                }
                info
            })
            .collect()
    }

    fn set_tweak(&mut self, plugin: &str, id: &str, value: &str) -> Result<(), TweakError> {
        let t = self.tweaks.get_mut(id).ok_or(TweakError::Unknown)?;
        if !t.info.available {
            return Err(TweakError::Unavailable);
        }
        let valid = match t.info.kind {
            TweakKind::Toggle => matches!(value, "true" | "false"),
            TweakKind::Choice => t.info.choices.iter().any(|c| c == value),
            TweakKind::Number => value.parse::<f64>().is_ok_and(f64::is_finite),
        };
        if !valid {
            return Err(TweakError::InvalidValue);
        }
        if t.user.is_some() {
            return Err(TweakError::UserLocked);
        }
        t.presets.retain(|(p, _)| p != plugin);
        t.presets.push((plugin.to_string(), value.to_string()));
        self.bump();
        Ok(())
    }

    fn reset_tweak(&mut self, plugin: &str, id: &str) {
        if let Some(t) = self.tweaks.get_mut(id) {
            t.presets.retain(|(p, _)| p != plugin);
            self.bump();
        }
    }
}
