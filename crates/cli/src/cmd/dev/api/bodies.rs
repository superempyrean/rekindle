//! The bodies of the POST endpoints: one parser each, which checks every
//! field before it becomes an [`Action`].
//!
//! Snowflakes are 64-bit, beyond what a JavaScript number holds exactly, so
//! the page's own request bodies accept ids as numbers or digit strings
//! ([`snowflake`]). Contract records (`target`, events, setting values) are
//! the generated types' serde shape; the page passes targets back as the JSON
//! text the state gave it, so they round-trip exactly.

use std::collections::BTreeMap;

use rekindle_plugin_host::api::{Event, Target};
use rekindle_plugin_host::model::{ALL_SLOTS, DECORATION_KINDS, Mods, UiEvent, UiValue};
use rekindle_plugin_host::settings::SettingValue;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

use super::{Action, ApiError, MAX_BODY, PRESENCE};

const MAX_ID: usize = 256;
pub(super) const MAX_TEXT: usize = 4096;
pub(super) const MAX_HOOK_PAYLOAD: usize = 64 * 1024;
/// Display names (users, channels) as the client would show them.
const MAX_NAME: usize = 100;
/// Entries in a list or table value.
pub(super) const MAX_ITEMS: usize = 500;

/// `/api/reset` takes no body (or `{}`).
pub(super) fn parse_reset(body: &[u8]) -> Result<Action, ApiError> {
    let empty = String::from_utf8_lossy(body);
    if matches!(empty.trim(), "" | "{}") {
        Ok(Action::Reset)
    } else {
        Err(ApiError::bad("/api/reset takes no body"))
    }
}

fn json<'a, T: Deserialize<'a>>(body: &'a [u8]) -> Result<T, ApiError> {
    if body.len() > MAX_BODY {
        return Err(ApiError::new(413, "request body too large"));
    }
    serde_json::from_slice(body).map_err(|e| ApiError::bad(format!("invalid JSON body: {e}")))
}

fn ident(what: &str, s: &str) -> Result<(), ApiError> {
    if s.is_empty() || s.len() > MAX_ID {
        return Err(ApiError::bad(format!(
            "`{what}` must be 1..={MAX_ID} bytes"
        )));
    }
    Ok(())
}

fn name(what: &str, s: &str) -> Result<(), ApiError> {
    if s.is_empty() || s.len() > MAX_NAME {
        return Err(ApiError::bad(format!(
            "`{what}` must be 1..={MAX_NAME} bytes"
        )));
    }
    Ok(())
}

fn text(what: &str, s: &str) -> Result<(), ApiError> {
    if s.trim().is_empty() || s.len() > MAX_TEXT {
        return Err(ApiError::bad(format!(
            "`{what}` must be 1..={MAX_TEXT} bytes and not blank"
        )));
    }
    Ok(())
}

/// A snowflake as a JSON number or a string of digits.
fn snowflake<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Num(u64),
        Str(String),
    }
    match Id::deserialize(d)? {
        Id::Num(n) => Ok(n),
        Id::Str(s) => s
            .parse()
            .map_err(|_| D::Error::custom(format!("`{s}` is not a snowflake"))),
    }
}

fn opt_snowflake<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    #[derive(Deserialize)]
    struct Wrap(#[serde(deserialize_with = "snowflake")] u64);
    Ok(Option::<Wrap>::deserialize(d)?.map(|w| w.0))
}

/// Whether a string is longer than [`MAX_TEXT`].
fn long(s: &str) -> bool {
    s.len() > MAX_TEXT
}

/// Whether a list has more than [`MAX_ITEMS`] entries or a long one.
fn long_list(items: &[String]) -> bool {
    items.len() > MAX_ITEMS || items.iter().any(|s| long(s))
}

/// Whether a table has more than [`MAX_ITEMS`] rows or a long cell.
fn long_table(rows: &[Vec<String>]) -> bool {
    rows.len() > MAX_ITEMS || rows.iter().flatten().any(|s| long(s))
}

fn check_value(v: &UiValue) -> Result<(), ApiError> {
    match v {
        UiValue::Number(n) if !n.is_finite() => Err(ApiError::bad("number must be finite")),
        UiValue::Text(t) if long(t) => {
            Err(ApiError::bad(format!("text longer than {MAX_TEXT} bytes")))
        }
        UiValue::Items(items) if long_list(items) => Err(ApiError::bad(format!(
            "at most {MAX_ITEMS} items of {MAX_TEXT} bytes"
        ))),
        UiValue::Rows(rows) if long_table(rows) => Err(ApiError::bad(format!(
            "at most {MAX_ITEMS} rows, cells of at most {MAX_TEXT} bytes"
        ))),
        _ => Ok(()),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UiBody {
    plugin: String,
    #[serde(alias = "panel")]
    surface: String,
    element: String,
    value: UiValue,
}

pub(super) fn parse_ui(body: &[u8]) -> Result<Action, ApiError> {
    let b: UiBody = json(body)?;
    ident("plugin", &b.plugin)?;
    ident("surface", &b.surface)?;
    ident("element", &b.element)?;
    check_value(&b.value)?;
    Ok(Action::Ui {
        plugin: b.plugin,
        event: UiEvent {
            surface: b.surface,
            element: b.element,
            value: b.value,
        },
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginIdBody {
    plugin: String,
    id: String,
}

pub(super) fn parse_command(body: &[u8]) -> Result<Action, ApiError> {
    let b: PluginIdBody = json(body)?;
    ident("plugin", &b.plugin)?;
    ident("id", &b.id)?;
    Ok(Action::Command {
        plugin: b.plugin,
        id: b.id,
    })
}

pub(super) fn parse_keybind(body: &[u8]) -> Result<Action, ApiError> {
    let b: PluginIdBody = json(body)?;
    ident("plugin", &b.plugin)?;
    ident("id", &b.id)?;
    Ok(Action::Keybind {
        plugin: b.plugin,
        id: b.id,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendBody {
    #[serde(deserialize_with = "snowflake")]
    channel: u64,
    text: String,
}

pub(super) fn parse_send(body: &[u8]) -> Result<Action, ApiError> {
    let b: SendBody = json(body)?;
    text("text", &b.text)?;
    Ok(Action::Send {
        channel: b.channel,
        text: b.text,
    })
}

/// A JSON document in a string (hook and raw payloads).
fn check_payload(kind: &str, event_name: &str, payload: &str) -> Result<(), ApiError> {
    let ok_name = !event_name.is_empty()
        && event_name.len() <= 64
        && event_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'));
    if !ok_name {
        return Err(ApiError::bad(format!(
            "{kind} name must be 1..=64 of [A-Za-z0-9-_.:]"
        )));
    }
    if payload.len() > MAX_HOOK_PAYLOAD {
        return Err(ApiError::bad(format!(
            "{kind} payload larger than {MAX_HOOK_PAYLOAD} bytes"
        )));
    }
    if serde_json::from_str::<serde_json::Value>(payload).is_err() {
        return Err(ApiError::bad(format!(
            "{kind} payload must be a JSON document (as a string)"
        )));
    }
    Ok(())
}

/// Shape checks a real client's events would always pass. Whether the
/// channels, users and messages exist is checked against the world by the
/// session.
fn check_event(ev: &Event) -> Result<(), ApiError> {
    match ev {
        Event::Hook((n, payload)) => check_payload("hook", n, payload),
        Event::Raw((n, payload)) => check_payload("raw", n, payload),
        Event::MyPresence(status) if !PRESENCE.contains(&status.as_str()) => Err(ApiError::bad(
            format!("presence must be one of {}", PRESENCE.join(", ")),
        )),
        Event::MessageCreated(m) | Event::MessageEdited((m, _)) => {
            name("author name", &m.author.name)?;
            if m.content.len() > MAX_TEXT {
                return Err(ApiError::bad(format!(
                    "message content longer than {MAX_TEXT} bytes"
                )));
            }
            Ok(())
        }
        Event::PointerMoved((x, y)) if !(x.is_finite() && y.is_finite()) => {
            Err(ApiError::bad("pointer position must be finite"))
        }
        // Everything else is accepted as posted, so any plugin's handlers
        // can be exercised from the API.
        _ => Ok(()),
    }
}

pub(super) fn parse_event(body: &[u8]) -> Result<Action, ApiError> {
    let ev: Event = json(body)?;
    check_event(&ev)?;
    Ok(Action::Event(Box::new(ev)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelBody {
    #[serde(deserialize_with = "snowflake")]
    channel: u64,
}

pub(super) fn parse_select(body: &[u8]) -> Result<Action, ApiError> {
    let b: ChannelBody = json(body)?;
    Ok(Action::SelectChannel { channel: b.channel })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PresenceBody {
    status: String,
}

pub(super) fn parse_presence(body: &[u8]) -> Result<Action, ApiError> {
    let b: PresenceBody = json(body)?;
    if !PRESENCE.contains(&b.status.as_str()) {
        return Err(ApiError::bad(format!(
            "presence must be one of {}",
            PRESENCE.join(", ")
        )));
    }
    Ok(Action::SetPresence { status: b.status })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestBody {
    method: String,
    path: String,
    #[serde(default)]
    body: Option<String>,
}

pub(super) fn parse_rest(body: &[u8]) -> Result<Action, ApiError> {
    let b: RestBody = json(body)?;
    let method = b.method.to_ascii_uppercase();
    if !["GET", "POST", "PATCH", "PUT", "DELETE"].contains(&method.as_str()) {
        return Err(ApiError::bad(format!(
            "`method` is {method}, not GET, POST, PATCH, PUT or DELETE"
        )));
    }
    if !b.path.starts_with('/') {
        return Err(ApiError::bad(
            "`path` starts with `/`, below the API version",
        ));
    }
    Ok(Action::Rest {
        method,
        path: b.path,
        body: b.body,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiveBody {
    #[serde(deserialize_with = "snowflake")]
    channel: u64,
    #[serde(deserialize_with = "snowflake")]
    author: u64,
    content: String,
}

pub(super) fn parse_receive(body: &[u8]) -> Result<Action, ApiError> {
    let b: ReceiveBody = json(body)?;
    text("content", &b.content)?;
    Ok(Action::Receive {
        channel: b.channel,
        author: b.author,
        content: b.content,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageBody {
    #[serde(deserialize_with = "snowflake")]
    channel: u64,
    #[serde(deserialize_with = "snowflake")]
    id: u64,
    #[serde(default)]
    content: Option<String>,
}

pub(super) fn parse_edit(body: &[u8]) -> Result<Action, ApiError> {
    let b: MessageBody = json(body)?;
    let content = b
        .content
        .ok_or_else(|| ApiError::bad("`content` is required"))?;
    text("content", &content)?;
    Ok(Action::EditMessage {
        channel: b.channel,
        id: b.id,
        content,
    })
}

pub(super) fn parse_delete(body: &[u8]) -> Result<Action, ApiError> {
    let b: MessageBody = json(body)?;
    if b.content.is_some() {
        return Err(ApiError::bad("a delete takes no `content`"));
    }
    Ok(Action::DeleteMessage {
        channel: b.channel,
        id: b.id,
    })
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ReloadBody {
    #[serde(default)]
    plugin: Option<String>,
}

pub(super) fn parse_reload(body: &[u8]) -> Result<Action, ApiError> {
    let b: ReloadBody = if body.iter().all(u8::is_ascii_whitespace) {
        ReloadBody::default()
    } else {
        json(body)?
    };
    if let Some(p) = &b.plugin {
        ident("plugin", p)?;
    }
    Ok(Action::Reload { plugin: b.plugin })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SlotBody {
    plugin: String,
    #[serde(default)]
    slot: Option<String>,
    item: String,
    target: Target,
    #[serde(default)]
    mods: Mods,
}

/// Shape checks on a target: names a client would show, a link it would
/// offer a menu on.
fn check_target_shape(t: &Target) -> Result<(), ApiError> {
    match t {
        Target::User((u, _)) => name("user name", &u.name),
        Target::Channel(c) => name("channel name", &c.name),
        Target::Guild(g) => name("guild name", &g.name),
        Target::Role(r) => name("role name", &r.name),
        Target::Link(url) => {
            let ok =
                url.len() <= 2048 && (url.starts_with("https://") || url.starts_with("http://"));
            if ok {
                Ok(())
            } else {
                Err(ApiError::bad(
                    "a link target must be an http(s) URL of at most 2048 bytes",
                ))
            }
        }
        Target::Selection(s) => text("selection", s),
        Target::Media(m) => {
            if m.url.is_empty() || m.url.len() > 2048 {
                return Err(ApiError::bad(
                    "a media target needs a URL of at most 2048 bytes",
                ));
            }
            Ok(())
        }
        Target::None | Target::Message(_) => Ok(()),
    }
}

pub(super) fn parse_slot(body: &[u8]) -> Result<Action, ApiError> {
    let b: SlotBody = json(body)?;
    ident("plugin", &b.plugin)?;
    ident("item", &b.item)?;
    if let Some(s) = &b.slot {
        known_slot(s)?;
    }
    check_target_shape(&b.target)?;
    Ok(Action::Slot {
        plugin: b.plugin,
        slot: b.slot,
        item: b.item,
        target: b.target,
        mods: b.mods,
    })
}

fn known_slot(slot: &str) -> Result<(), ApiError> {
    if ALL_SLOTS.iter().any(|(s, _)| *s == slot) {
        return Ok(());
    }
    let all: Vec<&str> = ALL_SLOTS.iter().map(|(s, _)| *s).collect();
    Err(ApiError::bad(format!(
        "`slot` must be one of {}",
        all.join(", ")
    )))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecorationBody {
    plugin: String,
    kind: String,
    #[serde(deserialize_with = "snowflake")]
    key: u64,
}

pub(super) fn parse_decoration(body: &[u8]) -> Result<Action, ApiError> {
    let b: DecorationBody = json(body)?;
    ident("plugin", &b.plugin)?;
    if !DECORATION_KINDS.iter().any(|(k, _)| *k == b.kind) {
        return Err(ApiError::bad(format!(
            "unknown decoration kind `{}`",
            b.kind
        )));
    }
    Ok(Action::Decoration {
        plugin: b.plugin,
        kind: b.kind,
        key: b.key,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserBody {
    #[serde(deserialize_with = "snowflake")]
    id: u64,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileBody {
    user: UserBody,
}

pub(super) fn parse_profile(body: &[u8]) -> Result<Action, ApiError> {
    let b: ProfileBody = json(body)?;
    name("user name", &b.user.name)?;
    Ok(Action::Profile {
        user: rekindle_plugin_host::api::UserRef {
            id: b.user.id,
            name: b.user.name,
        },
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModalCloseBody {
    plugin: String,
}

pub(super) fn parse_modal_close(body: &[u8]) -> Result<Action, ApiError> {
    let b: ModalCloseBody = json(body)?;
    ident("plugin", &b.plugin)?;
    Ok(Action::ModalClose { plugin: b.plugin })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SlotAvailabilityBody {
    slot: String,
    available: bool,
}

pub(super) fn parse_slot_availability(body: &[u8]) -> Result<Action, ApiError> {
    let b: SlotAvailabilityBody = json(body)?;
    known_slot(&b.slot)?;
    Ok(Action::SlotAvailability {
        slot: b.slot,
        available: b.available,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingBody {
    plugin: String,
    key: String,
    value: SettingValue,
}

pub(super) fn parse_setting(body: &[u8]) -> Result<Action, ApiError> {
    let b: SettingBody = json(body)?;
    ident("plugin", &b.plugin)?;
    ident("key", &b.key)?;
    let too_big = match &b.value {
        SettingValue::Number(n) => !n.is_finite(),
        SettingValue::Text(t) | SettingValue::Keybind(t) => long(t),
        SettingValue::Items(v) => long_list(v),
        SettingValue::Rows(v) => long_table(v),
        SettingValue::Ids(v) => v.len() > MAX_ITEMS,
        SettingValue::Boolean(_) | SettingValue::Selected(_) | SettingValue::Color(_) => false,
    };
    if too_big {
        return Err(ApiError::bad(
            "setting value is out of bounds (non-finite, or too long)",
        ));
    }
    Ok(Action::Setting {
        plugin: b.plugin,
        key: b.key,
        value: Some(b.value),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingResetBody {
    plugin: String,
    key: String,
}

pub(super) fn parse_setting_reset(body: &[u8]) -> Result<Action, ApiError> {
    let b: SettingResetBody = json(body)?;
    ident("plugin", &b.plugin)?;
    ident("key", &b.key)?;
    Ok(Action::Setting {
        plugin: b.plugin,
        key: b.key,
        value: None,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SlashBody {
    plugin: String,
    command: String,
    #[serde(default)]
    options: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "opt_snowflake")]
    channel: Option<u64>,
}

pub(super) fn parse_slash(body: &[u8]) -> Result<Action, ApiError> {
    let b: SlashBody = json(body)?;
    ident("plugin", &b.plugin)?;
    ident("command", &b.command)?;
    if b.options.len() > 25 {
        return Err(ApiError::bad("at most 25 options"));
    }
    for (k, v) in &b.options {
        ident("option name", k)?;
        if v.len() > MAX_TEXT {
            return Err(ApiError::bad(format!(
                "option `{k}` is longer than {MAX_TEXT} bytes"
            )));
        }
    }
    Ok(Action::Slash {
        plugin: b.plugin,
        command: b.command,
        options: b.options,
        channel: b.channel,
    })
}
