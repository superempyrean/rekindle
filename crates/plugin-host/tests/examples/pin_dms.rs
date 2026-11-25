use rekindle_plugin_host::MockBackend;
use rekindle_plugin_host::backend::fixtures::{ALICE, DM_ALICE};
use rekindle_plugin_host::model::{SLOT_CHANNEL_MENU, SLOT_USER_MENU};
use rekindle_plugin_host::testing::Harness;

use crate::toolchain::needs;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/ts/pin-dms");

fn pinned(h: &Harness) -> Vec<u64> {
    h.dm_sections()
        .into_iter()
        .flat_map(|s| s.channels)
        .collect()
}

#[test]
fn pins_and_unpins_a_dm_from_its_menu() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    assert!(pinned(&h).is_empty());
    let t = h.channel_target(DM_ALICE);
    h.click_slot("pin-dm", t.clone());
    assert_eq!(pinned(&h), [DM_ALICE], "{}", h.log_text());
    h.click_slot("pin-dm", t);
    assert!(pinned(&h).is_empty());
}

#[test]
fn the_user_menu_pins_their_dm_and_pins_survive_a_restart() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    let t = h.user_target(ALICE, None);
    h.click_slot("pin-dm", t);
    assert_eq!(pinned(&h), [DM_ALICE]);
    h.restart().unwrap();
    assert_eq!(pinned(&h), [DM_ALICE]);
}

#[test]
fn starts_without_dm_sections_or_menus() {
    let mut b = MockBackend::new();
    b.set_dm_sections_available(false);
    b.set_slot_available(SLOT_CHANNEL_MENU, false);
    b.set_slot_available(SLOT_USER_MENU, false);
    let h = Harness::builder(needs!(PLUGIN)).backend(b).start().unwrap();
    h.assert_active();
    assert!(pinned(&h).is_empty());
}
