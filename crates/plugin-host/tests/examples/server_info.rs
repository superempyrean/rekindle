use rekindle_plugin_host::backend::fixtures::{LAB, ME};
use rekindle_plugin_host::model::{Element, SLOT_GUILD_MENU};
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/server-info"
);

fn field<'a>(es: &'a [Element], key: &str) -> &'a str {
    es.iter()
        .find_map(|e| match e {
            Element::Field { key: k, value } if k == key => Some(value.as_str()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no field {key} in {es:#?}"))
}

#[test]
fn guild_menu_opens_a_modal_of_what_the_client_knows() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    assert_eq!(h.slot_items(SLOT_GUILD_MENU).len(), 1);
    let lab = h.guild_target(LAB);
    h.click_slot("info", lab);
    let m = h.modal().expect("modal");
    assert_eq!(m.title, "Rekindle Lab");
    assert_eq!(field(&m.elements, "Owner"), "You");
    assert!(m.elements.contains(&Element::User { id: ME }));
    assert_eq!(field(&m.elements, "Members"), "4");
    assert_eq!(field(&m.elements, "Channels"), "3 text, 1 voice, 1 hidden");
    assert_eq!(field(&m.elements, "Roles"), "3");
    // The host formats dates: an id from 2015 and a join date in 2025.
    assert!(
        field(&m.elements, "Created").contains("2015"),
        "{}",
        field(&m.elements, "Created")
    );
    assert!(field(&m.elements, "You joined").contains("ago"));
    h.click("server-info", "close");
    assert!(h.modal().is_none());
}

#[test]
#[should_panic(expected = "clicked on a guild")]
fn the_guild_menu_needs_a_guild() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let here = h.current_channel();
    h.click_slot("info", here);
}
