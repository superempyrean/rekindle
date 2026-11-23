//! `client-state`, `users`, `messages`, `inspect`: read-only queries over
//! what the client already holds (design doc §4). Lists are capped; nothing
//! here causes a network request except `resolve-message-link`, which only
//! makes the fetch the client makes when the user clicks a visible link.

use crate::api;
use crate::bindings::wit;
use crate::capability::Capability;

use super::state::PluginState;

/// Default list cap (design doc §4.2).
const CAP: usize = 200;
const MEMBER_CAP: u32 = 1000;

fn cap<T>(mut v: Vec<T>, n: usize) -> Vec<T> {
    v.truncate(n);
    v
}

/// Setting keys a plugin may read: a curated allowlist by prefix, never the
/// timezone or anything account-sensitive.
pub(super) fn setting_allowed(key: &str) -> bool {
    const PREFIXES: [&str; 8] = [
        "appearance.",
        "text-and-images.",
        "notifications.",
        "accessibility.",
        "voice.",
        "privacy.",
        "safety.",
        "streamer-mode.",
    ];
    const NEVER: [&str; 6] = ["timezone", "token", "email", "phone", "password", "mfa"];
    PREFIXES.iter().any(|p| key.starts_with(p)) && !NEVER.iter().any(|n| key.contains(n))
}

impl PluginState {
    /// `user` without the account fields `read-account` covers, unless the
    /// plugin has it.
    fn account_fields(&self, mut user: api::User) -> api::User {
        if !self.grants.has(Capability::ReadAccount) {
            user.email = None;
            user.phone = None;
            user.store_country = None;
        }
        user
    }
}

impl wit::client_state::Host for PluginState {
    fn get_selection(&mut self) -> wasmtime::Result<api::Selection> {
        self.require(Capability::ReadClient)?;
        Ok(self.backend.lock().unwrap().selection())
    }

    fn me(&mut self) -> wasmtime::Result<api::User> {
        self.require(Capability::ReadClient)?;
        let me = self.backend.lock().unwrap().me().unwrap_or_default();
        Ok(self.account_fields(me))
    }

    fn my_status(&mut self) -> wasmtime::Result<String> {
        self.require(Capability::ReadClient)?;
        Ok(self.backend.lock().unwrap().my_status())
    }

    fn my_custom_status(&mut self) -> wasmtime::Result<Option<api::CustomStatus>> {
        self.require(Capability::ReadClient)?;
        Ok(self.backend.lock().unwrap().my_custom_status())
    }

    fn get_user(&mut self, id: u64) -> wasmtime::Result<Option<api::User>> {
        self.require(Capability::ReadClient)?;
        let user = self.backend.lock().unwrap().user(id);
        Ok(user.map(|u| self.account_fields(u)))
    }

    fn get_guild(&mut self, id: u64) -> wasmtime::Result<Option<api::Guild>> {
        self.require(Capability::ReadClient)?;
        Ok(self.backend.lock().unwrap().guild(id))
    }

    fn guild_ids(&mut self) -> wasmtime::Result<Vec<u64>> {
        self.require(Capability::ReadClient)?;
        Ok(self.backend.lock().unwrap().guild_ids())
    }

    fn guild_folders(&mut self) -> wasmtime::Result<Vec<api::GuildFolder>> {
        self.require(Capability::ReadClient)?;
        Ok(cap(self.backend.lock().unwrap().guild_folders(), CAP))
    }

    fn get_channel(&mut self, id: u64) -> wasmtime::Result<Option<api::Channel>> {
        self.require(Capability::ReadClient)?;
        Ok(self.backend.lock().unwrap().channel(id))
    }

    fn channels(
        &mut self,
        guild: u64,
        include_hidden: bool,
    ) -> wasmtime::Result<Vec<api::Channel>> {
        self.require(Capability::ReadClient)?;
        Ok(cap(
            self.backend.lock().unwrap().channels(guild, include_hidden),
            500,
        ))
    }

    fn threads(&mut self, parent: u64) -> wasmtime::Result<Vec<api::Channel>> {
        self.require(Capability::ReadClient)?;
        Ok(cap(self.backend.lock().unwrap().threads(parent), CAP))
    }

    fn private_channels(&mut self) -> wasmtime::Result<Vec<api::Channel>> {
        self.require(Capability::ReadClient)?;
        Ok(cap(self.backend.lock().unwrap().private_channels(), CAP))
    }

    fn dm_channel(&mut self, user: u64) -> wasmtime::Result<Option<u64>> {
        self.require(Capability::ReadClient)?;
        Ok(self.backend.lock().unwrap().dm_channel(user))
    }

    fn get_role(&mut self, guild: u64, id: u64) -> wasmtime::Result<Option<api::Role>> {
        self.require(Capability::ReadClient)?;
        Ok(self.backend.lock().unwrap().role(guild, id))
    }

    fn roles(&mut self, guild: u64) -> wasmtime::Result<Vec<api::Role>> {
        self.require(Capability::ReadClient)?;
        Ok(cap(self.backend.lock().unwrap().roles(guild), 250))
    }

    fn guild_emoji(&mut self, guild: u64) -> wasmtime::Result<Vec<api::Emoji>> {
        self.require(Capability::ReadClient)?;
        Ok(cap(self.backend.lock().unwrap().guild_emoji(guild), 500))
    }

    fn frequent_emoji(&mut self, limit: u32) -> wasmtime::Result<Vec<api::Emoji>> {
        self.require(Capability::ReadClient)?;
        Ok(cap(
            self.backend
                .lock()
                .unwrap()
                .frequent_emoji(limit.min(CAP as u32)),
            CAP,
        ))
    }

    fn guild_stickers(&mut self, guild: u64) -> wasmtime::Result<Vec<api::Sticker>> {
        self.require(Capability::ReadClient)?;
        Ok(cap(self.backend.lock().unwrap().guild_stickers(guild), CAP))
    }

    fn my_member(&mut self, guild: u64) -> wasmtime::Result<Option<api::Member>> {
        self.require(Capability::ReadClient)?;
        Ok(self.backend.lock().unwrap().my_member(guild))
    }

    fn my_permissions(&mut self, channel_or_guild: u64) -> wasmtime::Result<u64> {
        self.require(Capability::ReadClient)?;
        Ok(self
            .backend
            .lock()
            .unwrap()
            .my_permissions(channel_or_guild))
    }

    fn can(&mut self, bits: u64, channel_or_guild: u64) -> wasmtime::Result<bool> {
        self.require(Capability::ReadClient)?;
        Ok(self
            .backend
            .lock()
            .unwrap()
            .my_permissions(channel_or_guild)
            & bits
            == bits)
    }

    fn permission_overwrites(&mut self, channel: u64) -> wasmtime::Result<Vec<api::Overwrite>> {
        self.require(Capability::ReadClient)?;
        Ok(cap(
            self.backend.lock().unwrap().permission_overwrites(channel),
            CAP,
        ))
    }

    fn get_read_state(&mut self, channel: u64) -> wasmtime::Result<Option<api::ReadState>> {
        self.require(Capability::ReadClient)?;
        Ok(self.backend.lock().unwrap().read_state(channel))
    }

    fn get_guild_read_state(&mut self, guild: u64) -> wasmtime::Result<api::GuildReadState> {
        self.require(Capability::ReadClient)?;
        Ok(self.backend.lock().unwrap().guild_read_state(guild))
    }

    fn unread_channels(&mut self, limit: u32) -> wasmtime::Result<Vec<api::ReadState>> {
        self.require(Capability::ReadClient)?;
        Ok(cap(
            self.backend
                .lock()
                .unwrap()
                .unread_channels(limit.min(CAP as u32)),
            CAP,
        ))
    }

    fn get_my_voice(&mut self) -> wasmtime::Result<Option<api::MyVoice>> {
        self.require(Capability::ReadClient)?;
        Ok(self.backend.lock().unwrap().my_voice())
    }

    fn my_setting(&mut self, key: String) -> wasmtime::Result<Option<String>> {
        self.require(Capability::ReadClient)?;
        if !setting_allowed(&key) {
            return Ok(None);
        }
        Ok(self.backend.lock().unwrap().my_setting(&key))
    }

    fn my_setting_keys(&mut self) -> wasmtime::Result<Vec<String>> {
        self.require(Capability::ReadClient)?;
        Ok(self
            .backend
            .lock()
            .unwrap()
            .my_setting_keys()
            .into_iter()
            .filter(|k| setting_allowed(k))
            .collect())
    }
}

impl wit::users::Host for PluginState {
    fn get_presence(
        &mut self,
        user: u64,
        guild: Option<u64>,
    ) -> wasmtime::Result<Option<api::Presence>> {
        self.require(Capability::ReadUsers)?;
        Ok(self.backend.lock().unwrap().presence(user, guild))
    }

    fn get_member(&mut self, guild: u64, user: u64) -> wasmtime::Result<Option<api::Member>> {
        self.require(Capability::ReadUsers)?;
        Ok(self.backend.lock().unwrap().member(guild, user))
    }

    fn loaded_members(
        &mut self,
        guild: u64,
        limit: u32,
    ) -> wasmtime::Result<(Vec<api::Member>, u32)> {
        self.require(Capability::ReadUsers)?;
        let (v, n) = self
            .backend
            .lock()
            .unwrap()
            .loaded_members(guild, limit.min(MEMBER_CAP));
        Ok((cap(v, MEMBER_CAP as usize), n))
    }

    fn member_permissions(
        &mut self,
        user: u64,
        channel_or_guild: u64,
    ) -> wasmtime::Result<Option<u64>> {
        self.require(Capability::ReadUsers)?;
        Ok(self
            .backend
            .lock()
            .unwrap()
            .member_permissions(user, channel_or_guild))
    }

    fn member_color(
        &mut self,
        guild: u64,
        user: u64,
    ) -> wasmtime::Result<Option<wit::types::Color>> {
        self.require(Capability::ReadUsers)?;
        Ok(self
            .backend
            .lock()
            .unwrap()
            .member_color(guild, user)
            .map(super::convert::color_to_wit))
    }

    fn typing(&mut self, channel: u64) -> wasmtime::Result<Vec<u64>> {
        self.require(Capability::ReadUsers)?;
        Ok(cap(self.backend.lock().unwrap().typing(channel), CAP))
    }

    fn voice_states(&mut self, channel: u64) -> wasmtime::Result<Vec<api::VoiceState>> {
        self.require(Capability::ReadUsers)?;
        Ok(cap(self.backend.lock().unwrap().voice_states(channel), CAP))
    }

    fn get_voice_state(&mut self, user: u64) -> wasmtime::Result<Option<api::VoiceState>> {
        self.require(Capability::ReadUsers)?;
        Ok(self.backend.lock().unwrap().voice_state(user))
    }

    fn speaking(&mut self, user: u64) -> wasmtime::Result<bool> {
        self.require(Capability::ReadUsers)?;
        Ok(self.backend.lock().unwrap().speaking(user))
    }

    fn relationships(&mut self) -> wasmtime::Result<Vec<api::Relationship>> {
        self.require(Capability::ReadUsers)?;
        Ok(cap(self.backend.lock().unwrap().relationships(), 1000))
    }

    fn get_relationship(&mut self, user: u64) -> wasmtime::Result<Option<api::RelationshipKind>> {
        self.require(Capability::ReadUsers)?;
        Ok(self
            .backend
            .lock()
            .unwrap()
            .relationships()
            .into_iter()
            .find(|r| r.user == user)
            .map(|r| r.kind))
    }

    fn note(&mut self, user: u64) -> wasmtime::Result<Option<String>> {
        self.require(Capability::ReadUsers)?;
        Ok(self.backend.lock().unwrap().note(user))
    }

    fn mutual_guilds(&mut self, user: u64) -> wasmtime::Result<Vec<u64>> {
        self.require(Capability::ReadUsers)?;
        Ok(cap(self.backend.lock().unwrap().mutual_guilds(user), CAP))
    }
}

/// `(guild, channel, message)` from a Discord message link.
pub(super) fn parse_message_link(url: &str) -> Option<(Option<u64>, u64, u64)> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    let host_ok = matches!(
        host,
        "discord.com"
            | "ptb.discord.com"
            | "canary.discord.com"
            | "discordapp.com"
            | "ptb.discordapp.com"
            | "canary.discordapp.com"
    );
    if !host_ok {
        return None;
    }
    let mut parts = path.split(['?', '#']).next()?.split('/');
    if parts.next()? != "channels" {
        return None;
    }
    let guild = match parts.next()? {
        "@me" => None,
        g => Some(g.parse().ok()?),
    };
    let channel = parts.next()?.parse().ok()?;
    let message = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((guild, channel, message))
}

impl wit::history::Host for PluginState {
    fn query(
        &mut self,
        q: wit::history::HistoryQuery,
    ) -> wasmtime::Result<Result<wit::history::Page, wit::history::HistoryError>> {
        use wit::history::HistoryError as E;
        self.require(Capability::History)?;
        if !self.grants.has(Capability::ReadMessages) {
            return Ok(Err(E::NotGranted));
        }
        if q.channel == 0 || q.limit == 0 {
            return Ok(Err(E::Invalid));
        }
        let now = self.now();
        if !self.rates.history.take(now, 30, 60_000) {
            return Ok(Err(E::RateLimited));
        }
        let limit = q.limit.min(1000);
        let after = q.after.filter(|_| q.before.is_none());
        let Some(mut messages) = self
            .backend
            .lock()
            .unwrap()
            .history(q.channel, q.before, after, limit)
        else {
            return Ok(Err(E::Unavailable));
        };
        messages.truncate(limit as usize);
        let cursor = (messages.len() == limit as usize)
            .then(|| messages.last().map(|m| m.id))
            .flatten();
        Ok(Ok(wit::history::Page { messages, cursor }))
    }
}

impl wit::messages::Host for PluginState {
    fn get_message(&mut self, channel: u64, id: u64) -> wasmtime::Result<Option<api::Message>> {
        self.require(Capability::ReadMessages)?;
        Ok(self.backend.lock().unwrap().message(channel, id))
    }

    fn loaded_messages(
        &mut self,
        channel: u64,
        before: Option<u64>,
        limit: u32,
    ) -> wasmtime::Result<Vec<api::Message>> {
        self.require(Capability::ReadMessages)?;
        Ok(cap(
            self.backend
                .lock()
                .unwrap()
                .loaded_messages(channel, before, limit.min(100)),
            100,
        ))
    }

    fn visible_messages(&mut self) -> wasmtime::Result<Vec<api::MessageRef>> {
        self.require(Capability::ReadMessages)?;
        Ok(cap(self.backend.lock().unwrap().visible_messages(), CAP))
    }

    fn pins(&mut self, channel: u64) -> wasmtime::Result<Vec<api::MessageRef>> {
        self.require(Capability::ReadMessages)?;
        Ok(cap(self.backend.lock().unwrap().pins(channel), 250))
    }

    fn resolve_message_link(
        &mut self,
        url: String,
    ) -> wasmtime::Result<Result<api::Message, wit::messages::LinkError>> {
        use wit::messages::LinkError as E;
        self.require(Capability::ReadMessages)?;
        let Some((guild, channel, id)) = parse_message_link(&url) else {
            return Ok(Err(E::Invalid));
        };
        let mut b = self.backend.lock().unwrap();
        if !b.link_on_screen(&url) {
            return Ok(Err(E::NotVisible));
        }
        if let Some(m) = b.message(channel, id) {
            return Ok(Ok(m));
        }
        let now = self.now();
        if !self.rates.link.take(now, 10, 60_000) {
            return Ok(Err(E::RateLimited));
        }
        Ok(match b.fetch_linked_message(guild, channel, id) {
            Some(Some(m)) => Ok(m),
            Some(None) => Err(E::NotFound),
            None => Err(E::Unavailable),
        })
    }
}

/// Keys whose values never reach a plugin, at any depth of `raw-json`.
fn secret_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    [
        "token",
        "email",
        "phone",
        "password",
        "mfa",
        "session",
        "secret",
        "party",
        "sync_id",
        "verified",
        "authorization",
        "nonce",
    ]
    .iter()
    .any(|s| {
        k == *s
            || k.contains(&format!("{s}_"))
            || k.ends_with(&format!("_{s}"))
            || (s.len() > 3 && k.contains(s))
    })
}

/// Strip secret-looking fields and CDN signing parameters (`?ex=&is=&hm=`).
pub fn redact(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            map.retain(|k, _| !secret_key(k));
            for (_, v) in map.iter_mut() {
                redact(v);
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(redact),
        serde_json::Value::String(s) if s.starts_with("https://") && s.contains("?ex=") => {
            if let Some(i) = s.find('?') {
                s.truncate(i);
            }
        }
        _ => {}
    }
}

impl wit::inspect::Host for PluginState {
    fn raw_json(&mut self, target: api::Target) -> wasmtime::Result<Option<String>> {
        self.require(Capability::ReadClient)?;
        let needs = match &target {
            api::Target::Message(_) | api::Target::Selection(_) => Some(Capability::ReadMessages),
            api::Target::User(_) => Some(Capability::ReadUsers),
            _ => None,
        };
        if needs.is_some_and(|c| !self.grants.has(c)) {
            return Ok(None);
        }
        let Some(raw) = self.backend.lock().unwrap().raw_json(&target) else {
            return Ok(None);
        };
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return Ok(None);
        };
        redact(&mut v);
        let s = serde_json::to_string_pretty(&v).unwrap_or_default();
        Ok((s.len() <= 256 * 1024).then_some(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_links() {
        assert_eq!(
            parse_message_link("https://discord.com/channels/1/2/3"),
            Some((Some(1), 2, 3))
        );
        assert_eq!(
            parse_message_link("https://canary.discord.com/channels/@me/2/3?x=1"),
            Some((None, 2, 3))
        );
        assert_eq!(
            parse_message_link("http://discord.com/channels/1/2/3"),
            None
        );
        assert_eq!(
            parse_message_link("https://evil.example/channels/1/2/3"),
            None
        );
        assert_eq!(parse_message_link("https://discord.com/channels/1/2"), None);
        assert_eq!(
            parse_message_link("https://discord.com/channels/1/2/3/4"),
            None
        );
    }

    #[test]
    fn redaction() {
        let mut v = serde_json::json!({
            "id": "1", "email": "a@b", "token": "x", "author": {"id": "2", "phone": "1", "mfa_enabled": true},
            "attachments": [{"url": "https://cdn.discordapp.com/a.png?ex=1&is=2&hm=3"}],
            "content": "hello"
        });
        redact(&mut v);
        let s = v.to_string();
        assert!(
            !s.contains("email")
                && !s.contains("token")
                && !s.contains("phone")
                && !s.contains("mfa")
        );
        assert!(s.contains("https://cdn.discordapp.com/a.png\"") && s.contains("hello"));
    }

    #[test]
    fn settings_allowlist() {
        assert!(setting_allowed("appearance.message-display-compact"));
        assert!(!setting_allowed("localization.timezone"));
        assert!(!setting_allowed("privacy.email-visible"));
        assert!(!setting_allowed("random.key"));
    }
}
