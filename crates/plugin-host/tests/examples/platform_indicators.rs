use rekindle_plugin_host::api::{ClientStatus, Presence};
use rekindle_plugin_host::backend::fixtures::{ALICE, BOT, CAROL};
use rekindle_plugin_host::model::Icon;
use rekindle_plugin_host::settings::SettingValue;
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/platform-indicators"
);

fn presence(user: u64, desktop: &str, mobile: &str, web: &str) -> Presence {
    Presence {
        user,
        status: "online".into(),
        client_status: ClientStatus {
            desktop: desktop.into(),
            mobile: mobile.into(),
            web: web.into(),
            embedded: String::new(),
        },
        activities: vec![],
    }
}

#[test]
fn decorates_everyone_loaded_at_start() {
    let h = Harness::for_crate(PLUGIN).unwrap();
    // Alice: desktop + mobile; the bot: web only.
    let a = h.decoration("user.name-badge", ALICE).expect("alice");
    assert_eq!((a.icon, a.text.as_deref()), (Some(Icon::Code), Some("2")));
    assert_eq!(a.tooltip.as_deref(), Some("Desktop: online · Mobile: idle"));
    assert_eq!(
        h.decoration("member-list.trailing", BOT).unwrap().icon,
        Some(Icon::Globe)
    );
    assert!(h.decoration("user.name-badge", CAROL).is_some());
}

#[test]
fn follows_presence_updates() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.presence(presence(CAROL, "", "online", ""));
    assert_eq!(
        h.decoration("user.name-badge", CAROL).unwrap().icon,
        Some(Icon::Smile)
    );
    h.presence(presence(CAROL, "", "", ""));
    assert!(
        h.decoration("user.name-badge", CAROL).is_none(),
        "offline everywhere"
    );
}

#[test]
fn the_setting_picks_where_they_show() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    assert_eq!(
        h.setting("where"),
        Some(SettingValue::Selected(2)),
        "manifest default: Both"
    );
    h.set_setting("where", SettingValue::Selected(1));
    assert!(h.decorations("user.name-badge").is_empty());
    assert!(!h.decorations("member-list.trailing").is_empty());
}
