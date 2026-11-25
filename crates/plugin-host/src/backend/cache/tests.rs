use std::collections::BTreeMap;

use serde_json::json;

use super::*;

/// A store in memory: table to key to value.
#[derive(Default)]
struct Memory(BTreeMap<String, BTreeMap<String, Vec<u8>>>);

impl Memory {
    fn put(&mut self, table: &str, k: &str, v: &serde_json::Value) {
        self.0
            .entry(table.into())
            .or_default()
            .insert(k.into(), serde_json::to_vec(v).unwrap());
    }
}

impl StoreReader for Memory {
    fn get(&self, table: &str, k: &str) -> Option<Vec<u8>> {
        self.0.get(table)?.get(k).cloned()
    }
    fn get_many(&self, table: &str, keys: &[String]) -> Vec<(String, Vec<u8>)> {
        keys.iter()
            .filter_map(|k| Some((k.clone(), self.get(table, k)?)))
            .collect()
    }
    fn scan(&self, table: &str, prefix: &str) -> Vec<(String, Vec<u8>)> {
        self.0
            .get(table)
            .into_iter()
            .flatten()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

const ME: u64 = 10;
const FRIEND: u64 = 11;
const GUILD: u64 = 100;
const MOD: u64 = 101;
const GENERAL: u64 = 200;
const SECRET: u64 = 201;
const DM_ID: u64 = 300;

fn user(id: u64) -> serde_json::Value {
    let name = if id == ME { "me" } else { "friend" };
    json!({"id": id.to_string(), "username": name, "global_name": null, "discriminator": "0", "avatar": null, "bot": false})
}

fn channel(id: u64, name: &str) -> serde_json::Value {
    json!({"id": id.to_string(), "type": 0, "guild_id": GUILD.to_string(), "name": name, "position": 0, "nsfw": false, "recipients": []})
}

fn member(id: u64, roles: &[u64], nick: Option<&str>) -> serde_json::Value {
    let roles: Vec<String> = roles.iter().map(u64::to_string).collect();
    json!({"user": null, "user_id": id.to_string(), "nick": nick, "roles": roles, "joined_at": "2026-01-03T08:27:21.677000+00:00", "deaf": false, "mute": false, "flags": 0})
}

fn message(id: u64, author: u64, content: &str) -> serde_json::Value {
    json!({"id": id.to_string(), "channel_id": GENERAL.to_string(), "author": user(author), "content": content, "timestamp": "2026-11-24T16:13:08.672000+00:00"})
}

/// A guild with `@everyone` and a moderator role, a public and a hidden
/// channel, two members, a DM, a friend and two messages, keyed as the
/// guest keys them.
fn store() -> CacheStore {
    let mut m = Memory::default();
    let mut me = user(ME);
    // The guest keeps the user's own `store_country` as an object.
    me["store_country"] = json!({"country_code": "US"});
    m.put("meta", "me", &me);
    m.put(
        "meta",
        "selected",
        &json!({"guild_id": GUILD.to_string(), "channel_id": GENERAL.to_string(), "favorites": false}),
    );
    m.put("meta", "guild_order", &json!([GUILD.to_string()]));
    m.put("users", &key(ME), &user(ME));
    m.put("users", &key(FRIEND), &user(FRIEND));
    m.put(
        "guilds",
        &key(GUILD),
        &json!({
            "properties": {"id": GUILD.to_string(), "name": "Test Guild", "owner_id": FRIEND.to_string(), "features": []},
            "roles": [
                {"id": GUILD.to_string(), "name": "@everyone", "color": 0, "position": 0, "permissions": "1024"},
                {"id": MOD.to_string(), "name": "Mod", "color": 0x00ff00, "position": 1, "permissions": "8192"}
            ],
            "emojis": [], "stickers": [], "member_count": 2, "joined_at_ms": 1_700_000_000_000u64
        }),
    );
    m.put(
        "user_guild_settings",
        &key(GUILD),
        &json!({"guild_id": GUILD.to_string(), "muted": true}),
    );
    m.put(
        "channels",
        &pair(GUILD, GENERAL),
        &channel(GENERAL, "general"),
    );
    m.put("channels", &pair(GUILD, SECRET), &channel(SECRET, "secret"));
    m.put(
        "channel_topics",
        &pair(GUILD, GENERAL),
        &json!("Read the rules"),
    );
    m.put(
        "channel_overwrites",
        &pair(GUILD, SECRET),
        &json!([{"id": GUILD.to_string(), "type": 0, "allow": "0", "deny": "1024"},
                {"id": MOD.to_string(), "type": 0, "allow": "1024", "deny": "0"}]),
    );
    m.put("members", &pair(GUILD, ME), &member(ME, &[], None));
    m.put(
        "members",
        &pair(GUILD, FRIEND),
        &member(FRIEND, &[MOD], Some("Boss")),
    );
    m.put(
        "channels",
        &pair(0, DM_ID),
        &json!({"id": DM_ID.to_string(), "type": 1, "recipient_ids": [FRIEND.to_string()], "last_message_id": "401"}),
    );
    m.put(
        "relationships",
        &key(FRIEND),
        &json!({"id": FRIEND.to_string(), "type": 1, "user_id": FRIEND.to_string(), "since": "2025-02-14T00:06:29.060000+00:00"}),
    );
    m.put(
        "messages",
        &pair(GENERAL, 400),
        &message(400, FRIEND, "first"),
    );
    m.put("messages", &pair(GENERAL, 401), &message(401, ME, "second"));
    CacheStore::new(Box::new(m))
}

#[test]
fn users_and_the_user() {
    let s = store();
    assert_eq!(s.me().unwrap().id, ME);
    assert_eq!(s.user(FRIEND).unwrap().username, "friend");
    assert!(s.user(12).is_none());
    assert_eq!(s.selection(), Some((Some(GUILD), Some(GENERAL))));
}

#[test]
fn guilds_and_roles() {
    let s = store();
    assert_eq!(s.guild_ids(), vec![GUILD]);
    let g = s.guild(GUILD).unwrap();
    assert_eq!(
        (g.name.as_str(), g.owner, g.member_count),
        ("Test Guild", FRIEND, Some(2))
    );
    assert_eq!(g.joined_ms, Some(1_700_000_000_000));
    assert!(g.muted);
    assert_eq!(s.roles(GUILD).len(), 2);
    assert_eq!(s.role(GUILD, MOD).unwrap().name, "Mod");
}

#[test]
fn channels_by_id_and_by_guild() {
    let s = store();
    let general = s.channel(GENERAL).unwrap();
    assert_eq!(general.guild, Some(GUILD));
    assert_eq!(general.topic.as_deref(), Some("Read the rules"));
    assert!(general.visible);
    // `@everyone` can't view the secret channel, and the user has no role.
    assert!(!s.channel(SECRET).unwrap().visible);
    let shown: Vec<u64> = s.channels(GUILD, false).iter().map(|c| c.id).collect();
    assert_eq!(shown, vec![GENERAL]);
    assert_eq!(s.channels(GUILD, true).len(), 2);
    assert_eq!(s.permission_overwrites(SECRET).len(), 2);
}

#[test]
fn dms_are_guild_zero() {
    let s = store();
    let dms = s.private_channels();
    assert_eq!(dms.len(), 1);
    assert_eq!(dms[0].name, "friend");
    assert_eq!(dms[0].guild, None);
    assert_eq!(s.channel(DM_ID).unwrap().id, DM_ID);
    assert_eq!(s.dm_channel(FRIEND), Some(DM_ID));
}

#[test]
fn members_and_permissions() {
    let s = store();
    let m = s.member(GUILD, FRIEND).unwrap();
    assert_eq!(m.nick.as_deref(), Some("Boss"));
    assert_eq!(s.display_name(Some(GUILD), FRIEND), "Boss");
    assert_eq!(s.display_name(None, FRIEND), "friend");
    let (members, total) = s.loaded_members(GUILD, 1);
    assert_eq!((members.len(), total), (1, 2));
    // The owner has every permission; the user has `@everyone`'s.
    assert_eq!(s.member_permissions(FRIEND, GUILD), Some(ALL_PERMISSIONS));
    assert_eq!(s.my_permissions(GUILD), VIEW_CHANNEL);
    assert_eq!(s.my_permissions(SECRET) & VIEW_CHANNEL, 0);
    assert_eq!(s.member_color(GUILD, FRIEND), Some(0x00ff00));
}

#[test]
fn relationships_and_messages() {
    let s = store();
    let r = s.relationships();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].user, FRIEND);
    let m = s.message(GENERAL, 400).unwrap();
    assert_eq!((m.content.as_str(), m.author.id), ("first", FRIEND));
    assert_eq!(s.message_in_selected(401).unwrap().author.id, ME);
    let newest: Vec<u64> = s
        .loaded_messages(GENERAL, None, 10)
        .iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(newest, vec![401, 400]);
    let older: Vec<u64> = s
        .loaded_messages(GENERAL, Some(401), 10)
        .iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(older, vec![400]);
}

#[test]
fn raw_rows() {
    let s = store();
    let target = api::Target::Guild(api::GuildRef {
        id: GUILD,
        name: String::new(),
    });
    assert!(s.raw_json(&target).unwrap().contains("Test Guild"));
    let none = api::Target::Guild(api::GuildRef {
        id: 1,
        name: String::new(),
    });
    assert!(s.raw_json(&none).is_none());
}
