use rekindle_plugin_host::backend::fixtures::{ALICE, GENERAL, LAB};
use rekindle_plugin_host::model::Element;
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/rust/view-raw");

fn shown_json(h: &Harness) -> String {
    h.modal()
        .expect("modal")
        .elements
        .iter()
        .find_map(|e| match e {
            Element::RichText { spans } => Some(spans[0].text.clone()),
            _ => None,
        })
        .unwrap()
}

#[test]
fn shows_and_copies_a_messages_json() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let id = h.with_world(|w| w.messages[&GENERAL][0].id);
    let t = h.message_target(GENERAL, id);
    h.click_slot("view-raw", t);
    let json = shown_json(&h);
    assert!(json.contains("hello everyone!"), "{json}");
    h.click("view-raw", "copy");
    assert_eq!(h.clipboard().as_deref(), Some(json.as_str()));
}

#[test]
fn guilds_and_users_too() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let g = h.guild_target(LAB);
    h.click_slot("view-raw", g);
    assert!(shown_json(&h).contains("Rekindle Lab"));
    let u = h.user_target(ALICE, Some(LAB));
    h.click_slot("view-raw", u);
    assert!(shown_json(&h).contains("alice"));
}
