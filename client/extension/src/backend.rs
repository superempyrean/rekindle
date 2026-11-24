//! The [`ClientBackend`] the plugin runtime drives inside the client.
//!
//! [`DetourBackend`] holds what plugins contributed and hands it to the
//! detours that put it on screen: theme colours to [`ThemeState`] (the theme
//! detour and framework recolouring read it), commands and chat-bar buttons
//! to the [`Driver`](crate::Driver), which publishes them, and menu rows,
//! profile sections, panels, modals and toasts straight to
//! `rekindle-ui-hook`. Overlay sprites go to [`OverlayState`], and the
//! styles of named surfaces to [`SurfaceStyles`], which the host puts on a
//! client's wire.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use rekindle_plugin_host::backend::ClientBackend;
use rekindle_plugin_host::model::{
    Action, ActionError, Element, OverlaySprite, Panel, SLOT_CHANNEL_MENU, SLOT_CHAT_BAR_BUTTON,
    SLOT_MESSAGE_MENU, SLOT_PROFILE_SECTION, SLOT_USER_MENU, SlotError, SlotItem, SurfaceStyle,
    ThemeValue, Tint, ToastKind, UiError, UiOp,
};
use rekindle_plugin_host::tokens::{self, ThemeError, TokenInfo};
use rekindle_ui_hook::registry::Surfaces;
use rekindle_ui_hook::{Button, MenuItem, MenuKind, PanelItem, ProfileSectionItem};

use crate::convert::{glyph, lines_of, widgets_of};
use crate::key;
use crate::overlay::OverlayState;
use crate::theme::ThemeState;

/// A command a plugin registered, with its routing key.
#[derive(Clone)]
pub(crate) struct CommandReg {
    pub(crate) plugin: String,
    pub(crate) key: String,
    pub(crate) title: String,
}

/// The state the [`Driver`](crate::Driver) shares with the backend.
#[derive(Clone)]
pub(crate) struct Shared {
    pub(crate) theme: Arc<Mutex<ThemeState>>,
    pub(crate) commands: Arc<Mutex<Vec<CommandReg>>>,
    /// Chat-bar buttons, as (plugin, button).
    pub(crate) buttons: Arc<Mutex<Vec<(String, Button)>>>,
    pub(crate) overlay: Arc<Mutex<OverlayState>>,
    pub(crate) surfaces: Arc<Mutex<SurfaceStyles>>,
    /// Slash commands, as (plugin, command). `None` when the host runs
    /// none.
    pub(crate) slash: Arc<Mutex<Option<SlashCommands>>>,
}

/// The plugins' slash commands, as (plugin, command).
pub(crate) type SlashCommands = Vec<(String, rekindle_plugin_host::api::SlashCommand)>;

/// The plugins' styles for the client's named surfaces
/// (`theming.set-surface`).
#[derive(Default)]
pub(crate) struct SurfaceStyles {
    /// The surfaces this build can restyle. Empty when it can restyle none.
    pub(crate) available: Vec<String>,
    /// Per surface, (plugin, style) in the order they were set. The last
    /// one applies.
    pub(crate) styles: BTreeMap<String, Vec<(String, SurfaceStyle)>>,
}

impl SurfaceStyles {
    fn update(&mut self, plugin: &str, op: UiOp) -> Result<(), UiError> {
        match op {
            UiOp::SetSurface { surface, style } => {
                if self.available.is_empty() {
                    return Err(UiError::Unavailable);
                }
                if !self.available.contains(&surface) {
                    return Err(UiError::NotFound);
                }
                let stack = self.styles.entry(surface).or_default();
                stack.retain(|(p, _)| p != plugin);
                stack.push((plugin.to_string(), style));
                Ok(())
            }
            UiOp::ResetSurface { surface } => {
                if let Some(stack) = self.styles.get_mut(&surface) {
                    stack.retain(|(p, _)| p != plugin);
                }
                self.styles.retain(|_, s| !s.is_empty());
                Ok(())
            }
            _ => Err(UiError::Unavailable),
        }
    }

    fn plugin_unloaded(&mut self, plugin: &str) {
        for stack in self.styles.values_mut() {
            stack.retain(|(p, _)| p != plugin);
        }
        self.styles.retain(|_, s| !s.is_empty());
    }

    /// The style that applies to each surface a plugin restyled.
    pub(crate) fn applied(&self) -> Vec<(String, SurfaceStyle)> {
        self.styles
            .iter()
            .filter_map(|(s, stack)| stack.last().map(|(_, style)| (s.clone(), *style)))
            .collect()
    }
}

/// rekindle's `ClientBackend` inside the client, built from recovered layout.
pub(crate) struct DetourBackend {
    shared: Shared,
    /// The canonical tokens the client's theme has an offset for.
    tokens: Vec<TokenInfo>,
    /// Profile section titles, from slot items: (plugin, section) -> title.
    profile_titles: BTreeMap<(String, String), String>,
    /// Profile sections with their content, from `set_profile_section`.
    profiles: Vec<ProfileSectionItem>,
    panels: Vec<PanelItem>,
    /// Context-menu rows, as (plugin, row); `None` when rekindle isn't in
    /// this build's menus registry.
    menus: Option<Vec<(String, MenuItem)>>,
    /// rekindle is in the profile popout's registry.
    profiles_available: bool,
}

impl DetourBackend {
    /// `wire_menus`: the host draws the menu rows on the client's wire.
    pub(crate) fn new(shared: Shared, surfaces: Surfaces, wire_menus: bool) -> Self {
        let names = shared.theme.lock().unwrap().token_names();
        let tokens = tokens::default_tokens()
            .into_iter()
            .map(|(i, _)| i)
            .filter(|i| names.iter().any(|n| n == &i.name))
            .collect();
        Self {
            shared,
            tokens,
            profile_titles: BTreeMap::new(),
            profiles: Vec::new(),
            panels: Vec::new(),
            menus: (surfaces.menus || wire_menus).then(Vec::new),
            profiles_available: surfaces.profiles,
        }
    }

    fn publish_menus(&self) {
        if let Some(rows) = &self.menus {
            rekindle_ui_hook::set_menu_items(rows.iter().map(|(_, r)| r.clone()).collect());
        }
    }

    fn add_button(&self, plugin: &str, id: &str, glyph: &str, tooltip: &str) {
        let button = Button {
            key: key(plugin, id),
            glyph: glyph.to_string(),
            tooltip: format!("{tooltip} · {plugin}"),
        };
        let mut b = self.shared.buttons.lock().unwrap();
        b.retain(|(p, x)| !(p == plugin && x.key == button.key));
        b.push((plugin.to_string(), button));
    }

    fn add_menu_row(
        &mut self,
        plugin: &str,
        slot: &str,
        id: &str,
        label: &str,
        danger: bool,
    ) -> Result<(), SlotError> {
        let Some(menus) = &mut self.menus else {
            return Err(SlotError::Unavailable);
        };
        let kind = match slot {
            SLOT_MESSAGE_MENU => MenuKind::Message,
            SLOT_USER_MENU => MenuKind::User,
            _ => MenuKind::Channel,
        };
        let row = MenuItem {
            key: key(plugin, id),
            kind,
            label: label.to_string(),
            danger,
        };
        menus.retain(|(_, r)| r.key != row.key);
        menus.push((plugin.to_string(), row));
        self.publish_menus();
        Ok(())
    }
}

impl ClientBackend for DetourBackend {
    // ---- client state: read from the client's memory, on its UI thread ----

    fn selection(&self) -> rekindle_plugin_host::api::Selection {
        let (guild, channel) = client::read(client::Client::selection).unwrap_or((None, 0));
        rekindle_plugin_host::api::Selection {
            guild,
            channel: (channel != 0).then_some(channel),
            voice_channel: None,
            side_thread: None,
            focused: true,
            idle: false,
        }
    }

    fn user(&self, id: u64) -> Option<rekindle_plugin_host::api::User> {
        client::read(move |c| c.user(id))
            .flatten()
            .map(|u| user_record(&u))
    }

    fn guild_ids(&self) -> Vec<u64> {
        client::read(client::Client::guild_ids).unwrap_or_default()
    }

    fn guild(&self, id: u64) -> Option<rekindle_plugin_host::api::Guild> {
        let g = client::read(move |c| c.guild(id)).flatten()?;
        Some(rekindle_plugin_host::api::Guild {
            id: g.id,
            name: g.name,
            owner: g.owner,
            description: None,
            features: vec![],
            premium_tier: 0,
            premium_subscribers: 0,
            verification_level: 0,
            nsfw_level: 0,
            preferred_locale: "en-US".into(),
            member_count: Some(g.members),
            joined_ms: None,
            muted: false,
            ..Default::default()
        })
    }

    fn roles(&self, guild: u64) -> Vec<rekindle_plugin_host::api::Role> {
        client::read(move |c| c.roles(guild))
            .unwrap_or_default()
            .into_iter()
            .map(|r| role_record(guild, r))
            .collect()
    }

    fn role(&self, guild: u64, id: u64) -> Option<rekindle_plugin_host::api::Role> {
        self.roles(guild).into_iter().find(|r| r.id == id)
    }

    fn member(&self, guild: u64, user: u64) -> Option<rekindle_plugin_host::api::Member> {
        client::read(move |c| c.member(guild, user))
            .flatten()
            .map(|m| member_record(guild, m))
    }

    fn loaded_members(
        &self,
        guild: u64,
        limit: u32,
    ) -> (Vec<rekindle_plugin_host::api::Member>, u32) {
        let all = client::read(move |c| c.members(guild)).unwrap_or_default();
        let total = all.len() as u32;
        let members = all
            .into_iter()
            .take(limit as usize)
            .map(|m| member_record(guild, m))
            .collect();
        (members, total)
    }

    fn member_permissions(&self, user: u64, channel_or_guild: u64) -> Option<u64> {
        client::read(move |c| permissions(c, channel_or_guild, user)).flatten()
    }

    fn member_color(&self, guild: u64, user: u64) -> Option<rekindle_plugin_host::model::Color> {
        let rgb = client::read(move |c| {
            let roles = c.roles(guild);
            let mine = c.member(guild, user)?.roles;
            roles
                .into_iter()
                .find(|r| r.color != 0 && mine.contains(&r.id))
                .map(|r| r.color)
        })
        .flatten()?;
        let [_, r, g, b] = rgb.to_be_bytes();
        Some(rekindle_plugin_host::model::Color { r, g, b, a: 255 })
    }

    fn theme_tokens(&self) -> Vec<TokenInfo> {
        self.tokens.clone()
    }

    fn get_value(&self, token: &str) -> Option<ThemeValue> {
        self.shared
            .theme
            .lock()
            .unwrap()
            .get_value(tokens::canonical(token))
    }

    fn set_value(
        &mut self,
        plugin: &str,
        token: &str,
        value: ThemeValue,
    ) -> Result<(), ThemeError> {
        self.shared
            .theme
            .lock()
            .unwrap()
            .set_value(plugin, tokens::canonical(token), &value)
    }

    fn reset_value(&mut self, plugin: &str, token: &str) {
        self.shared
            .theme
            .lock()
            .unwrap()
            .reset_value(plugin, tokens::canonical(token));
    }

    /// For now, only colour tokens are applied, so a tint has no effect.
    fn set_tint(&mut self, _plugin: &str, _tint: Option<Tint>) {}

    /// For now, fonts aren't applied: the family is accepted and unused.
    fn register_font(
        &mut self,
        _plugin: &str,
        _family: &str,
        _data: Vec<u8>,
    ) -> Result<(), String> {
        Ok(())
    }

    /// Re-showing a panel replaces it in place, keeping its position.
    fn show_panel(&mut self, plugin: &str, panel: Panel) {
        let item = PanelItem {
            plugin: plugin.to_string(),
            id: panel.id,
            title: panel.title,
            widgets: widgets_of(&panel.elements),
        };
        match self
            .panels
            .iter_mut()
            .find(|p| p.plugin == plugin && p.id == item.id)
        {
            Some(slot) => *slot = item,
            None => self.panels.push(item),
        }
        rekindle_ui_hook::set_panels(self.panels.clone());
    }

    fn remove_panel(&mut self, plugin: &str, panel_id: &str) {
        self.panels
            .retain(|p| !(p.plugin == plugin && p.id == panel_id));
        rekindle_ui_hook::set_panels(self.panels.clone());
    }

    fn register_command(&mut self, plugin: &str, id: &str, title: &str) {
        let key = key(plugin, id);
        let mut cmds = self.shared.commands.lock().unwrap();
        cmds.retain(|c| c.key != key);
        cmds.push(CommandReg {
            plugin: plugin.to_string(),
            key,
            title: title.to_string(),
        });
    }

    fn unregister_command(&mut self, plugin: &str, id: &str) {
        let key = key(plugin, id);
        self.shared
            .commands
            .lock()
            .unwrap()
            .retain(|c| c.key != key);
    }

    fn available_slots(&self) -> Vec<String> {
        let mut slots = vec![SLOT_CHAT_BAR_BUTTON.to_string()];
        if self.profiles_available {
            slots.push(SLOT_PROFILE_SECTION.to_string());
        }
        if self.menus.is_some() {
            slots.extend([SLOT_MESSAGE_MENU, SLOT_USER_MENU, SLOT_CHANNEL_MENU].map(String::from));
        }
        slots
    }

    fn add_slot_item(&mut self, plugin: &str, slot: &str, item: SlotItem) -> Result<(), SlotError> {
        match (slot, item) {
            (
                SLOT_CHAT_BAR_BUTTON,
                SlotItem::IconButton {
                    id, icon, tooltip, ..
                },
            ) => {
                self.add_button(plugin, &id, glyph(icon), &tooltip);
                Ok(())
            }
            (SLOT_PROFILE_SECTION, SlotItem::ListItem { .. }) if !self.profiles_available => {
                Err(SlotError::Unavailable)
            }
            (SLOT_PROFILE_SECTION, SlotItem::ListItem { id, title }) => {
                self.profile_titles.insert((plugin.to_string(), id), title);
                Ok(())
            }
            (
                SLOT_MESSAGE_MENU | SLOT_USER_MENU | SLOT_CHANNEL_MENU,
                SlotItem::MenuItem {
                    id, label, danger, ..
                },
            ) => self.add_menu_row(plugin, slot, &id, &label, danger),
            (
                SLOT_CHAT_BAR_BUTTON | SLOT_PROFILE_SECTION | SLOT_MESSAGE_MENU | SLOT_USER_MENU
                | SLOT_CHANNEL_MENU,
                _,
            ) => Err(SlotError::Invalid),
            _ => Err(SlotError::Unavailable),
        }
    }

    fn remove_slot_item(&mut self, plugin: &str, _slot: &str, id: &str) {
        let key = key(plugin, id);
        self.shared
            .buttons
            .lock()
            .unwrap()
            .retain(|(_, b)| b.key != key);
        if let Some(menus) = &mut self.menus {
            menus.retain(|(_, r)| r.key != key);
            self.publish_menus();
        }
    }

    fn set_profile_section(
        &mut self,
        plugin: &str,
        section: &str,
        _user: u64,
        elements: Vec<Element>,
    ) {
        let title = self
            .profile_titles
            .get(&(plugin.to_string(), section.to_string()))
            .cloned()
            .unwrap_or_else(|| section.to_string());
        self.profiles
            .retain(|i| !(i.plugin == plugin && i.id == section));
        self.profiles.push(ProfileSectionItem {
            plugin: plugin.to_string(),
            id: section.to_string(),
            title,
            lines: lines_of(&elements),
        });
        rekindle_ui_hook::set_profiles(self.profiles.clone());
    }

    fn show_toast(&mut self, _plugin: &str, text: &str, kind: ToastKind) {
        let style = match kind {
            ToastKind::Info => rekindle_ui_hook::ToastStyle::Info,
            ToastKind::Success => rekindle_ui_hook::ToastStyle::Success,
            ToastKind::Warning => rekindle_ui_hook::ToastStyle::Warning,
            ToastKind::Error => rekindle_ui_hook::ToastStyle::Error,
        };
        rekindle_ui_hook::push_toast(text, style);
    }

    fn show_modal(&mut self, plugin: &str, panel: Panel) {
        rekindle_ui_hook::set_modal(Some(rekindle_ui_hook::Modal {
            plugin: plugin.to_string(),
            key: plugin.to_string(),
            title: panel.title.clone(),
            panel: PanelItem {
                plugin: plugin.to_string(),
                id: panel.id,
                title: panel.title,
                widgets: widgets_of(&panel.elements),
            },
        }));
    }

    fn close_modal(&mut self, _plugin: &str) {
        rekindle_ui_hook::set_modal(None);
    }

    fn overlay_register_image(&mut self, plugin: &str, id: &str, png: Vec<u8>, _w: u32, _h: u32) {
        self.shared
            .overlay
            .lock()
            .unwrap()
            .register_image(plugin, id, &png);
    }

    fn overlay_remove_image(&mut self, plugin: &str, id: &str) {
        self.shared.overlay.lock().unwrap().remove_image(plugin, id);
    }

    fn overlay_draw(&mut self, sprite: OverlaySprite) {
        self.shared.overlay.lock().unwrap().draw(sprite);
    }

    fn overlay_clear(&mut self, plugin: &str, instance: &str) {
        self.shared.overlay.lock().unwrap().clear(plugin, instance);
    }

    fn viewport(&self) -> (f32, f32) {
        self.shared.overlay.lock().unwrap().viewport()
    }

    /// The runtime has checked the gesture and the size. The text waits for
    /// the client's UI thread, which owns the clipboard.
    fn write_clipboard(&mut self, _plugin: &str, text: &str) {
        rekindle_ui_hook::queue_clipboard(text.to_string());
    }

    /// For now, `insert-text` is the only action. It goes into the text
    /// field the user last typed in, at its caret.
    fn perform(&mut self, _plugin: &str, action: Action) -> Result<(), ActionError> {
        match action {
            Action::InsertText { text } => {
                rekindle_ui_hook::queue_insert(text);
                Ok(())
            }
            _ => Err(ActionError::Unavailable),
        }
    }

    fn surfaces(&self) -> Vec<String> {
        self.shared.surfaces.lock().unwrap().available.clone()
    }

    fn ui_update(&mut self, plugin: &str, op: UiOp) -> Result<(), UiError> {
        match op {
            UiOp::RegisterSlashCommand { command } => {
                let mut slash = self.shared.slash.lock().unwrap();
                let list = slash.as_mut().ok_or(UiError::Unavailable)?;
                list.retain(|(p, c)| !(p == plugin && c.name == command.name));
                list.push((plugin.to_string(), command));
                Ok(())
            }
            UiOp::UnregisterSlashCommand { name } => {
                if let Some(list) = self.shared.slash.lock().unwrap().as_mut() {
                    list.retain(|(p, c)| !(p == plugin && c.name == name));
                }
                Ok(())
            }
            op => self.shared.surfaces.lock().unwrap().update(plugin, op),
        }
    }

    fn plugin_unloaded(&mut self, plugin: &str) {
        self.shared.surfaces.lock().unwrap().plugin_unloaded(plugin);
        if let Some(list) = self.shared.slash.lock().unwrap().as_mut() {
            list.retain(|(p, _)| p != plugin);
        }
        self.shared.overlay.lock().unwrap().plugin_unloaded(plugin);
        self.shared.theme.lock().unwrap().plugin_unloaded(plugin);
        self.shared
            .commands
            .lock()
            .unwrap()
            .retain(|c| c.plugin != plugin);
        self.shared
            .buttons
            .lock()
            .unwrap()
            .retain(|(p, _)| p != plugin);
        self.profile_titles.retain(|(p, _), _| p != plugin);
        self.profiles.retain(|i| i.plugin != plugin);
        rekindle_ui_hook::set_profiles(self.profiles.clone());
        self.panels.retain(|p| p.plugin != plugin);
        rekindle_ui_hook::set_panels(self.panels.clone());
        if let Some(menus) = &mut self.menus {
            menus.retain(|(p, _)| p != plugin);
            self.publish_menus();
        }
    }
}

use rekindle_ui_hook::client;

/// Discord's Administrator bit.
const ADMINISTRATOR: u64 = 1 << 3;
/// Every permission bit.
const ALL_PERMISSIONS: u64 = (1 << 50) - 1;

/// `user`'s permissions in a server, as Discord works them out: the owner
/// and administrators have every bit, everyone else `@everyone`'s and
/// their roles'. Channel overwrites aren't kept by this client.
fn permissions(c: &client::Client, guild: u64, user: u64) -> Option<u64> {
    let g = c.guild(guild)?;
    if g.owner == user {
        return Some(ALL_PERMISSIONS);
    }
    let roles = c.roles(guild);
    let everyone = roles
        .iter()
        .find(|r| r.id == guild)
        .map_or(0, |r| r.permissions);
    let mine = c.member(guild, user)?.roles;
    let bits = roles
        .iter()
        .filter(|r| mine.contains(&r.id))
        .fold(everyone, |a, r| a | r.permissions);
    Some(if bits & ADMINISTRATOR != 0 {
        ALL_PERMISSIONS
    } else {
        bits
    })
}

fn user_record(u: &client::UserRead) -> rekindle_plugin_host::api::User {
    rekindle_plugin_host::api::User {
        id: u.id,
        username: u.username.clone(),
        global_name: (u.display != u.username).then(|| u.display.clone()),
        bot: u.bot,
        system: false,
        public_flags: 0,
        accent: None,
        has_decoration: false,
        ..Default::default()
    }
}

fn role_record(guild: u64, r: client::RoleRead) -> rekindle_plugin_host::api::Role {
    let [_, red, green, blue] = r.color.to_be_bytes();
    rekindle_plugin_host::api::Role {
        id: r.id,
        guild,
        name: r.name,
        color: (r.color != 0).then_some(rekindle_plugin_host::api::WitColor {
            r: red,
            g: green,
            b: blue,
            a: 255,
        }),
        colors_extra: vec![],
        position: r.position,
        permissions: r.permissions,
        hoist: r.hoist,
        mentionable: false,
        managed: false,
        unicode_emoji: None,
        ..Default::default()
    }
}

fn member_record(guild: u64, m: client::MemberRead) -> rekindle_plugin_host::api::Member {
    rekindle_plugin_host::api::Member {
        guild,
        user: m.user,
        nick: m.nick,
        roles: m.roles,
        joined_ms: None,
        boosting_since_ms: None,
        timed_out_until_ms: None,
        pending: false,
        ..Default::default()
    }
}
