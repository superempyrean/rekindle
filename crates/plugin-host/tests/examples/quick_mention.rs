use rekindle_plugin_host::MockBackend;
use rekindle_plugin_host::backend::fixtures::{CAROL, GENERAL};
use rekindle_plugin_host::model::{Action, SLOT_MESSAGE_HOVER_BAR, SLOT_MESSAGE_MENU};
use rekindle_plugin_host::testing::Harness;

use crate::toolchain::needs;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/ts/quick-mention"
);

#[test]
fn mentions_the_author_from_the_hover_bar_and_the_menu() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    assert_eq!(h.slot_items(SLOT_MESSAGE_HOVER_BAR).len(), 1);
    assert_eq!(h.slot_items(SLOT_MESSAGE_MENU).len(), 1);
    let m = h.receive_message(GENERAL, CAROL, "hello");
    let t = h.message_target(GENERAL, m.id);
    h.click_slot("mention", t);
    assert_eq!(
        h.performed(),
        vec![Action::InsertText {
            text: format!("<@{CAROL}> ")
        }],
        "{}",
        h.log_text()
    );
}

#[test]
fn runs_without_a_hover_bar() {
    let mut b = MockBackend::new();
    b.set_slot_available(SLOT_MESSAGE_HOVER_BAR, false);
    let h = Harness::builder(needs!(PLUGIN)).backend(b).start().unwrap();
    h.assert_active();
    assert_eq!(
        h.slot_items(SLOT_MESSAGE_MENU).len(),
        1,
        "the menu item is still there"
    );
}
