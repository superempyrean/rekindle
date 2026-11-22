use rekindle_plugin_host::backend::fixtures::{ALICE, CAROL, GENERAL};
use rekindle_plugin_host::model::{ToastKind, UiValue};
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/keyword-notify"
);

const PANEL: &str = "keyword-notify";

fn harness() -> Harness {
    Harness::for_crate(PLUGIN).unwrap()
}

#[test]
fn toasts_on_a_watched_word_and_counts() {
    let mut h = harness();
    h.ui(PANEL, "keywords", UiValue::Text("deploy, release".into()));
    h.receive_message(GENERAL, ALICE, "we should DEPLOY on friday");
    h.assert_active();
    let t = h.toasts();
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].kind, ToastKind::Info);
    assert!(
        t[0].text.contains("deploy") && t[0].text.contains("Alice"),
        "{}",
        t[0].text
    );
    // A message with no keyword does nothing.
    h.receive_message(GENERAL, CAROL, "lunch?");
    assert_eq!(h.toasts().len(), 1);
    // Only keywords + counts are stored, never message content.
    assert_eq!(h.stored("keywords").as_deref(), Some("deploy,release"));
    assert_eq!(h.stored("hits:deploy").as_deref(), Some("1"));
    assert!(
        h.runtime()
            .storage("dev.rekindle.keyword-notify")
            .unwrap()
            .lock()
            .unwrap()
            .entries()
            .values()
            .all(|v| !v.contains("friday"))
    );
}

#[test]
fn mute_counts_without_toasting() {
    let mut h = harness();
    h.ui(PANEL, "keywords", UiValue::Text("ping".into()));
    h.toggle(PANEL, "mute", true);
    h.receive_message(GENERAL, CAROL, "ping ping ping");
    assert!(h.toasts().is_empty());
    assert_eq!(h.stored("hits:ping").as_deref(), Some("1"));
}
