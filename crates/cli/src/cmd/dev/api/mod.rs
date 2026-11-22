//! Request routing for the preview server: pure functions over method, path
//! and body, so they can be tested without sockets or a runtime. The bodies
//! of the POST endpoints are parsed and checked in [`bodies`].

mod bodies;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use rekindle_plugin_host::api::{Event, Target};
use rekindle_plugin_host::model::{Mods, UiEvent};
use rekindle_plugin_host::settings::SettingValue;

/// Max request body we read.
pub(super) const MAX_BODY: usize = 128 * 1024;
/// The statuses `/api/presence` takes.
pub(super) const PRESENCE: &[&str] = &["online", "idle", "dnd", "invisible"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Method {
    Get,
    Post,
    Other,
}

#[derive(Debug, PartialEq)]
pub(super) enum Route {
    Page,
    /// Long-poll until the revision moves past `rev`.
    Wait {
        rev: u64,
    },
    Api(Action),
}

/// Something only the session (the thread that owns the runtime) can do.
#[derive(Debug, PartialEq)]
pub(crate) enum Action {
    State,
    /// An interaction on a plugin surface: a panel or modal (by id), else
    /// `settings-page` (its settings-page extras), `notice`, or
    /// `message:<id>` (an accessory).
    Ui {
        plugin: String,
        event: UiEvent,
    },
    Command {
        plugin: String,
        id: String,
    },
    /// A client event, as posted (its world effects are applied first).
    Event(Box<Event>),
    /// The user switches channel: `selection-changed`.
    SelectChannel {
        channel: u64,
    },
    /// The user sets their status: `my-presence`.
    SetPresence {
        status: String,
    },
    /// Someone else's message arrives: `message-created`.
    Receive {
        channel: u64,
        author: u64,
        content: String,
    },
    /// A loaded message's author edits it: `message-edited`.
    EditMessage {
        channel: u64,
        id: u64,
        content: String,
    },
    /// A loaded message is deleted: `message-deleted`.
    DeleteMessage {
        channel: u64,
        id: u64,
    },
    /// Rebuild (or, for plugins without a source, restart) one plugin or all.
    Reload {
        plugin: Option<String>,
    },
    /// The user clicked one of a plugin's slot items. `slot` narrows which
    /// slot the item is in (an id may be used in several).
    Slot {
        plugin: String,
        slot: Option<String>,
        item: String,
        target: Target,
        mods: Mods,
    },
    /// The user clicked a plugin's (clickable) decoration.
    Decoration {
        plugin: String,
        kind: String,
        key: u64,
    },
    /// The user opened someone's profile.
    Profile {
        user: rekindle_plugin_host::api::UserRef,
    },
    /// The user dismissed a plugin's modal (the plugin isn't called).
    ModalClose {
        plugin: String,
    },
    /// Pretend this client build does (or doesn't) have a slot's hook point.
    SlotAvailability {
        slot: String,
        available: bool,
    },
    /// The user sends a message: run it through `compose` plugins.
    Send {
        channel: u64,
        text: String,
    },
    /// The user changed (`Some`) or reset (`None`) a setting on a plugin's
    /// generated settings page.
    Setting {
        plugin: String,
        key: String,
        value: Option<SettingValue>,
    },
    /// The user pressed a plugin keybind.
    Keybind {
        plugin: String,
        id: String,
    },
    /// The user ran a plugin's slash command, options typed as text
    /// (converted per the option's kind). `channel` defaults to the selected one.
    Slash {
        plugin: String,
        command: String,
        options: BTreeMap<String, String>,
        channel: Option<u64>,
    },
    /// Start over with a fresh client and storage, and reload every plugin.
    Reset,
}

#[derive(Debug, PartialEq)]
pub(crate) struct ApiError {
    pub status: u16,
    pub message: String,
}

impl ApiError {
    pub(crate) fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub(crate) fn bad(message: impl Into<String>) -> Self {
        Self::new(400, message)
    }
}

type Parser = fn(&[u8]) -> Result<Action, ApiError>;

const GETS: &[&str] = &["/", "/index.html", "/api/state", "/api/wait"];

const POSTS: &[(&str, Parser)] = &[
    ("/api/ui", bodies::parse_ui),
    ("/api/command", bodies::parse_command),
    ("/api/event", bodies::parse_event),
    ("/api/select", bodies::parse_select),
    ("/api/presence", bodies::parse_presence),
    ("/api/receive", bodies::parse_receive),
    ("/api/message/edit", bodies::parse_edit),
    ("/api/message/delete", bodies::parse_delete),
    ("/api/reload", bodies::parse_reload),
    ("/api/slot", bodies::parse_slot),
    ("/api/decoration", bodies::parse_decoration),
    ("/api/profile", bodies::parse_profile),
    ("/api/modal/close", bodies::parse_modal_close),
    ("/api/slot-availability", bodies::parse_slot_availability),
    ("/api/send", bodies::parse_send),
    ("/api/setting", bodies::parse_setting),
    ("/api/setting/reset", bodies::parse_setting_reset),
    ("/api/keybind", bodies::parse_keybind),
    ("/api/slash", bodies::parse_slash),
    ("/api/reset", bodies::parse_reset),
];

/// The route for a request, its body parsed and checked.
pub(super) fn route(method: Method, url: &str, body: &[u8]) -> Result<Route, ApiError> {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let not_allowed = || Err(ApiError::new(405, format!("method not allowed for {path}")));
    if let Some((_, parse)) = POSTS.iter().find(|(p, _)| *p == path) {
        return match method {
            Method::Post => parse(body).map(Route::Api),
            _ => not_allowed(),
        };
    }
    match (method, path) {
        (Method::Get, "/" | "/index.html") => Ok(Route::Page),
        (Method::Get, "/api/state") => Ok(Route::Api(Action::State)),
        (Method::Get, "/api/wait") => {
            let rev = query_param(query, "rev").unwrap_or("0");
            let rev = rev
                .parse()
                .map_err(|_| ApiError::bad("`rev` must be a non-negative integer"))?;
            Ok(Route::Wait { rev })
        }
        _ if GETS.contains(&path) => not_allowed(),
        _ => Err(ApiError::new(404, format!("no such endpoint: {path}"))),
    }
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

/// DNS-rebinding and cross-site request guard: the Host must be our loopback
/// address, and a browser-supplied Origin (sent on cross-site POSTs) must be us.
pub(super) fn allowed_origin(host: Option<&str>, origin: Option<&str>, port: u16) -> bool {
    let ours = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let host_ok = host.is_some_and(|h| ours.iter().any(|o| h.eq_ignore_ascii_case(o)));
    let origin_ok = match origin {
        None => true,
        Some(o) => ours
            .iter()
            .any(|ours| o.eq_ignore_ascii_case(&format!("http://{ours}"))),
    };
    host_ok && origin_ok
}
