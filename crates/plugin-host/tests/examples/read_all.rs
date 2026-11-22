use rekindle_plugin_host::api::Target;
use rekindle_plugin_host::backend::ClientBackend;
use rekindle_plugin_host::model::{Action, SLOT_GUILD_LIST_BUTTON, SlotItem, ToastKind};
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/rust/read-all");

#[test]
fn the_button_marks_everything_read_on_click() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let items = h.slot_items(SLOT_GUILD_LIST_BUTTON);
    assert!(
        matches!(&items[..], [SlotItem::IconButton { tooltip, .. }] if tooltip == "Mark 2 channels read")
    );
    h.click_slot("read-all", Target::None);
    assert_eq!(h.performed(), vec![Action::MarkAllRead]);
    assert!(h.backend().unread_channels(10).is_empty());
    assert_eq!(h.toasts().last().unwrap().kind, ToastKind::Success);
    assert!(
        matches!(&h.slot_items(SLOT_GUILD_LIST_BUTTON)[..], [SlotItem::IconButton { tooltip, .. }] if tooltip == "Nothing unread")
    );
}

#[test]
fn the_palette_command_is_a_gesture_too() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.command("read-all");
    assert_eq!(h.performed(), vec![Action::MarkAllRead]);
}
