use super::*;
use crate::backend::{MockBackend, World};

const SESSION: &str = include_str!("session.jsonl");

/// The fixture's dispatches: (at, t, d).
fn session() -> Vec<(u64, String, Value)> {
    SESSION
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: Value = serde_json::from_str(l).expect("fixture line");
            (
                v["at"].as_u64().unwrap(),
                v["t"].as_str().unwrap().to_string(),
                v["d"].clone(),
            )
        })
        .collect()
}

/// One dispatch through rekindle and the simulated client.
fn step(client: &mut MockBackend, t: &str, d: Value, at: u64) -> Vec<Event> {
    let dispatch = Dispatch::new(t, d, client);
    client.process_dispatch(t, &dispatch.d, at);
    dispatch.events(client, at)
}

/// The fixture up to (not including) the first dispatch named `until`:
/// the client, and every event tagged by its dispatch.
fn run(until: Option<&str>) -> (MockBackend, Vec<(String, Event)>) {
    let mut client = MockBackend::with_world(World::empty());
    let mut events = Vec::new();
    for (at, t, d) in session() {
        if Some(t.as_str()) == until {
            break;
        }
        for e in step(&mut client, &t, d, at) {
            events.push((t.clone(), e));
        }
    }
    (client, events)
}

fn of<'a>(events: &'a [(String, Event)], t: &str) -> Vec<&'a Event> {
    events
        .iter()
        .filter(|(x, _)| x == t)
        .map(|(_, e)| e)
        .collect()
}

#[test]
fn no_secret_reaches_a_plugin() {
    let (client, events) = run(None);
    for (t, e) in &events {
        let s = format!("{e:?}");
        assert!(
            !s.contains("SECRET"),
            "{t} produced an event holding a secret: {s}"
        );
    }
    // The simulated client decodes with the same records, so its world
    // (what the mock's queries answer from) holds none either.
    assert!(
        !serde_json::to_string(&client.world)
            .unwrap()
            .contains("SECRET")
    );
}

#[test]
fn credentials_and_session_dispatches_are_never_raw() {
    let (_, events) = run(None);
    let raw: Vec<&str> = events
        .iter()
        .filter_map(|(_, e)| {
            if let Event::Raw((n, _)) = e {
                Some(n.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(raw, ["GUILD_AUDIT_LOG_ENTRY_CREATE"]);
    let Some(Event::Raw((_, json))) = events
        .iter()
        .map(|(_, e)| e)
        .find(|e| matches!(e, Event::Raw(_)))
    else {
        panic!("no raw event: {events:?}")
    };
    assert!(json.contains("\"action_type\":24") && !json.contains("session_token"));
    assert!(
        never_forwarded("AUTH_SESSION_CHANGE")
            && never_forwarded("VOICE_SERVER_UPDATE")
            && never_forwarded("READY")
    );
}

#[test]
fn ready_is_a_connection() {
    let (_, events) = run(Some("READY_SUPPLEMENTAL"));
    assert_eq!(
        of(&events, "READY"),
        [&Event::Connection(api::ConnectionState::Connected)]
    );
}

#[test]
fn a_lazy_guild_loads_without_a_join() {
    let (_, events) = run(Some("MESSAGE_CREATE"));
    assert!(
        matches!(of(&events, "GUILD_CREATE")[..], [Event::GuildUpdated(g)] if g.id == 201 && g.preferred_locale == "de")
    );
}

#[test]
fn a_guild_joined_just_now_is_a_join() {
    let (mut client, _) = run(Some("MESSAGE_CREATE"));
    let now = 1_793_000_100_000;
    let d = serde_json::json!({"id": "202", "joined_at": "2026-10-26T08:54:00Z",
        "properties": {"id": "202", "name": "New", "owner_id": "101"}, "channels": [], "roles": []});
    assert_eq!(
        convert::iso_ms("2026-10-26T08:54:00Z").map(|j| j + JOIN_WINDOW_MS > now),
        Some(true)
    );
    assert!(
        matches!(step(&mut client, "GUILD_CREATE", d, now)[..], [Event::GuildJoined(ref g)] if g.name == "New")
    );
}

#[test]
fn messages_edits_and_deletes() {
    let (_, events) = run(None);
    let Event::MessageCreated(m) = of(&events, "MESSAGE_CREATE")[0] else {
        panic!()
    };
    assert_eq!(
        (m.author.name.as_str(), m.content.as_str()),
        ("Ada L.", "hello @rk")
    );
    assert_eq!(m.mentions, [100]);
    assert!(m.attachments[0].spoiler);
    // The unfurl isn't an edit; the content change is, with what it said.
    let edits = of(&events, "MESSAGE_UPDATE");
    assert_eq!(edits.len(), 1);
    let Event::MessageEdited((m, before)) = edits[0] else {
        panic!()
    };
    assert_eq!(
        (m.content.as_str(), before.as_deref()),
        ("hello @rk!", Some("hello @rk"))
    );
    assert_eq!(m.edited_ms, Some(1_794_268_803_250));
    assert_eq!(m.embeds[0].title.as_deref(), Some("Example"));
    // The delete carries the client's copy, read before it processed the
    // delete: edited, with its reactions.
    let Event::MessageDeleted(d) = of(&events, "MESSAGE_DELETE")[0] else {
        panic!()
    };
    let cached = d.cached.as_ref().unwrap();
    assert_eq!(cached.content, "hello @rk!");
    assert_eq!(
        (
            cached.reactions[0].emoji.as_str(),
            cached.reactions[0].burst_count
        ),
        ("blob:400", 1)
    );
}

#[test]
fn reactions() {
    let (_, events) = run(Some("PRESENCE_UPDATE"));
    let changes: Vec<(u64, bool, bool, u64)> = events
        .iter()
        .filter_map(|(_, e)| {
            if let Event::Reaction(c) = e {
                Some((c.user, c.added, c.burst, c.message.author))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        changes,
        [
            (100, true, false, 101),
            (101, true, true, 101),
            (100, false, false, 101)
        ]
    );
}

#[test]
fn typing_snapshots_come_from_the_client() {
    let (mut client, events) = run(Some("MESSAGE_REACTION_ADD"));
    assert_eq!(
        of(&events, "TYPING_START").last().unwrap(),
        &&Event::Typing((350, vec![102, 101]))
    );
    // Grace's message ends her indicator in the client.
    assert!(of(&events, "MESSAGE_CREATE").contains(&&Event::Typing((350, vec![101]))));
    client.expire_typing(1_793_000_013_200);
    assert!(client.world.typing.is_empty());
}

#[test]
fn a_dm_counts_as_a_mention() {
    let (_, events) = run(Some("MESSAGE_REACTION_ADD"));
    let dm = of(&events, "MESSAGE_CREATE")
        .into_iter()
        .find_map(|e| {
            if let Event::ReadState(rs) = e {
                (rs.channel == 350).then_some(*rs)
            } else {
                None
            }
        })
        .unwrap();
    assert!(dm.unread && dm.mentions == 1);
}

#[test]
fn a_tick_coalesces_presence_and_typing() {
    let (_, events) = run(Some("VOICE_STATE_UPDATE"));
    let presences: Vec<Event> = of(&events, "PRESENCE_UPDATE")
        .into_iter()
        .cloned()
        .collect();
    assert_eq!(presences.len(), 2);
    let one = coalesce(&presences);
    let [Event::Presence(p)] = &one[..] else {
        panic!("{one:?}")
    };
    assert_eq!(
        (p.status.as_str(), p.client_status.mobile.as_str()),
        ("dnd", "online")
    );
    assert_eq!(p.activities[0].emoji.as_deref(), Some("🔥"));
}

#[test]
fn the_users_voice() {
    let (_, events) = run(None);
    let voice = of(&events, "VOICE_STATE_UPDATE");
    assert!(
        matches!(voice[0], Event::MyVoice(v) if v.channel == 303 && v.since_ms == 1_793_000_006_000)
    );
    assert!(matches!(voice[1], Event::VoiceState(v) if v.user == 100));
    assert_eq!(
        voice[2..],
        [&Event::MyVoiceLeft, &Event::VoiceLeft((100, 303))]
    );
}

#[test]
fn the_rest_of_the_session() {
    let (_, events) = run(None);
    assert!(
        matches!(of(&events, "GUILD_MEMBER_UPDATE")[..], [Event::MemberUpdated(m)] if m.roles == [210])
    );
    assert!(
        matches!(of(&events, "CHANNEL_UPDATE")[..], [Event::ChannelUpdated(c)] if c.name == "general-chat" && c.last_message == Some(5001))
    );
    assert_eq!(
        of(&events, "SESSIONS_REPLACE"),
        [&Event::MyPresence("idle".into())]
    );
    assert!(matches!(
        of(&events, "RELATIONSHIP_UPDATE")[..],
        [Event::Relationship(c)] if c.old == Some(api::RelationshipKind::Incoming) && c.new == Some(api::RelationshipKind::Friend) && c.by_you
    ));
    assert!(
        matches!(of(&events, "MESSAGE_ACK")[..], [Event::ReadState(rs)] if !rs.unread && rs.mentions == 0)
    );
    assert_eq!(
        of(&events, "GUILD_DELETE"),
        [&Event::GuildLeft((201, api::GuildLeftReason::Left))]
    );
    assert!(
        of(&events, "VOICE_SERVER_UPDATE").is_empty()
            && of(&events, "AUTH_SESSION_CHANGE").is_empty()
    );
}

#[test]
fn a_bad_dispatch_causes_nothing() {
    let (mut client, _) = run(None);
    assert!(
        step(
            &mut client,
            "MESSAGE_CREATE",
            serde_json::json!({"id": "x"}),
            0
        )
        .is_empty()
    );
    assert!(
        step(
            &mut client,
            "GUILD_MEMBER_UPDATE",
            serde_json::json!(null),
            0
        )
        .is_empty()
    );
}

#[test]
fn folders_come_from_the_settings_protobuf() {
    use crate::backend::ClientBackend;
    let (client, _) = run(Some("MESSAGE_CREATE"));
    let folders = client.guild_folders();
    assert_eq!(folders.len(), 1);
    let f = &folders[0];
    assert_eq!(
        (f.id, f.name.as_deref(), f.guilds.as_slice()),
        (Some(7), Some("Lab & Big"), &[200, 201][..])
    );
    assert_eq!(f.color.map(|c| (c.r, c.g, c.b)), Some((0x58, 0x65, 0xf2)));
    // 201 loaded after READY and still sits where the folder puts it.
    assert_eq!(client.guild_ids(), [200, 201]);
    // The partial update moves both out of the folder, 201 first; the
    // client's queries see it, and no plugin gets the dispatch.
    let (client, events) = run(Some("RELATIONSHIP_UPDATE"));
    let folders = client.guild_folders();
    assert_eq!(
        folders
            .iter()
            .map(|f| (f.id, f.guilds.clone()))
            .collect::<Vec<_>>(),
        [(None, vec![201]), (None, vec![200])]
    );
    assert_eq!(client.guild_ids(), [201, 200]);
    assert!(of(&events, "USER_SETTINGS_PROTO_UPDATE").is_empty());
}

#[test]
fn settings_without_folders_leave_them() {
    use prost::Message;
    let empty = settings::PreloadedUserSettings {
        guild_folders: None,
    };
    let b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        empty.encode_to_vec(),
    );
    assert_eq!(
        settings::decode(&b64).as_ref().and_then(settings::folders),
        None
    );
    assert!(settings::decode("not base64!").is_none());
    // Older settings list unfoldered guilds in `guild_positions`.
    let old = settings::PreloadedUserSettings {
        guild_folders: Some(settings::GuildFolders {
            folders: vec![],
            guild_positions: vec![5, 6],
        }),
    };
    let f = settings::folders(&old).unwrap();
    assert_eq!(
        f.iter().map(|f| f.guilds.clone()).collect::<Vec<_>>(),
        [vec![5], vec![6]]
    );
}

#[test]
fn timestamps() {
    use convert::iso_ms;
    assert_eq!(iso_ms("1970-01-01T00:00:00+00:00"), Some(0));
    assert_eq!(
        iso_ms("2026-11-10T00:00:03.250000+00:00"),
        Some(1_794_268_803_250)
    );
    assert_eq!(
        iso_ms("2026-11-10T02:00:03.25+02:00"),
        Some(1_794_268_803_250)
    );
    assert_eq!(iso_ms("2026-11-10T00:00:03Z"), Some(1_794_268_803_000));
    assert_eq!(iso_ms("yesterday"), None);
}

/// The fields the client's own models have beyond the basic ones, as a
/// READY user and a guild message carry them.
#[test]
fn profile_cosmetics_account_fields_and_message_extras_decode() {
    use super::{convert, wire};
    use serde_json::json;

    let u: wire::User = serde_json::from_value(json!({
        "id": "100", "username": "me", "discriminator": "0", "avatar": "a1",
        "banner": "b1", "premium_type": 2, "flags": 64,
        "avatar_decoration_data": { "asset": "d1", "sku_id": "7", "expires_at": 10 },
        "collectibles": { "nameplate": { "asset": "n1", "palette": "berry", "label": "x", "sku_id": "8" } },
        "display_name_styles": { "font_id": 3, "effect_id": 2, "colors": [16711680] },
        "primary_guild": { "identity_guild_id": "1000", "identity_enabled": true, "tag": "RK", "badge": "g1" },
        "email": "me@example.com", "mfa_enabled": true, "nsfw_allowed": false,
        "age_verification_status": 1, "locale": "en-US", "verified": true
    }))
    .unwrap();
    let mut user = api::User::default();
    convert::user_fields(&u, &mut user);
    assert_eq!(user.premium_type, 2);
    assert_eq!(user.flag_bits, 64);
    assert_eq!(
        user.avatar_decoration.as_ref().unwrap().expires_ms,
        Some(10_000)
    );
    assert_eq!(user.nameplate.as_ref().unwrap().palette, "berry");
    assert_eq!(user.display_name_style.as_ref().unwrap().font, 3);
    assert_eq!(
        user.primary_guild.as_ref().unwrap().tag.as_deref(),
        Some("RK")
    );
    assert!(user.has_decoration);
    assert_eq!(user.email.as_deref(), Some("me@example.com"));
    assert_eq!(user.age_verification_status, Some(1));

    // A partial user leaves what was known.
    let partial: wire::User = serde_json::from_value(json!({ "id": "100" })).unwrap();
    convert::user_fields(&partial, &mut user);
    assert_eq!(user.avatar.as_deref(), Some("a1"));

    let m: wire::Message = serde_json::from_value(json!({
        "id": "5", "channel_id": "1002", "content": "hi", "nonce": 1234,
        "application_id": "9", "position": 4,
        "mention_channels": [{ "id": "1003" }],
        "interaction_metadata": { "id": "11", "type": 2, "user": { "id": "101" }, "name": "ping" },
        "components": [{ "type": 1 }]
    }))
    .unwrap();
    let msg = convert::message(&m, Some(1000), None, &|_, _| String::new(), &|_, _| None);
    assert_eq!(msg.nonce.as_deref(), Some("1234"));
    assert_eq!(msg.application_id, Some(9));
    assert_eq!(msg.position, Some(4));
    assert_eq!(msg.mention_channels, vec![1003]);
    assert_eq!(msg.interaction.as_ref().unwrap().user, 101);
    assert!(
        msg.components_json
            .as_deref()
            .unwrap()
            .contains("\"type\":1")
    );

    let r: wire::Role = serde_json::from_value(json!({
        "id": "3", "name": "Booster", "icon": "i1",
        "tags": { "premium_subscriber": null, "bot_id": "77" }
    }))
    .unwrap();
    let role = convert::role(r, 1000);
    assert!(role.premium_subscriber);
    assert_eq!(role.bot, Some(77));
    assert!(!role.guild_connections);
}

/// On a client whose users can't be read, a message's author is named from
/// the dispatch: the global name, else the username.
#[test]
fn an_unknown_author_is_named_from_the_dispatch() {
    let client = MockBackend::with_world(World::empty());
    let d = serde_json::json!({
        "id": "5", "channel_id": "1002", "content": "hi",
        "author": { "id": "101", "username": "alice", "global_name": "Alice" }
    });
    let events = step(
        &mut MockBackend::with_world(World::empty()),
        "MESSAGE_CREATE",
        d,
        0,
    );
    assert!(
        matches!(events.first(), Some(Event::MessageCreated(m)) if m.author.name == "Alice"),
        "{events:?}"
    );
    let m: wire::Message = serde_json::from_value(serde_json::json!({
        "id": "5", "channel_id": "1002", "author": { "id": "101", "username": "alice" }
    }))
    .unwrap();
    let names = |g, u| display_name(&client, g, u);
    let msg = convert::message(&m, None, None, &names, &|_, _| None);
    assert_eq!(msg.author.name, "alice");
}

#[test]
fn ready_is_not_forwarded_but_messages_are() {
    assert!(!forwarded("READY"));
    assert!(!forwarded("AUTH_SESSION_CHANGE"));
    assert!(forwarded("MESSAGE_CREATE"));
    assert_eq!(
        dispatch_kind("MESSAGE_CREATE"),
        Some(api::EventKind::Message)
    );
}
