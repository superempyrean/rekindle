use rekindle_plugin_host::api::{ClientNotification, NotificationDecision, NotificationKind};
use rekindle_plugin_host::backend::fixtures::{GENERAL, OFF_TOPIC};
use rekindle_plugin_host::settings::SettingValue;
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/notification-router"
);

fn notification(channel: u64, body: &str) -> ClientNotification {
    ClientNotification {
        kind: NotificationKind::Message,
        guild: None,
        channel: Some(channel),
        author: None,
        message: None,
        title: "Alice".into(),
        body: body.into(),
        sound: Some("message1".into()),
    }
}

#[test]
fn routes_by_channel_and_keyword() {
    let mut h = Harness::builder(PLUGIN)
        .setting("quiet-channels", SettingValue::Ids(vec![OFF_TOPIC]))
        .setting("silent-words", SettingValue::Items(vec!["standup".into()]))
        .start()
        .unwrap();
    assert_eq!(
        h.filter_notification(&notification(OFF_TOPIC, "hi")),
        NotificationDecision::Suppress
    );
    assert_eq!(
        h.filter_notification(&notification(GENERAL, "Standup in 5")),
        NotificationDecision::Silent
    );
    assert_eq!(
        h.filter_notification(&notification(GENERAL, "lunch?")),
        NotificationDecision::Show
    );
}
