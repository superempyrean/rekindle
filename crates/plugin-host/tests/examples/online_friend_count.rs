use rekindle_plugin_host::backend::fixtures::ALICE;
use rekindle_plugin_host::model::{SLOT_GUILD_LIST_BUTTON, SlotItem};
use rekindle_plugin_host::testing::Harness;

use crate::toolchain::needs;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/ts/online-friend-count"
);

fn tooltip(h: &Harness) -> String {
    match &h.slot_items(SLOT_GUILD_LIST_BUTTON)[..] {
        [SlotItem::IconButton { tooltip, .. }] => tooltip.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn counts_friends_online_and_follows_presence() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    // Alice is the one friend, and she's online.
    assert_eq!(tooltip(&h), "1 friend online");
    let mut p = h.with_world(|w| w.presences[&ALICE].clone());
    p.status = "offline".into();
    h.presence(p);
    assert_eq!(tooltip(&h), "0 friends online");
}

#[test]
fn a_click_lists_them() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    h.click_slot("online-friends", rekindle_plugin_host::api::Target::None);
    let modal = h.modal().expect("the list");
    assert!(
        format!("{:?}", modal.elements).contains(&format!("User {{ id: {ALICE} }}")),
        "{:?}",
        modal.elements
    );
}
