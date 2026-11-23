//! A stand-in for Discord's REST API, for the routes the samples and tests
//! use, answered from the sample world. The plugins' `http-filter`s run
//! around it (`testing::Harness::rest`, the preview's `/api/rest`).

use serde_json::{Value, json};

use super::MockBackend;
use crate::api;

impl MockBackend {
    /// Answer a request the way Discord would, from the world: `GET` and
    /// `POST` on `/channels/{id}/messages`, `GET` on `/channels/{id}`.
    /// Everything else is a 404. Returns (status, body).
    pub fn rest(&mut self, method: &str, path: &str, body: Option<&str>) -> (u16, Option<String>) {
        let (route, query) = path.split_once('?').unwrap_or((path, ""));
        let seg: Vec<&str> = route.split('/').filter(|s| !s.is_empty()).collect();
        let answer = match (method, seg.as_slice()) {
            ("GET", ["channels", c, "messages"]) => c.parse().ok().map(|c| {
                let limit = query
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("limit="))
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(50)
                    .min(100);
                let list: Vec<Value> = self
                    .world
                    .messages
                    .get(&c)
                    .map(|v| v.iter().rev().take(limit).map(message_json).collect())
                    .unwrap_or_default();
                (200, Value::Array(list))
            }),
            ("POST", ["channels", c, "messages"]) => c.parse().ok().and_then(|c: u64| {
                let content = serde_json::from_str::<Value>(body?)
                    .ok()?
                    .get("content")?
                    .as_str()?
                    .to_string();
                let guild = self.world.channels.iter().find(|ch| ch.id == c)?.guild;
                let last = self
                    .world
                    .messages
                    .get(&c)
                    .and_then(|v| v.last())
                    .map_or(1 << 22, |m| m.id);
                let me = self.world.me;
                let name = self.name_of(me);
                let m = super::super::sample_message(c, guild, last + 1, (me, &name), &content);
                self.world.push_message(m.clone());
                Some((200, message_json(&m)))
            }),
            ("GET", ["channels", c]) => c.parse::<u64>().ok().and_then(|c| {
                let ch = self.world.channels.iter().find(|ch| ch.id == c)?;
                Some((200, json!({ "id": ch.id.to_string(), "name": ch.name })))
            }),
            _ => None,
        };
        let (status, value) =
            answer.unwrap_or((404, json!({ "message": "404: Not Found", "code": 0 })));
        (status, Some(value.to_string()))
    }
}

fn message_json(m: &api::Message) -> Value {
    json!({
        "id": m.id.to_string(),
        "channel_id": m.channel.to_string(),
        "author": { "id": m.author.id.to_string(), "username": m.author.name },
        "content": m.content,
    })
}
