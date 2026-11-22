use rekindle_plugin_host::backend::fixtures::{ALICE, GENERAL};
use rekindle_plugin_host::settings::SettingValue;
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/no-reply-mention"
);

fn alices(h: &Harness) -> u64 {
    h.with_world(|w| {
        w.messages[&GENERAL]
            .iter()
            .find(|m| m.author.id == ALICE)
            .unwrap()
            .id
    })
}

#[test]
fn replies_stop_pinging() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let d = h.reply_draft(GENERAL, alices(&h), "sounds good", true);
    let out = h.compose(&d);
    assert_eq!(out.mention, Some(false));
    assert_eq!(out.text, "sounds good");
    assert_eq!(out.rewritten_by, vec![h.id().to_string()]);
    // A reply that already doesn't ping, and a plain send, are left alone.
    let quiet = h.reply_draft(GENERAL, alices(&h), "ok", false);
    assert!(h.compose(&quiet).rewritten_by.is_empty());
    assert!(h.send_message(GENERAL, "hi").rewritten_by.is_empty());
}

#[test]
fn people_you_list_still_get_pinged() {
    let mut h = Harness::builder(PLUGIN)
        .setting("always-ping", SettingValue::Ids(vec![ALICE]))
        .start()
        .unwrap();
    let d = h.reply_draft(GENERAL, alices(&h), "hey", true);
    assert_eq!(h.compose(&d).mention, None);
}
