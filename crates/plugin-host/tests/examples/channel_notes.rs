use rekindle_plugin_host::backend::fixtures::{GENERAL, OFF_TOPIC};
use rekindle_plugin_host::model::{Element, UiValue};
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/channel-notes"
);

fn note_value(h: &Harness) -> String {
    h.panel("notes")
        .unwrap()
        .elements
        .iter()
        .find_map(|e| match e {
            Element::TextInput { value, .. } => Some(value.clone()),
            _ => None,
        })
        .expect("note input shown")
}

#[test]
fn notes_follow_the_selected_channel() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    // The sample client opens on #general.
    assert_eq!(
        h.panel("notes").unwrap().elements[0],
        Element::Heading {
            text: "#general".into()
        }
    );
    h.ui(
        "notes",
        "note",
        UiValue::Text("ask about the release".into()),
    );
    assert_eq!(
        h.stored(&format!("note:{GENERAL}")).as_deref(),
        Some("ask about the release")
    );

    h.select_channel(OFF_TOPIC);
    assert_eq!(
        h.panel("notes").unwrap().elements[0],
        Element::Heading {
            text: "#off-topic".into()
        }
    );
    assert_eq!(note_value(&h), "");
    h.select_channel(GENERAL);
    assert_eq!(note_value(&h), "ask about the release");

    h.command("clear-note");
    assert_eq!(h.stored(&format!("note:{GENERAL}")), None);
}
