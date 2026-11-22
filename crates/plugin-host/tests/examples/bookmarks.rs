use rekindle_plugin_host::MockBackend;
use rekindle_plugin_host::api::{MessageRef, Target};
use rekindle_plugin_host::model::{
    Element, SLOT_CHAT_BAR_BUTTON, SLOT_MESSAGE_MENU, SLOT_PROFILE_SECTION, SlotItem, ToastKind,
};
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/rust/bookmarks");

fn msg(id: u64, author: u64) -> Target {
    Target::Message(MessageRef {
        id,
        channel: 10,
        guild: None,
        author,
    })
}

#[test]
fn registers_its_slots() {
    let h = Harness::for_crate(PLUGIN).unwrap();
    let menu: Vec<String> = h
        .slot_items(SLOT_MESSAGE_MENU)
        .iter()
        .map(|i| i.id().to_string())
        .collect();
    assert_eq!(menu, vec!["bookmark", "unbookmark"]);
    assert!(
        matches!(&h.slot_items(SLOT_CHAT_BAR_BUTTON)[0], SlotItem::IconButton { id, .. } if id == "open")
    );
    assert_eq!(h.slot_items(SLOT_PROFILE_SECTION).len(), 1);
}

#[test]
fn bookmark_from_menu_then_browse_and_remove() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.click_slot("bookmark", msg(1, 7));
    h.click_slot("bookmark", msg(2, 8));
    h.click_slot("bookmark", msg(1, 7));
    assert_eq!(h.toasts().last().unwrap().text, "Already bookmarked");
    assert_eq!(h.stored("bookmarks").as_deref(), Some("1,10,7;2,10,8"));

    let here = h.current_channel();
    h.click_slot("open", here);
    let modal = h.modal().expect("modal shown");
    assert_eq!(
        modal
            .elements
            .iter()
            .filter(|e| matches!(e, Element::Button { .. }))
            .count(),
        4
    ); // 2 remove + all + close
    h.click("bookmarks", "remove-0");
    assert_eq!(h.stored("bookmarks").as_deref(), Some("2,10,8"));
    h.click("bookmarks", "close");
    assert!(h.modal().is_none());

    h.restart().unwrap();
    h.click_slot("unbookmark", msg(2, 8));
    assert_eq!(h.toasts().last().unwrap().kind, ToastKind::Info);
    assert_eq!(h.stored("bookmarks").as_deref(), Some(""));
}

#[test]
fn profile_section_counts_that_users_bookmarks() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.click_slot("bookmark", msg(1, 7));
    h.click_slot("bookmark", msg(2, 7));
    h.open_profile(7, "maple");
    assert_eq!(
        h.profile_section("profile", 7),
        Some(vec![Element::Text {
            text: "You bookmarked 2 messages from maple.".into()
        }])
    );
    h.open_profile(8, "quill");
    assert!(
        matches!(&h.profile_section("profile", 8).unwrap()[0], Element::Text { text } if text.contains("haven't"))
    );
}

#[test]
fn degrades_when_a_slot_is_missing_on_this_build() {
    let mut b = MockBackend::new();
    b.set_slot_available(SLOT_CHAT_BAR_BUTTON, false);
    let h = Harness::builder(PLUGIN).backend(b).start().unwrap();
    h.assert_active();
    assert!(h.slot_items(SLOT_CHAT_BAR_BUTTON).is_empty());
    assert_eq!(
        h.slot_items(SLOT_MESSAGE_MENU).len(),
        2,
        "other slots still work"
    );
    assert!(h.log_text().contains("isn't available"));
}
