//! Test a rekindle plugin from `cargo test`.
//!
//! `rekindle test` builds the plugin, runs it in the simulated client, and
//! then runs `cargo test` with the preview's address in the environment. Each
//! call on [`Preview`] does what a user would (run a command, click a menu
//! item, receive a message) and returns the preview's new state:
//!
//! ```no_run
//! use rekindle_test::Preview;
//!
//! #[test]
//! fn toggles() {
//!     let p = Preview::reset();
//!     p.command("toggle");
//!     assert_eq!(p.toasts(), ["Compact mode on"]);
//!     assert_eq!(p.storage()["compact"], "true");
//! }
//! ```
//!
//! Tests share one simulated client, so `rekindle test` runs them one at a
//! time. [`Preview::reset`] starts each from a clean client and storage.
//!
//! This crate only talks to the preview over loopback HTTP, so it builds in
//! seconds. For tests that load the plugin in-process, see
//! `rekindle_plugin_host::testing::Harness`.

/// For building interaction values: `json!({ "kind": "toggled", "value": true })`.
pub use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpStream;

/// The plugin under test, in the simulated client `rekindle test` runs.
pub struct Preview {
    addr: String,
    plugin: String,
}

/// The preview refused an action, or couldn't be reached.
#[derive(Debug)]
pub struct PreviewError {
    /// The HTTP status, or 0 if no response came back.
    pub status: u16,
    /// The preview's reason (its `error` field).
    pub message: String,
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (HTTP {})", self.message, self.status)
    }
}

impl std::error::Error for PreviewError {}

impl Preview {
    /// Connect to the preview `rekindle test` started.
    ///
    /// # Panics
    /// If the tests weren't started by `rekindle test`.
    pub fn connect() -> Self {
        let (Ok(url), Ok(plugin)) = (
            std::env::var("REKINDLE_TEST_URL"),
            std::env::var("REKINDLE_TEST_PLUGIN"),
        ) else {
            panic!("nothing to test against: run these tests with `rekindle test`");
        };
        let addr = url
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_string();
        Self { addr, plugin }
    }

    /// Connect, and start from a fresh simulated client and empty storage,
    /// with the plugin loaded again.
    pub fn reset() -> Self {
        let p = Self::connect();
        p.post("/api/reset", json!({}));
        p
    }

    /// The id of the plugin under test.
    pub fn plugin(&self) -> &str {
        &self.plugin
    }

    /// The preview's whole state, as `GET /api/state` returns it.
    pub fn state(&self) -> Value {
        self.request("GET", "/api/state", None)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// POST an action; panics if the preview refuses it (see [`Self::try_post`]).
    pub fn post(&self, path: &str, body: Value) -> Value {
        self.try_post(path, body)
            .unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    /// POST an action, returning the preview's refusal instead of panicking.
    pub fn try_post(&self, path: &str, body: Value) -> Result<Value, PreviewError> {
        self.request("POST", path, Some(body))
    }

    /// Rebuild and restart the plugin.
    pub fn reload(&self) -> Value {
        self.post("/api/reload", json!({ "plugin": self.plugin }))
    }

    /// Run one of the plugin's palette commands.
    pub fn command(&self, id: &str) -> Value {
        self.post("/api/command", json!({ "plugin": self.plugin, "id": id }))
    }

    /// Press one of the plugin's keybinds.
    pub fn keybind(&self, id: &str) -> Value {
        self.post("/api/keybind", json!({ "plugin": self.plugin, "id": id }))
    }

    /// Run a slash command, with its options as typed.
    pub fn slash(&self, command: &str, options: &[(&str, &str)]) -> Value {
        let options: serde_json::Map<String, Value> = options
            .iter()
            .map(|(k, v)| (k.to_string(), json!(v)))
            .collect();
        self.post(
            "/api/slash",
            json!({ "plugin": self.plugin, "command": command, "options": options }),
        )
    }

    /// Click one of the plugin's slot items, on nothing in particular.
    pub fn click(&self, item: &str) -> Value {
        self.click_on(item, None, json!("None"))
    }

    /// Click a slot item in `slot` on `target` (a target as the preview's
    /// state gives it, or e.g. `json!({ "Link": "https://example.com" })`).
    pub fn click_on(&self, item: &str, slot: Option<&str>, target: Value) -> Value {
        let mut body = json!({ "plugin": self.plugin, "item": item });
        body["target"] = target;
        if let Some(s) = slot {
            body["slot"] = json!(s);
        }
        self.post("/api/slot", body)
    }

    /// An interaction with one of the plugin's surfaces:
    /// `ui("main", "save", json!({ "kind": "clicked" }))`.
    pub fn ui(&self, surface: &str, element: &str, value: Value) -> Value {
        let mut body = json!({ "plugin": self.plugin, "surface": surface, "element": element });
        body["value"] = value;
        self.post("/api/ui", body)
    }

    /// A message from someone else arrives in `channel`.
    pub fn receive(&self, channel: u64, author: u64, content: &str) -> Value {
        self.post("/api/receive", json!({ "channel": channel.to_string(), "author": author.to_string(), "content": content }))
    }

    /// The user sends `text` in `channel`, through the plugins' `compose` hooks.
    pub fn send(&self, channel: u64, text: &str) -> Value {
        self.post(
            "/api/send",
            json!({ "channel": channel.to_string(), "text": text }),
        )
    }

    /// The user switches to `channel`.
    pub fn select(&self, channel: u64) -> Value {
        self.post("/api/select", json!({ "channel": channel.to_string() }))
    }

    /// Any client event, as the preview's event injector takes it.
    pub fn event(&self, ev: Value) -> Value {
        self.post("/api/event", ev)
    }

    /// Change a setting: `value` is `{ "kind": "boolean", "value": true }`
    /// and so on.
    pub fn set_setting(&self, key: &str, value: Value) -> Value {
        let mut body = json!({ "plugin": self.plugin, "key": key });
        body["value"] = value;
        self.post("/api/setting", body)
    }

    /// Put a setting back to its default.
    pub fn reset_setting(&self, key: &str) -> Value {
        self.post(
            "/api/setting/reset",
            json!({ "plugin": self.plugin, "key": key }),
        )
    }

    fn mine(&self, state: &Value) -> Value {
        state["plugins"]
            .as_array()
            .and_then(|ps| ps.iter().find(|p| p["id"] == self.plugin.as_str()))
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// `"active"` or `"faulted"`; `"failed"` if the plugin didn't load, and
    /// `"missing"` if the preview doesn't have it.
    pub fn status(&self) -> String {
        let p = self.mine(&self.state());
        if p.is_null() {
            "missing".into()
        } else if !p["load_error"].is_null() {
            "failed".into()
        } else {
            p["info"]["status"]["state"]
                .as_str()
                .unwrap_or("unknown")
                .into()
        }
    }

    /// Panic (with the plugin's logs) unless the plugin is active.
    pub fn assert_active(&self) {
        let state = self.state();
        let p = self.mine(&state);
        if p["info"]["status"]["state"] == "active" && p["load_error"].is_null() {
            return;
        }
        let why = p["load_error"]
            .as_str()
            .or(p["info"]["status"]["reason"].as_str())
            .unwrap_or("not loaded");
        let logs: Vec<String> = self.logs().into_iter().map(|l| format!("  {l}")).collect();
        panic!("the plugin isn't active: {why}\n{}", logs.join("\n"));
    }

    /// The text of the plugin's toasts, oldest first.
    pub fn toasts(&self) -> Vec<String> {
        let state = self.state();
        let all = state["slots"]["toasts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        all.iter()
            .filter(|t| t["plugin"] == self.plugin.as_str())
            .filter_map(|t| t["text"].as_str().map(String::from))
            .collect()
    }

    /// The plugin's storage, as a JSON object of strings.
    pub fn storage(&self) -> Value {
        let p = self.mine(&self.state());
        if p["storage"].is_object() {
            p["storage"].clone()
        } else {
            json!({})
        }
    }

    /// The plugin's log lines, as `level: message`.
    pub fn logs(&self) -> Vec<String> {
        let state = self.state();
        let all = state["logs"].as_array().cloned().unwrap_or_default();
        all.iter()
            .map(|l| &l["record"])
            .filter(|r| r["plugin"] == self.plugin.as_str())
            .map(|r| {
                format!(
                    "{}: {}",
                    r["level"].as_str().unwrap_or("?"),
                    r["message"].as_str().unwrap_or("")
                )
            })
            .collect()
    }

    /// Actions the plugin performed as the user (marked read, reacted, ...).
    pub fn performed(&self) -> Vec<Value> {
        self.state()["performed"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// A theme token's value as the client has it now (`"#cba6f7"`,
    /// `"14px"`), whoever set it.
    pub fn theme(&self, token: &str) -> Option<String> {
        let state = self.state();
        let all = state["theme"].as_array().cloned().unwrap_or_default();
        let t = all.iter().find(|t| {
            t["token"] == token
                || t["aliases"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| x == token))
        })?;
        t["value"].as_str().map(String::from)
    }

    /// One of the plugin's panels, as the client shows it: `{ id, title,
    /// elements }`. `None` if it isn't showing.
    pub fn panel(&self, id: &str) -> Option<Value> {
        let p = self.mine(&self.state());
        p["panels"]
            .as_array()?
            .iter()
            .find(|x| x["id"] == id)
            .cloned()
    }

    /// The text of a panel: its headings and text lines, and fields as
    /// `key: value`, top to bottom. Empty if it isn't showing.
    pub fn panel_text(&self, id: &str) -> Vec<String> {
        let Some(panel) = self.panel(id) else {
            return Vec::new();
        };
        let elements = panel["elements"].as_array().cloned().unwrap_or_default();
        elements
            .iter()
            .filter_map(|e| match e["kind"].as_str()? {
                "heading" | "text" => e["text"].as_str().map(String::from),
                "field" => Some(format!("{}: {}", e["key"].as_str()?, e["value"].as_str()?)),
                _ => None,
            })
            .collect()
    }

    /// The id of a channel in the simulated client, by name (`"general"`).
    /// Panics with the names there are if none has it.
    pub fn channel(&self, name: &str) -> u64 {
        let state = self.state();
        let all = state["client"]["channels"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let id = all.iter().find(|c| c["name"] == name).map(|c| &c["id"]);
        if let Some(id) = id.and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok())) {
            id
        } else {
            let names: Vec<&str> = all.iter().filter_map(|c| c["name"].as_str()).collect();
            panic!("no channel named `{name}`; the simulated client has {names:?}")
        }
    }

    /// One HTTP/1.1 request over loopback. The preview is ours and simple:
    /// one request per connection, the body sized by `Content-Length`.
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, PreviewError> {
        let io = |e: std::io::Error| PreviewError {
            status: 0,
            message: format!("talking to the preview at {}: {e}", self.addr),
        };
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let mut s = TcpStream::connect(&self.addr).map_err(io)?;
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.addr,
            body.len()
        );
        s.write_all(head.as_bytes())
            .and_then(|()| s.write_all(body.as_bytes()))
            .map_err(io)?;
        let mut raw = Vec::new();
        s.read_to_end(&mut raw).map_err(io)?;
        parse_response(&raw)
    }
}

/// The JSON body of a raw HTTP response, or the preview's refusal.
fn parse_response(raw: &[u8]) -> Result<Value, PreviewError> {
    let (head, payload) = match find(raw, b"\r\n\r\n") {
        Some(i) => (&raw[..i], &raw[i + 4..]),
        None => (raw, &[][..]),
    };
    let head = String::from_utf8_lossy(head).to_ascii_lowercase();
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let payload = if head.contains("transfer-encoding: chunked") {
        dechunk(payload)
    } else {
        payload.to_vec()
    };
    let v: Value = serde_json::from_slice(&payload).map_err(|e| PreviewError {
        status,
        message: format!("bad response from the preview: {e}"),
    })?;
    if (200..300).contains(&status) {
        return Ok(v);
    }
    Err(PreviewError {
        status,
        message: v["error"]
            .as_str()
            .unwrap_or("the preview refused the request")
            .to_string(),
    })
}

/// A chunked HTTP body, joined. Chunk sizes count bytes, and a chunk can end
/// inside a UTF-8 character, so this works on bytes.
fn dechunk(mut s: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(eol) = find(s, b"\r\n") {
        let size = std::str::from_utf8(&s[..eol])
            .ok()
            .and_then(|line| line.split(';').next())
            .and_then(|n| usize::from_str_radix(n.trim(), 16).ok());
        let rest = &s[eol + 2..];
        let Some(n) = size.filter(|&n| n > 0 && n <= rest.len()) else {
            break;
        };
        out.extend_from_slice(&rest[..n]);
        s = rest[n..].strip_prefix(b"\r\n").unwrap_or(&rest[n..]);
    }
    out
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_chunks() {
        assert_eq!(
            dechunk(b"4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n"),
            b"{\"a\":1}"
        );
    }

    #[test]
    fn a_character_split_across_chunks_survives() {
        // "é" is C3 A9; the first chunk ends between the two bytes.
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n7\r\n{\"a\":\"\xC3\r\n3\r\n\xA9\"}\r\n0\r\n\r\n";
        assert_eq!(parse_response(raw).unwrap(), json!({ "a": "é" }));
    }

    #[test]
    fn a_refusal_carries_the_previews_reason() {
        let raw =
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 27\r\n\r\n{\"error\":\"no such command\"}";
        let e = parse_response(raw).unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (404, "no such command"));
    }
}
