use rekindle_plugin_host::backend::fixtures::{ALICE, CAROL};
use rekindle_plugin_host::settings::SettingValue;
use rekindle_plugin_host::testing::Harness;

use crate::toolchain::needs;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/ts/friend-notifications"
);

fn status(h: &mut Harness, user: u64, s: &str) {
    let mut p = h.with_world(|w| w.presences[&user].clone());
    p.status = s.into();
    h.presence(p);
}

/// Notification titles, without the " · Plugin Name" the host adds.
fn titles(h: &Harness) -> Vec<String> {
    h.notifications()
        .into_iter()
        .map(|n| n.title.split(" · ").next().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn a_friend_coming_online_notifies() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    // Alice (a friend) starts online: going idle is no change, offline then online is.
    status(&mut h, ALICE, "idle");
    status(&mut h, ALICE, "offline");
    assert!(
        titles(&h).is_empty(),
        "offline is off by default: {:?}",
        titles(&h)
    );
    status(&mut h, ALICE, "online");
    assert_eq!(titles(&h), ["Alice is online"], "{}", h.log_text());
}

#[test]
fn people_who_arent_friends_are_ignored() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    // Carol only sent a friend request.
    status(&mut h, CAROL, "offline");
    status(&mut h, CAROL, "online");
    assert!(titles(&h).is_empty());
}

#[test]
fn offline_too_and_only_the_friends_picked() {
    let mut h = Harness::builder(needs!(PLUGIN))
        .setting("offline", SettingValue::Boolean(true))
        .setting("only", SettingValue::Ids(vec![ALICE]))
        .start()
        .unwrap();
    status(&mut h, ALICE, "offline");
    assert_eq!(titles(&h), ["Alice went offline"]);
    let mut h = Harness::builder(needs!(PLUGIN))
        .setting("only", SettingValue::Ids(vec![999]))
        .start()
        .unwrap();
    status(&mut h, ALICE, "offline");
    status(&mut h, ALICE, "online");
    assert!(
        titles(&h).is_empty(),
        "Alice isn't one of the friends picked"
    );
}
