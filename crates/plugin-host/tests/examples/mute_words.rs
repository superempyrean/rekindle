use rekindle_plugin_host::api::Event;
use rekindle_plugin_host::backend::fixtures::{ALICE, GENERAL};
use rekindle_plugin_host::settings::SettingValue;
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/rust/mute-words");

fn muting(words: &[&str]) -> Harness {
    Harness::builder(PLUGIN)
        .setting(
            "words",
            SettingValue::Items(words.iter().map(std::string::ToString::to_string).collect()),
        )
        .start()
        .unwrap()
}

fn contents(h: &Harness) -> Vec<String> {
    h.backend()
        .world
        .messages
        .get(&GENERAL)
        .map(|ms| ms.iter().map(|m| m.content.clone()).collect())
        .unwrap_or_default()
}

#[test]
fn a_message_with_a_muted_word_never_reaches_the_client() {
    let mut h = muting(&["spoiler"]);
    h.receive_message(GENERAL, ALICE, "huge SPOILER for the finale");
    h.receive_message(GENERAL, ALICE, "good morning");
    let seen = contents(&h);
    assert!(!seen.iter().any(|c| c.contains("SPOILER")), "{seen:?}");
    assert!(seen.iter().any(|c| c == "good morning"), "{seen:?}");
    h.assert_active();
}

#[test]
fn other_events_and_unmuted_messages_pass_unchanged() {
    let mut h = muting(&["spoiler"]);
    let typing = Event::Typing((GENERAL, vec![ALICE]));
    assert!(
        h.intercept(typing).is_some(),
        "only message kinds were registered"
    );
    let m = h.receive_message(GENERAL, ALICE, "nothing to hide");
    assert!(
        matches!(h.intercept(Event::MessageCreated(m.clone())), Some(Event::MessageCreated(x)) if x.content == m.content)
    );
}

#[test]
fn with_no_words_nothing_is_dropped() {
    let mut h = muting(&[]);
    h.receive_message(GENERAL, ALICE, "spoiler");
    assert!(contents(&h).iter().any(|c| c == "spoiler"));
}

/// The same plugin on the gateway path: a dispatch it drops never reaches
/// the client, so the message isn't stored and the channel doesn't go unread.
#[test]
fn a_dropped_dispatch_never_reaches_the_client() {
    use rekindle_plugin_host::backend::gateway::Dispatch;
    use rekindle_plugin_host::backend::{MockBackend, World};
    let mut h = muting(&["spoiler"]);
    let mut client = MockBackend::with_world(World::empty());
    let step = |h: &mut Harness, client: &mut MockBackend, t: &str, d: serde_json::Value| {
        let dispatch = Dispatch::new(t, d, client).filter(h.runtime(), client)?;
        client.process_dispatch(t, &dispatch.d, 0);
        Some(dispatch.events(client, 0))
    };
    let ready = serde_json::json!({
        "user": {"id": "1", "username": "me"},
        "guilds": [{"id": "2", "properties": {"id": "2", "name": "G", "owner_id": "1"},
                    "roles": [{"id": "2", "permissions": "1024"}],
                    "channels": [{"id": "3", "type": 0, "name": "general", "last_message_id": "10"}]}],
        "read_state": {"entries": [{"id": "3", "last_message_id": "10"}]}
    });
    assert!(step(&mut h, &mut client, "READY", ready).is_some());
    let message = |id: &str, content: &str| {
        serde_json::json!({"id": id, "channel_id": "3", "guild_id": "2", "content": content,
                           "author": {"id": "9", "username": "alice"}, "mentions": []})
    };
    assert!(
        step(
            &mut h,
            &mut client,
            "MESSAGE_CREATE",
            message("11", "huge SPOILER ahead")
        )
        .is_none()
    );
    assert!(
        client
            .world
            .messages
            .get(&3)
            .is_none_or(std::vec::Vec::is_empty)
    );
    assert!(!client.world.read_states.get(&3).is_some_and(|rs| rs.unread));
    let events = step(
        &mut h,
        &mut client,
        "MESSAGE_CREATE",
        message("12", "good morning"),
    )
    .unwrap();
    assert!(
        matches!(&events[0], Event::MessageCreated(m) if m.id == 12 && m.content == "good morning")
    );
    assert!(client.world.read_states[&3].unread);
}
