use rekindle_plugin_host::backend::fixtures::ALICE;
use rekindle_plugin_host::model::{Action, UiValue};
use rekindle_plugin_host::testing::Harness;

use crate::toolchain::needs;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/ts/user-volume");

#[test]
fn sets_an_exact_volume_from_the_dialog() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    let t = h.user_target(ALICE, None);
    h.click_slot("set-volume", t);
    assert!(h.modal().is_some());
    h.ui("volume", "percent", UiValue::Number(155.0));
    h.click("volume", "apply");
    assert_eq!(
        h.performed(),
        vec![Action::SetUserVolume {
            user: ALICE,
            percent: 155.0
        }],
        "{}",
        h.log_text()
    );
}

#[test]
fn a_typed_percentage_is_kept_in_range() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    let t = h.user_target(ALICE, None);
    h.click_slot("set-volume", t);
    h.type_text("volume", "typed", "250");
    h.click("volume", "apply");
    // 250 is out of range: the volume stays at 100.
    h.type_text("volume", "typed", "40%");
    h.click("volume", "apply");
    assert_eq!(
        h.performed(),
        vec![
            Action::SetUserVolume {
                user: ALICE,
                percent: 100.0
            },
            Action::SetUserVolume {
                user: ALICE,
                percent: 40.0
            }
        ],
        "{}",
        h.log_text()
    );
}
