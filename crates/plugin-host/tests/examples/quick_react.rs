use rekindle_plugin_host::backend::fixtures::GENERAL;
use rekindle_plugin_host::model::{Action, SLOT_MESSAGE_HOVER_BAR};
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/quick-react"
);

fn harness() -> Harness {
    Harness::for_crate(PLUGIN).unwrap()
}

#[test]
fn hover_bar_buttons_for_frequent_emoji() {
    let h = harness();
    let tips: Vec<String> = h
        .slot_items(SLOT_MESSAGE_HOVER_BAR)
        .iter()
        .map(|i| i.label().to_string())
        .collect();
    assert_eq!(
        tips,
        ["React with 👍", "React with rekindle", "React with 😂"]
    );
}

#[test]
fn reacts_and_unreacts() {
    let mut h = harness();
    let id = h.with_world(|w| w.messages[&GENERAL][1].id);
    let t = h.message_target(GENERAL, id);
    let r = h.message_ref(GENERAL, id);
    h.click_slot("react-1", t.clone());
    h.click_slot("react-1", t);
    assert_eq!(
        h.performed(),
        vec![
            Action::React {
                message: r,
                emoji: "rekindle:5001".into(),
                burst: false
            },
            Action::Unreact {
                message: r,
                emoji: "rekindle:5001".into(),
                burst: false
            },
        ]
    );
    assert!(h.with_world(|w| w.find_message(GENERAL, id).unwrap().reactions.is_empty()));
}
