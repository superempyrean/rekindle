use rekindle_plugin_host::api::{CommandReply, OptionValue};
use rekindle_plugin_host::backend::ClientBackend;
use rekindle_plugin_host::model::SLOT_CHAT_BAR_BUTTON;
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/silent-typing"
);

fn tweak(h: &Harness) -> String {
    h.backend()
        .tweaks()
        .into_iter()
        .find(|t| t.id == "silent-typing")
        .unwrap()
        .value
}

#[test]
fn the_chat_bar_button_flips_the_tweak() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    assert_eq!(tweak(&h), "false");
    let here = h.current_channel();
    h.click_slot("toggle", here);
    assert_eq!(tweak(&h), "true");
    assert_eq!(
        h.backend()
            .is_active(h.id(), SLOT_CHAT_BAR_BUTTON, "toggle"),
        Some(true)
    );
}

#[test]
fn slash_command_with_a_typed_option() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    assert_eq!(h.slash_commands()[0].name, "silenttyping");
    let reply = h.slash(
        "silenttyping",
        vec![("enabled", OptionValue::Boolean(true))],
    );
    let CommandReply::Local(spans) = reply else {
        panic!("{reply:?}")
    };
    assert!(spans[0].text.starts_with("Silent typing on"));
    assert_eq!(tweak(&h), "true");
}

#[test]
fn the_users_own_choice_wins_and_unloading_resets() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.slash(
        "silenttyping",
        vec![("enabled", OptionValue::Boolean(true))],
    );
    h.stop().unwrap();
    assert_eq!(tweak(&h), "false", "the plugin's preset is gone with it");
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.backend()
        .set_tweak_by_user("silent-typing", Some("false"));
    let CommandReply::Local(spans) = h.slash(
        "silenttyping",
        vec![("enabled", OptionValue::Boolean(true))],
    ) else {
        panic!()
    };
    assert!(spans[0].text.contains("user-locked"), "{}", spans[0].text);
}
