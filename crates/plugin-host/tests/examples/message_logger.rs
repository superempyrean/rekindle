use rekindle_plugin_host::backend::fixtures::{ALICE, BOT, GENERAL};
use rekindle_plugin_host::model::{Element, Span};
use rekindle_plugin_host::settings::SettingValue;
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/message-logger"
);

fn harness() -> Harness {
    Harness::for_crate(PLUGIN).unwrap()
}

#[test]
fn a_deleted_message_comes_back_as_a_ghost_row() {
    let mut h = harness();
    let m = h.receive_message(GENERAL, ALICE, "oops, wrong channel");
    h.delete_message(GENERAL, m.id);
    let ghosts = h.ghosts(GENERAL);
    assert_eq!(ghosts.len(), 1);
    assert_eq!((ghosts[0].id, ghosts[0].style.as_str()), (m.id, "deleted"));
    assert_eq!(ghosts[0].content, vec![Span::plain("oops, wrong channel")]);
    assert_eq!(ghosts[0].author.id, ALICE);
    // Kept on this device, and shown again after a restart.
    h.restart().unwrap();
    assert_eq!(h.ghosts(GENERAL).len(), 1);
    h.command("clear-log");
    assert!(h.ghosts(GENERAL).is_empty());
    assert!(h.stored(&format!("del:{GENERAL}:{}", m.id)).is_none());
}

#[test]
fn edits_keep_a_history_under_the_message() {
    let mut h = harness();
    let m = h.receive_message(GENERAL, ALICE, "teh plan");
    h.edit_message(GENERAL, m.id, "the plan");
    h.edit_message(GENERAL, m.id, "the new plan");
    let acc = h.accessory(m.id).expect("history");
    assert_eq!(
        acc[0],
        Element::Text {
            text: "Edited 2 time(s). Before:".into()
        }
    );
    let texts: Vec<&str> = acc[1..]
        .iter()
        .filter_map(|e| match e {
            Element::RichText { spans } => Some(spans[0].text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["the plan", "teh plan"], "newest first");
}

#[test]
fn settings_decide_what_is_kept() {
    let mut h = Harness::builder(PLUGIN)
        .setting("ignored-users", SettingValue::Ids(vec![ALICE]))
        .start()
        .unwrap();
    let a = h.receive_message(GENERAL, ALICE, "ignored");
    h.delete_message(GENERAL, a.id);
    let b = h.receive_message(GENERAL, BOT, "beep");
    h.delete_message(GENERAL, b.id);
    assert!(
        h.ghosts(GENERAL).is_empty(),
        "ignored user and a bot (ignore-bots defaults on)"
    );

    h.set_setting("ignore-bots", SettingValue::Boolean(false));
    let c = h.receive_message(GENERAL, BOT, "boop");
    h.delete_message(GENERAL, c.id);
    assert_eq!(h.ghosts(GENERAL).len(), 1);

    h.set_setting("log-deletes", SettingValue::Boolean(false));
    let d = h.receive_message(GENERAL, BOT, "not kept");
    h.delete_message(GENERAL, d.id);
    assert_eq!(h.ghosts(GENERAL).len(), 1);
}

#[test]
fn the_settings_page_comes_from_the_manifest() {
    let h = harness();
    let page = h.settings_page().expect("schema");
    let keys: Vec<&str> = page.entries.iter().map(|e| e.spec.key.as_str()).collect();
    assert_eq!(
        keys,
        ["log-deletes", "log-edits", "ignore-bots", "ignored-users"],
        "declaration order"
    );
}
