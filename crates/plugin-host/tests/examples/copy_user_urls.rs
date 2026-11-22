use rekindle_plugin_host::MockBackend;
use rekindle_plugin_host::api::{Target, UserRef};
use rekindle_plugin_host::model::{SLOT_USER_MENU, SlotItem, ToastKind};
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/copy-user-urls"
);

fn user(id: u64, name: &str) -> Target {
    Target::User((
        UserRef {
            id,
            name: name.into(),
        },
        None,
    ))
}

#[test]
fn adds_a_user_menu_item() {
    let h = Harness::for_crate(PLUGIN).unwrap();
    let items = h.slot_items(SLOT_USER_MENU);
    assert!(
        matches!(&items[..], [SlotItem::MenuItem { id, label, .. }] if id == "copy-user-url" && label == "Copy User URL")
    );
}

#[test]
fn copies_the_users_url_on_click() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.click_slot("copy-user-url", user(80351110224678912, "maple"));
    assert_eq!(
        h.clipboard().as_deref(),
        Some("https://discord.com/users/80351110224678912")
    );
    let t = h.toasts().pop().unwrap();
    assert_eq!(
        (t.kind, t.text.as_str()),
        (ToastKind::Success, "Copied maple's URL")
    );
}

#[test]
fn toast_without_a_display_name() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.click_slot("copy-user-url", user(5, ""));
    assert_eq!(h.toasts().pop().unwrap().text, "Copied user URL");
}

/// No client can click a user-menu item without a user; the harness refuses
/// to pretend otherwise.
#[test]
#[should_panic(expected = "which is clicked on a user, not a none")]
fn a_user_menu_item_is_only_clicked_on_a_user() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.click_slot("copy-user-url", Target::None);
}

#[test]
fn degrades_without_user_menus() {
    let mut b = MockBackend::new();
    b.set_slot_available(SLOT_USER_MENU, false);
    let h = Harness::builder(PLUGIN).backend(b).start().unwrap();
    h.assert_active();
    assert!(h.slot_items(SLOT_USER_MENU).is_empty());
    // The runtime remembers the slot it asked for and didn't get, and
    // rekindle's page names the feature on the plugin's card and page.
    let mut h = h;
    let info = h.runtime().plugins().into_iter().next().unwrap();
    assert_eq!(info.missing_slots, [SLOT_USER_MENU]);
    let home = rekindle_plugin_host::home::Home::new(
        std::env::temp_dir().join(format!("rekindle-missing-{}", std::process::id())),
    );
    let mut m = rekindle_plugin_host::manager::Manager::new(home);
    m.set_missing(vec![(info.id, info.missing_slots)]);
    let text = format!("{:?}", m.panel(&[]).elements);
    assert!(
        text.contains("doesn't have every place rekindle can add to"),
        "{text}"
    );
    assert_eq!(
        rekindle_plugin_host::manager::slot_feature(SLOT_USER_MENU),
        "items in the user menu"
    );
}
