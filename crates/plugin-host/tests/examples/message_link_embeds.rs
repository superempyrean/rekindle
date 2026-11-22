use rekindle_plugin_host::api::Event;
use rekindle_plugin_host::backend::fixtures::{CAROL, GENERAL, LAB, OFF_TOPIC, message_id};
use rekindle_plugin_host::backend::sample_message;
use rekindle_plugin_host::model::Element;
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/message-link-embeds"
);

fn link() -> String {
    format!(
        "https://discord.com/channels/{LAB}/{OFF_TOPIC}/{}",
        message_id(5, 9)
    )
}

#[test]
fn previews_a_linked_message_that_isnt_loaded() {
    let h = Harness::for_crate(PLUGIN).unwrap();
    // The sample client has a message in #general linking to an unloaded one
    // in #off-topic.
    let linker = h.with_world(|w| {
        w.messages[&GENERAL]
            .iter()
            .find(|m| m.content.contains("/channels/"))
            .unwrap()
            .id
    });
    let acc = h.accessory(linker).expect("preview");
    assert!(
        acc.contains(&Element::Text {
            text: "the build is green again".into()
        }),
        "{acc:#?}"
    );
}

#[test]
fn new_messages_with_links_get_previews() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let m = h.receive_message(GENERAL, CAROL, &format!("again: {}", link()));
    assert!(h.accessory(m.id).is_some());
}

#[test]
fn only_links_on_screen_resolve() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    // A message the client loaded but isn't showing (scrolled away).
    h.with_world(|w| w.visible.clear());
    let m = sample_message(
        GENERAL,
        Some(LAB),
        message_id(90, 1),
        (CAROL, "Carol"),
        &format!("off screen {}", link()),
    );
    h.with_world(|w| w.messages.get_mut(&GENERAL).unwrap().push(m.clone()));
    h.event(Event::MessageCreated(m.clone()));
    assert!(
        h.accessory(m.id).is_none(),
        "not visible: the host refuses to resolve the link"
    );
}
