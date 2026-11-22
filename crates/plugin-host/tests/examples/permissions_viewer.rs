use rekindle_plugin_host::backend::fixtures::{ALICE, GENERAL, LAB};
use rekindle_plugin_host::model::Element;
use rekindle_plugin_host::testing::Harness;

use crate::toolchain::needs;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/ts/permissions-viewer"
);

fn lines(h: &Harness) -> Vec<String> {
    h.modal()
        .unwrap_or_else(|| {
            panic!(
                "no dialog; log:
{}",
                h.log_text()
            )
        })
        .elements
        .iter()
        .filter_map(|e| match e {
            Element::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn lists_a_members_permissions_in_the_server() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    let t = h.user_target(ALICE, Some(LAB));
    h.click_slot("view-permissions", t);
    assert_eq!(
        lines(&h),
        [
            "In Rekindle Lab",
            "• Add Reactions",
            "• View Channel",
            "• Send Messages"
        ]
    );
    h.click("permissions", "close");
    assert!(h.modal().is_none());
}

#[test]
fn the_channel_menu_shows_your_own() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    let t = h.channel_target(GENERAL);
    h.click_slot("view-permissions", t);
    // The user owns the Lab, so every permission applies.
    assert_eq!(lines(&h)[1..], ["• Administrator (every permission)"]);
}
