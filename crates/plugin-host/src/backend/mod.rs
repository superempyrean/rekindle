//! The client a plugin host drives.
//!
//! [`ClientBackend`] is the seam between the plugin runtime and a client.
//! rekindle's extension implements it inside the client. [`MockBackend`] is
//! an in-memory client with a small sample world of guilds, channels, people
//! and messages, which `rekindle dev` and the test harness run on.
//!
//! All calls carry the calling plugin's id so a backend can layer and clean up
//! per-plugin state. Token names are canonical (the runtime resolves aliases
//! first); see [`crate::tokens`]. The runtime has already checked the
//! capability, gesture, limits and argument shapes before any call lands
//! here; a backend only answers from what its client holds and performs what
//! its build supports.
//!
//! Theming, panels, commands, slots, toasts, modals and `plugin_unloaded`
//! are required. Every other method has a default that answers "nothing
//! loaded" or `unavailable`, so a backend grows into the contract one
//! surface at a time and plugins see exactly which parts work through
//! `host.features()`.

pub mod fixtures;
pub mod gateway;
mod mock;

pub use fixtures::{message as sample_message, sample_world};
pub use mock::{MockBackend, RegisteredFont, World};

use crate::api;
use crate::model::{
    Action, ActionError, ActionValue, Color, DesktopNotification, Element, FileError,
    OverlaySprite, Panel, PickedFile, SlotError, SlotItem, SurfaceStyle, ThemeValue, Tint,
    ToastKind, Tweak, TweakError, UiError, UiOp,
};
use crate::tokens::{ThemeError, TokenInfo};

pub trait ClientBackend: Send {
    // ===================== theming (0.7.0) =====================

    /// Canonical tokens this client exposes, sorted by name.
    fn theme_tokens(&self) -> Vec<TokenInfo>;
    /// Effective value (base + tint + overrides) of a canonical token.
    fn get_value(&self, token: &str) -> Option<ThemeValue>;
    /// Override a canonical token. The backend checks the token exists and
    /// the value fits it.
    fn set_value(&mut self, plugin: &str, token: &str, value: ThemeValue)
    -> Result<(), ThemeError>;
    fn reset_value(&mut self, plugin: &str, token: &str);
    /// Set or clear this plugin's whole-theme tint.
    fn set_tint(&mut self, plugin: &str, tint: Option<Tint>);
    /// Make a font (already size/format-checked by the runtime) available
    /// under `family`.
    fn register_font(&mut self, plugin: &str, family: &str, data: Vec<u8>) -> Result<(), String>;

    // ===================== panels, commands, slots (0.7.0) =====================

    fn show_panel(&mut self, plugin: &str, panel: Panel);
    fn remove_panel(&mut self, plugin: &str, panel_id: &str);
    fn register_command(&mut self, plugin: &str, id: &str, title: &str);
    fn unregister_command(&mut self, plugin: &str, id: &str);

    /// Slot names this client build supports (hook point found).
    fn available_slots(&self) -> Vec<String>;
    /// Add or replace (by plugin + id) an item in `slot`. The runtime has
    /// already validated labels, ids, the item kind for the slot and
    /// per-plugin limits.
    fn add_slot_item(&mut self, plugin: &str, slot: &str, item: SlotItem) -> Result<(), SlotError>;
    fn remove_slot_item(&mut self, plugin: &str, slot: &str, id: &str);
    /// Content of a plugin's profile section for one user.
    fn set_profile_section(
        &mut self,
        plugin: &str,
        section: &str,
        user: u64,
        elements: Vec<Element>,
    );
    fn show_toast(&mut self, plugin: &str, text: &str, kind: ToastKind);
    /// One modal per plugin; showing again replaces it.
    fn show_modal(&mut self, plugin: &str, panel: Panel);
    fn close_modal(&mut self, plugin: &str);

    // ===================== overlay, clipboard (0.7.0) =====================
    // The runtime has decoded and size-checked the image and enforced limits.
    // Defaults are no-ops so a build without an overlay layer still loads the
    // plugin (its sprites simply don't appear).

    /// Register a decoded PNG atlas; `w`/`h` are its pixel dimensions.
    fn overlay_register_image(
        &mut self,
        _plugin: &str,
        _id: &str,
        _png: Vec<u8>,
        _w: u32,
        _h: u32,
    ) {
    }
    fn overlay_remove_image(&mut self, _plugin: &str, _id: &str) {}
    /// Add or move (by plugin + instance) a sprite.
    fn overlay_draw(&mut self, _sprite: OverlaySprite) {}
    fn overlay_clear(&mut self, _plugin: &str, _instance: &str) {}
    /// Client viewport in logical pixels, for clamping sprites.
    fn viewport(&self) -> (f32, f32) {
        (0.0, 0.0)
    }

    /// Put `text` on the system clipboard. The runtime has checked the
    /// capability, the size cap and that a user gesture is being delivered.
    fn write_clipboard(&mut self, _plugin: &str, _text: &str) {}
    /// Put a PNG on the clipboard (same checks).
    fn write_clipboard_png(&mut self, _plugin: &str, _png: Vec<u8>) -> bool {
        false
    }

    /// Drop everything a plugin contributed. Called after `deactivate`, and
    /// when a plugin faults.
    fn plugin_unloaded(&mut self, plugin: &str);

    /// Convenience: effective color of a color token.
    fn get_color(&self, token: &str) -> Option<Color> {
        self.get_value(token).and_then(|v| v.as_color())
    }

    // ===================== the client (0.8.0) =====================

    /// What the client is. `host-version` is filled in by the runtime.
    fn client_info(&self) -> api::ClientInfo {
        api::ClientInfo {
            host_version: String::new(),
            client: "unknown".into(),
            client_build: String::new(),
            platform: std::env::consts::OS.into(),
            locale: "en-US".into(),
            base_theme: "dark".into(),
            reduced_motion: false,
            streamer_mode: false,
        }
    }

    /// Hook points beyond slots/decorations/surfaces that resolved on this
    /// build (state queries, event kinds, ...), merged into `host.features()`.
    fn extra_features(&self) -> Vec<String> {
        Vec::new()
    }

    /// Format an epoch-ms time in the user's locale and timezone. The
    /// default is the runtime's (UTC, en-US); a real client overrides it.
    fn format_time(&self, _epoch_ms: u64, _style: api::TimeStyle) -> Option<String> {
        None
    }

    // ===================== client-state (read-client) =====================

    fn selection(&self) -> api::Selection {
        api::Selection {
            guild: None,
            channel: None,
            voice_channel: None,
            side_thread: None,
            focused: true,
            idle: false,
        }
    }
    fn me(&self) -> Option<api::User> {
        None
    }
    fn my_status(&self) -> String {
        "online".into()
    }
    fn my_custom_status(&self) -> Option<api::CustomStatus> {
        None
    }
    fn user(&self, _id: u64) -> Option<api::User> {
        None
    }
    fn guild(&self, _id: u64) -> Option<api::Guild> {
        None
    }
    fn guild_ids(&self) -> Vec<u64> {
        Vec::new()
    }
    fn guild_folders(&self) -> Vec<api::GuildFolder> {
        Vec::new()
    }
    fn channel(&self, _id: u64) -> Option<api::Channel> {
        None
    }
    fn channels(&self, _guild: u64, _include_hidden: bool) -> Vec<api::Channel> {
        Vec::new()
    }
    fn threads(&self, _parent: u64) -> Vec<api::Channel> {
        Vec::new()
    }
    fn private_channels(&self) -> Vec<api::Channel> {
        Vec::new()
    }
    fn dm_channel(&self, _user: u64) -> Option<u64> {
        None
    }
    fn role(&self, _guild: u64, _id: u64) -> Option<api::Role> {
        None
    }
    fn roles(&self, _guild: u64) -> Vec<api::Role> {
        Vec::new()
    }
    fn guild_emoji(&self, _guild: u64) -> Vec<api::Emoji> {
        Vec::new()
    }
    fn frequent_emoji(&self, _limit: u32) -> Vec<api::Emoji> {
        Vec::new()
    }
    fn guild_stickers(&self, _guild: u64) -> Vec<api::Sticker> {
        Vec::new()
    }
    fn my_member(&self, _guild: u64) -> Option<api::Member> {
        None
    }
    fn my_permissions(&self, _channel_or_guild: u64) -> u64 {
        0
    }
    fn permission_overwrites(&self, _channel: u64) -> Vec<api::Overwrite> {
        Vec::new()
    }
    fn read_state(&self, _channel: u64) -> Option<api::ReadState> {
        None
    }
    fn guild_read_state(&self, guild: u64) -> api::GuildReadState {
        api::GuildReadState {
            guild,
            unread: false,
            mentions: 0,
        }
    }
    fn unread_channels(&self, _limit: u32) -> Vec<api::ReadState> {
        Vec::new()
    }
    fn my_voice(&self) -> Option<api::MyVoice> {
        None
    }
    /// One of the curated setting keys (the runtime filters the allowlist).
    fn my_setting(&self, _key: &str) -> Option<String> {
        None
    }
    fn my_setting_keys(&self) -> Vec<String> {
        Vec::new()
    }

    // ===================== users (read-users) =====================

    fn presence(&self, _user: u64, _guild: Option<u64>) -> Option<api::Presence> {
        None
    }
    fn member(&self, _guild: u64, _user: u64) -> Option<api::Member> {
        None
    }
    /// Loaded members, up to `limit`, and how many are loaded in total.
    fn loaded_members(&self, _guild: u64, _limit: u32) -> (Vec<api::Member>, u32) {
        (Vec::new(), 0)
    }
    fn member_permissions(&self, _user: u64, _channel_or_guild: u64) -> Option<u64> {
        None
    }
    fn member_color(&self, _guild: u64, _user: u64) -> Option<Color> {
        None
    }
    fn typing(&self, _channel: u64) -> Vec<u64> {
        Vec::new()
    }
    fn voice_states(&self, _channel: u64) -> Vec<api::VoiceState> {
        Vec::new()
    }
    fn voice_state(&self, _user: u64) -> Option<api::VoiceState> {
        None
    }
    fn speaking(&self, _user: u64) -> bool {
        false
    }
    fn relationships(&self) -> Vec<api::Relationship> {
        Vec::new()
    }
    fn note(&self, _user: u64) -> Option<String> {
        None
    }
    fn mutual_guilds(&self, _user: u64) -> Vec<u64> {
        Vec::new()
    }

    // ===================== messages (read-messages) =====================

    fn message(&self, _channel: u64, _id: u64) -> Option<api::Message> {
        None
    }
    /// Newest first, older than `before` if given, at most `limit`.
    fn loaded_messages(
        &self,
        _channel: u64,
        _before: Option<u64>,
        _limit: u32,
    ) -> Vec<api::Message> {
        Vec::new()
    }
    fn visible_messages(&self) -> Vec<api::MessageRef> {
        Vec::new()
    }
    fn pins(&self, _channel: u64) -> Vec<api::MessageRef> {
        Vec::new()
    }
    /// Is a message containing this link on screen right now?
    fn link_on_screen(&self, _url: &str) -> bool {
        false
    }
    /// The fetch the client makes when the user clicks a message link: one
    /// message, from a channel the user can open. `Some(None)`: not found;
    /// `None`: this build can't fetch.
    fn fetch_linked_message(
        &mut self,
        _guild: Option<u64>,
        _channel: u64,
        _id: u64,
    ) -> Option<Option<api::Message>> {
        None
    }

    /// The client's own model of a target as JSON, for `inspect.raw-json`.
    /// The runtime strips secret-looking fields again before a plugin sees it.
    fn raw_json(&self, _target: &api::Target) -> Option<String> {
        None
    }

    // ===================== acting =====================

    /// Perform an action as the user. The runtime has checked capability,
    /// gesture budget and rate limits; the backend checks Discord's
    /// permissions (`Forbidden`) and whether the target is loaded.
    fn perform(&mut self, _plugin: &str, _action: Action) -> Result<(), ActionError> {
        Err(ActionError::Unavailable)
    }

    /// Perform an action that returns something: `open-dm`, `create-invite`
    /// and `join-guild`. Checked like [`Self::perform`].
    fn perform_value(&mut self, plugin: &str, action: Action) -> Result<ActionValue, ActionError> {
        self.perform(plugin, action).map(|()| ActionValue::None)
    }

    /// A page of a channel's history past what the client has loaded:
    /// newest first older than `before`, or oldest first newer than
    /// `after`. `None`: this build can't read its history. The real client
    /// should read its own message store.
    fn history(
        &self,
        _channel: u64,
        _before: Option<u64>,
        _after: Option<u64>,
        _limit: u32,
    ) -> Option<Vec<api::Message>> {
        None
    }

    /// Replace `plugin`'s `view` overrides (empty: drop them). The backend
    /// lays every plugin's overrides over users, members and the user's
    /// settings wherever the client draws or reads them, in the order they
    /// were set. `false`: this build can't. The real client should apply
    /// them where it renders and decides from those records.
    fn set_view(&mut self, _plugin: &str, _overrides: crate::model::ViewOverrides) -> bool {
        false
    }

    // ===================== 0.8.0 UI surfaces =====================

    /// Decoration kinds this build can draw.
    fn available_decorations(&self) -> Vec<String> {
        Vec::new()
    }
    /// Named layout surfaces this build can restyle.
    fn surfaces(&self) -> Vec<String> {
        Vec::new()
    }
    /// The keys of the client's own strings plugins may replace
    /// (`theming.set-string`).
    fn strings(&self) -> Vec<String> {
        Vec::new()
    }
    /// Apply a newer UI update (decorations, accessories, keybinds, slash
    /// commands, click bindings, surfaces, ...). See [`UiOp`].
    fn ui_update(&mut self, _plugin: &str, _op: UiOp) -> Result<(), UiError> {
        Err(UiError::Unavailable)
    }
    /// Show a theming surface style (convenience over `ui_update`).
    fn set_surface(
        &mut self,
        plugin: &str,
        surface: &str,
        style: SurfaceStyle,
    ) -> Result<(), UiError> {
        self.ui_update(
            plugin,
            UiOp::SetSurface {
                surface: surface.into(),
                style,
            },
        )
    }

    // ===================== services =====================

    /// Post a desktop notification (rate limit and label already applied).
    /// `false` if this build can't show one.
    fn post_notification(&mut self, _n: DesktopNotification) -> bool {
        false
    }
    /// The client is in Do Not Disturb / quiet hours.
    fn notifications_muted(&self) -> bool {
        false
    }

    fn register_sound(&mut self, _plugin: &str, _id: &str, _data: Vec<u8>) -> Result<(), String> {
        Err("audio unavailable on this client build".into())
    }
    fn play_sound(&mut self, _plugin: &str, _id: &str, _volume: f32) {}
    fn stop_sound(&mut self, _plugin: &str, _id: &str) {}
    fn client_sounds(&self) -> Vec<String> {
        Vec::new()
    }
    fn override_sound(&mut self, _plugin: &str, _client_sound: &str, _sound: Option<&str>) -> bool {
        false
    }
    fn speak(&mut self, _plugin: &str, _text: &str) -> bool {
        false
    }

    /// Show a Save dialog and write `data` where the user chose.
    fn save_file(
        &mut self,
        _plugin: &str,
        _name: &str,
        _content_type: &str,
        _data: Vec<u8>,
    ) -> Result<(), FileError> {
        Err(FileError::Failed)
    }
    /// Show an Open dialog and return the picked file.
    fn open_file(&mut self, _plugin: &str, _accept: &[String]) -> Result<PickedFile, FileError> {
        Err(FileError::Failed)
    }

    fn tweaks(&self) -> Vec<Tweak> {
        Vec::new()
    }
    fn set_tweak(&mut self, _plugin: &str, _id: &str, _value: &str) -> Result<(), TweakError> {
        Err(TweakError::Unknown)
    }
    fn reset_tweak(&mut self, _plugin: &str, _id: &str) {}
}
