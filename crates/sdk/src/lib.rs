//! # rekindle-sdk
//!
//! Write a sandboxed rekindle plugin in a few lines:
//!
//! ```no_run
//! use rekindle_sdk::prelude::*;
//!
//! struct Midnight;
//!
//! impl Plugin for Midnight {
//!     fn activate(_granted: &Granted) -> Result<Self, String> {
//!         theme::set("background-primary", Color::rgb(0x0b, 0x0d, 0x12));
//!         info!("midnight applied");
//!         Ok(Midnight)
//!     }
//! }
//!
//! rekindle_sdk::plugin!(Midnight);
//! ```
//!
//! Declare the plugin's identity, capabilities and settings in `Cargo.toml`:
//!
//! ```toml
//! [package.metadata.rekindle]
//! id = "dev.you.midnight"
//! name = "Midnight"
//! capabilities = ["theming"]
//! # network = ["api.example.com"]   # only with the `network` capability
//!
//! [[package.metadata.rekindle.settings]]
//! key = "dim"
//! type = "toggle"
//! label = "Dim the sidebar"
//! default = true
//! ```
//!
//! then `rekindle build` (or `rekindle dev` for a live preview).
//!
//! What a plugin can do, by module, and the capability each needs:
//!
//! | Module | Capability | |
//! |---|---|---|
//! | [`state`] | `read-client` | where the user is, guilds, channels, roles, emoji, the user's member, read states, voice |
//! | [`users`] | `read-users` | presence, members, typing, voice states, relationships, notes |
//! | [`messages`] | `read-messages` | loaded and visible messages, pins, message links |
//! | [`events`] | per kind | subscribe to client events |
//! | [`ui`], [`slots`], [`decorations`], [`commands`] | `ui` | panels, menus, buttons, badges, keybinds, slash commands |
//! | [`render`] | `render-messages` | change how messages look ([`Plugin::render`]) |
//! | [`compose`] | `compose` | rewrite the user's outgoing messages ([`Plugin::on_compose`]) |
//! | [`actions`] | `act-local`, `react`, `act` | act as the user, on their click |
//! | [`moderation`] | `moderate` | kick, ban, timeout, voice moderation, bulk delete, on the user's click |
//! | [`history`] | `history` | page through a channel's history past what is loaded |
//! | [`view`] | `view` | change how this client shows users, members and the user's settings |
//! | [`presence`] | `presence` | status, custom status, rich presence |
//! | [`theme`] | `theming` | tokens, tint, fonts, surfaces, background |
//! | [`storage`] | `storage` | key/value data |
//! | [`http`] | `network` | HTTPS to declared hosts |
//! | [`notify`], [`audio`], [`files`], [`tweaks`], [`clipboard`], [`overlay`] | same names | |
//! | [`settings`], [`timer`], [`time`], [`consts`] | none | |
//!
//! Plugins compile to bare `wasm32-unknown-unknown`: there is no filesystem,
//! clock, or stdout. Use [`info!`] and friends instead of `println!`, and
//! [`now_millis`] and [`time`] instead of `std::time`, which panics there.
//! Panics are logged with their message before the plugin traps.

#![warn(missing_docs)]

pub mod actions;
pub mod audio;
pub mod clipboard;
pub mod commands;
pub mod compose;
pub mod consts;
pub mod decorations;
pub mod events;
pub mod files;
pub mod history;
pub mod http;
pub mod messages;
pub mod moderation;
pub mod notify;
pub mod overlay;
pub mod presence;
pub mod render;
pub mod settings;
pub mod slots;
pub mod state;
pub mod storage;
pub mod theme;
pub mod time;
pub mod timer;
pub mod tweaks;
pub mod ui;
pub mod users;
pub mod view;

#[doc(hidden)]
pub mod __private;

pub use rekindle_plugin_api as raw;

pub use raw::exports::rekindle::plugin::ui_handler::{SlotClick, SlotEvent};
pub use raw::rekindle::plugin::events::{Event, EventKind};
pub use raw::rekindle::plugin::host::{ClientInfo, LogLevel};
pub use raw::rekindle::plugin::records::{self, Message, User};
pub use raw::rekindle::plugin::types::{
    Capability, ChannelRef, Color, GuildRef, Icon, MediaRef, Member, MessageRef, Modifiers,
    RoleRef, Snowflake, Span, SpanColor, Target, UiEvent, UiValue, UserRef,
};

use commands::{CommandReply, Invocation};
use compose::{ComposeResult, Draft};
use render::{RenderInput, Transform};

/// Everything a typical plugin needs.
pub mod prelude {
    pub use crate::commands::{self, CommandReply, Invocation, OptionValue, SlashCommand};
    pub use crate::compose::{self, ComposeResult, Draft};
    pub use crate::decorations::{self, Decoration};
    pub use crate::render::{self, RenderInput, Transform};
    pub use crate::slots::{self, MenuItem};
    pub use crate::ui::{self, Elements, Panel, SpanExt, ToastKind, span};
    pub use crate::{
        Capability, ChannelRef, Color, Event, EventKind, Granted, GuildRef, Icon, Message,
        MessageRef, Modifiers, Plugin, SlotClick, SlotEvent, Span, Target, UiEvent, UiValue, User,
        UserRef, now_millis,
    };
    pub use crate::{
        actions, audio, clipboard, consts, events, files, history, http, messages, moderation,
        notify, overlay, presence, settings, state, storage, theme, time, timer, tweaks, users,
        view,
    };
    pub use crate::{debug, error, info, trace, warn};
}

/// The capabilities the plugin was granted, passed to [`Plugin::activate`].
/// The host grants everything the manifest requests.
#[derive(Debug, Clone)]
pub struct Granted(pub(crate) Vec<Capability>);

impl Granted {
    /// Whether `cap` was granted, directly or through a capability that
    /// implies it (`read-messages` implies `read-users`, `act` implies
    /// `react`).
    pub fn has(&self, cap: Capability) -> bool {
        self.0.contains(&cap)
            || match cap {
                Capability::ReadUsers => self.0.contains(&Capability::ReadMessages),
                Capability::React => self.0.contains(&Capability::Act),
                _ => false,
            }
    }

    /// The granted capabilities, as the host listed them.
    pub fn all(&self) -> &[Capability] {
        &self.0
    }
}

/// A plugin: its state, and the hooks the host calls on it. [`activate`]
/// builds the state, and every other hook takes `&mut self` and has a
/// default that does nothing.
///
/// The host calls a hook only if the plugin has its capability:
/// [`on_compose`] needs `compose`, [`render`] `render-messages`,
/// [`on_notification`] `notifications`, [`on_intercept`] `intercept`,
/// [`on_request`] and [`on_response`] `intercept-http`.
/// Events arrive only for kinds the capabilities cover, and after
/// [`events::subscribe`], only for the kinds listed.
///
/// [`activate`]: Plugin::activate
/// [`on_compose`]: Plugin::on_compose
/// [`render`]: Plugin::render
/// [`on_notification`]: Plugin::on_notification
/// [`on_intercept`]: Plugin::on_intercept
/// [`on_request`]: Plugin::on_request
/// [`on_response`]: Plugin::on_response
pub trait Plugin: Sized + 'static {
    /// Builds the plugin's state, once, after the host loads it. An `Err`
    /// fails the load, and the host shows the message.
    fn activate(granted: &Granted) -> Result<Self, String>;

    /// Called before the host unloads the plugin. The host removes
    /// everything the plugin contributed afterwards, so this only has to
    /// save state to [`storage`].
    fn deactivate(&mut self) {}

    /// A schema setting changed, on the settings page or through
    /// [`settings::set`]. Read the new value with [`settings`].
    fn on_settings_changed(&mut self, _key: &str) {}

    /// [`features`] changed (a client update or a re-anchor): slots or
    /// decorations may have appeared or gone, so re-add items if needed.
    fn on_features_changed(&mut self) {}

    /// The events of one host tick. The default calls [`on_event`] for each,
    /// in order.
    ///
    /// [`on_event`]: Plugin::on_event
    fn on_events(&mut self, events: Vec<Event>) {
        for e in events {
            self.on_event(e);
        }
    }

    /// One client event. The contract adds event kinds over time, so a
    /// `match` here should end with `_ => {}`.
    fn on_event(&mut self, _event: Event) {}

    /// The user changed an element on one of the plugin's surfaces: a panel
    /// or modal (by id), the settings page's extras ([`ui::SETTINGS_PAGE`]),
    /// the notice ([`ui::NOTICE`]), or a message accessory (`message:<id>`).
    fn on_ui(&mut self, _event: UiEvent) {}

    /// The user picked one of the plugin's palette commands.
    fn on_command(&mut self, _id: &str) {}

    /// A slot interaction: a click on one of the plugin's items, a
    /// decoration click, a message click binding, or a profile with the
    /// plugin's section opening.
    fn on_slot(&mut self, _event: SlotEvent) {}

    /// The user pressed one of the plugin's keybinds.
    fn on_keybind(&mut self, _id: &str) {}

    /// The user ran one of the plugin's slash commands.
    fn on_slash_command(&mut self, _inv: Invocation) -> CommandReply {
        CommandReply::None
    }

    /// Suggestions for an `autocomplete` option, as (label, value) pairs, at
    /// most 25. It runs on the render budget, so answer quickly.
    fn on_autocomplete(
        &mut self,
        _command: &str,
        _option: &str,
        _partial: &str,
    ) -> Vec<(String, String)> {
        Vec::new()
    }

    /// A timer set with [`timer::set`] or [`timer::repeat`] fired.
    fn on_timer(&mut self, _id: &str) {}

    /// The user is sending a message, or saving an edit of one. Return
    /// [`ComposeResult::Keep`], a rewrite ([`compose::replace`]), or a
    /// cancel ([`compose::cancel`]). The draft already carries earlier
    /// plugins' rewrites. The host refuses a result that's blank, over 4000
    /// characters or has control characters, and tells the user which
    /// plugins changed their message.
    fn on_compose(&mut self, _draft: &Draft) -> ComposeResult {
        ComposeResult::Keep
    }

    /// A file the user attached, before it's uploaded. Called only with
    /// `compose-uploads = true` in the manifest.
    fn on_upload(&mut self, _upload: compose::Upload) -> compose::UploadResult {
        compose::UploadResult::Keep
    }

    /// How a message should look. Runs in the *render isolate*: a separate
    /// instance of the plugin that never activates (so there is no `self`),
    /// can read settings and storage, and can't write, schedule, call out
    /// or keep anything. The host caches results per message. Call
    /// [`render::invalidate`] from the main instance when the rules change;
    /// a settings change invalidates by itself.
    fn render(_input: &RenderInput) -> Vec<Transform> {
        Vec::new()
    }

    /// A notification the client is about to show. Runs in the render
    /// isolate, like [`Plugin::render`].
    fn on_notification(_n: &notify::ClientNotification) -> notify::NotificationDecision {
        notify::NotificationDecision::Show
    }

    /// An event from Discord, before the client processes it, for the kinds
    /// registered with [`events::intercept`]. Return
    /// [`events::Verdict::Pass`], `Drop` (the client never sees it), or
    /// `Replace` with an event of the same kind. It runs on the client's hot
    /// path with the render budget; over the budget, the event passes.
    fn on_intercept(&mut self, _event: &Event) -> events::Verdict {
        events::Verdict::Pass
    }

    /// A REST request the client is about to send, on a route of a kind
    /// registered with [`events::intercept_http`]. It carries the method,
    /// path and body, never the headers. Return
    /// [`events::RequestVerdict::Pass`], `Drop` (the client sees a failed
    /// request) or `Replace` with another JSON body. Budgeted like
    /// [`Plugin::on_intercept`]; over the budget, the request goes as it is.
    fn on_request(&mut self, _request: &events::HttpRequest) -> events::RequestVerdict {
        events::RequestVerdict::Pass
    }

    /// The response to a request [`Plugin::on_request`] saw, before the
    /// client reads it. Return [`events::ResponseVerdict::Pass`] or
    /// `Replace` with another JSON body; the status stays.
    fn on_response(&mut self, _response: &events::HttpResponse) -> events::ResponseVerdict {
        events::ResponseVerdict::Pass
    }
}

/// Monotonic milliseconds since the host started.
pub fn now_millis() -> u64 {
    raw::rekindle::plugin::host::now_millis()
}

/// What the client is: host version, client build, platform, locale, base
/// theme, reduced motion and streamer mode.
pub fn client() -> ClientInfo {
    raw::rekindle::plugin::host::client()
}

/// The host features that resolved on this client build: slot names,
/// decoration kinds, `surface:<name>`, `tweak:<id>` and the backend's own.
pub fn features() -> Vec<String> {
    raw::rekindle::plugin::host::features()
}

/// Whether `name` is one of [`features`].
pub fn has_feature(name: &str) -> bool {
    raw::rekindle::plugin::host::has_feature(name)
}

/// A bundled string (`i18n/<locale>.toml` in the plugin's project) in the
/// user's language, falling back to the default locale, then to `key`.
pub fn tr(key: &str) -> String {
    raw::rekindle::plugin::host::tr(key)
}

/// Writes `message` to the host log at `level`. The [`info!`]-style macros
/// format and call this.
pub fn log(level: LogLevel, message: &str) {
    raw::rekindle::plugin::host::log(level, message);
}

/// Logs at trace level, with [`format!`] arguments.
#[macro_export]
macro_rules! trace { ($($t:tt)*) => { $crate::log($crate::LogLevel::Trace, &::std::format!($($t)*)) }; }
/// Logs at debug level, with [`format!`] arguments.
#[macro_export]
macro_rules! debug { ($($t:tt)*) => { $crate::log($crate::LogLevel::Debug, &::std::format!($($t)*)) }; }
/// Logs at info level, with [`format!`] arguments.
#[macro_export]
macro_rules! info { ($($t:tt)*) => { $crate::log($crate::LogLevel::Info, &::std::format!($($t)*)) }; }
/// Logs at warn level, with [`format!`] arguments.
#[macro_export]
macro_rules! warn { ($($t:tt)*) => { $crate::log($crate::LogLevel::Warn, &::std::format!($($t)*)) }; }
/// Logs at error level, with [`format!`] arguments.
#[macro_export]
macro_rules! error { ($($t:tt)*) => { $crate::log($crate::LogLevel::Error, &::std::format!($($t)*)) }; }

/// Exports a [`Plugin`] implementation as the component's entry points.
/// Call it once, at the crate root.
#[macro_export]
macro_rules! plugin {
    ($ty:ty) => {
        #[doc(hidden)]
        pub struct __RekindleExport;

        impl $crate::__private::Lifecycle for __RekindleExport {
            fn activate(
                granted: ::std::vec::Vec<$crate::Capability>,
            ) -> ::std::result::Result<(), ::std::string::String> {
                $crate::__private::activate::<$ty>(granted)
            }
            fn deactivate() {
                $crate::__private::deactivate::<$ty>();
            }
            fn on_settings_changed(key: ::std::string::String) {
                $crate::__private::with::<$ty>(|p| $crate::Plugin::on_settings_changed(p, &key));
            }
            fn on_features_changed() {
                $crate::__private::with::<$ty>(|p| $crate::Plugin::on_features_changed(p));
            }
        }

        impl $crate::__private::EventHandler for __RekindleExport {
            fn on_events(batch: ::std::vec::Vec<$crate::Event>) {
                $crate::__private::with::<$ty>(|p| $crate::Plugin::on_events(p, batch));
            }
        }

        impl $crate::__private::UiHandler for __RekindleExport {
            fn on_ui(ev: $crate::UiEvent) {
                $crate::__private::with::<$ty>(|p| $crate::Plugin::on_ui(p, ev));
            }
            fn on_command(id: ::std::string::String) {
                $crate::__private::with::<$ty>(|p| $crate::Plugin::on_command(p, &id));
            }
            fn on_slot(ev: $crate::SlotEvent) {
                $crate::__private::with::<$ty>(|p| $crate::Plugin::on_slot(p, ev));
            }
            fn on_keybind(id: ::std::string::String) {
                $crate::__private::with::<$ty>(|p| $crate::Plugin::on_keybind(p, &id));
            }
            fn on_slash_command(inv: $crate::commands::Invocation) -> $crate::commands::CommandReply {
                $crate::__private::with_ret::<$ty, _>(|p| $crate::Plugin::on_slash_command(p, inv))
                    .unwrap_or($crate::commands::CommandReply::None)
            }
            fn on_autocomplete(
                command: ::std::string::String,
                option: ::std::string::String,
                partial: ::std::string::String,
            ) -> ::std::vec::Vec<(::std::string::String, ::std::string::String)> {
                $crate::__private::with_ret::<$ty, _>(|p| $crate::Plugin::on_autocomplete(p, &command, &option, &partial))
                    .unwrap_or_default()
            }
        }

        impl $crate::__private::TimerHandler for __RekindleExport {
            fn on_timer(id: ::std::string::String) {
                $crate::__private::with::<$ty>(|p| $crate::Plugin::on_timer(p, &id));
            }
        }

        impl $crate::__private::ComposeHandler for __RekindleExport {
            fn on_compose(d: $crate::compose::Draft) -> $crate::compose::ComposeResult {
                $crate::__private::with_ret::<$ty, _>(|p| $crate::Plugin::on_compose(p, &d))
                    .unwrap_or($crate::compose::ComposeResult::Keep)
            }
            fn on_upload(u: $crate::compose::Upload) -> $crate::compose::UploadResult {
                $crate::__private::with_ret::<$ty, _>(|p| $crate::Plugin::on_upload(p, u))
                    .unwrap_or($crate::compose::UploadResult::Keep)
            }
        }

        impl $crate::__private::Renderer for __RekindleExport {
            fn on_render(
                batch: ::std::vec::Vec<$crate::render::RenderInput>,
            ) -> ::std::vec::Vec<::std::vec::Vec<$crate::render::Transform>> {
                $crate::__private::render::<$ty>(&batch)
            }
        }

        impl $crate::__private::EventFilter for __RekindleExport {
            fn on_intercept(ev: $crate::Event) -> $crate::events::Verdict {
                $crate::__private::with_ret::<$ty, _>(|p| $crate::Plugin::on_intercept(p, &ev))
                    .unwrap_or($crate::events::Verdict::Pass)
            }
        }

        impl $crate::__private::HttpFilter for __RekindleExport {
            fn on_request(req: $crate::events::HttpRequest) -> $crate::events::RequestVerdict {
                $crate::__private::with_ret::<$ty, _>(|p| $crate::Plugin::on_request(p, &req))
                    .unwrap_or($crate::events::RequestVerdict::Pass)
            }
            fn on_response(res: $crate::events::HttpResponse) -> $crate::events::ResponseVerdict {
                $crate::__private::with_ret::<$ty, _>(|p| $crate::Plugin::on_response(p, &res))
                    .unwrap_or($crate::events::ResponseVerdict::Pass)
            }
        }

        impl $crate::__private::NotificationFilter for __RekindleExport {
            fn on_notification(n: $crate::notify::ClientNotification) -> $crate::notify::NotificationDecision {
                $crate::__private::isolate_init();
                <$ty as $crate::Plugin>::on_notification(&n)
            }
        }

        $crate::raw::export!(__RekindleExport with_types_in $crate::raw);
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capability_counts_as_granted_when_one_that_implies_it_is() {
        let g = Granted(vec![Capability::ReadMessages, Capability::Act]);
        assert!(g.has(Capability::ReadMessages));
        assert!(g.has(Capability::ReadUsers));
        assert!(g.has(Capability::React));
        assert!(!g.has(Capability::ReadClient));
        assert_eq!(g.all(), [Capability::ReadMessages, Capability::Act]);
    }
}
