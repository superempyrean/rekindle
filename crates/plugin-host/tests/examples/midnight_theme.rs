use rekindle_plugin_host::model::Color;
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/midnight-theme"
);

#[test]
fn applies_default_variant_and_registers_command() {
    let h = Harness::for_crate(PLUGIN).unwrap();
    h.assert_active();
    assert_eq!(h.color("background-primary"), Some(Color::hex(0x0b0d12)));
    assert_eq!(h.commands(), vec!["next-variant".to_string()]);
}

#[test]
fn sets_dark_foregrounds_so_it_works_on_a_light_base() {
    let h = Harness::for_crate(PLUGIN).unwrap();
    assert_eq!(h.color("text-strong"), Some(Color::hex(0xf2f3f5)));
    assert_eq!(h.color("background-mod-normal"), Some(Color::hex(0x232733)));
}

#[test]
fn cycles_and_remembers_variant() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.command("next-variant");
    assert_eq!(h.color("background-primary"), Some(Color::hex(0x000000)));
    assert_eq!(h.stored("variant").as_deref(), Some("abyss"));

    h.restart().unwrap();
    assert_eq!(h.color("background-primary"), Some(Color::hex(0x000000)));
}

#[test]
fn overrides_vanish_on_unload() {
    use rekindle_plugin_host::ClientBackend;
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let base = rekindle_plugin_host::MockBackend::new();
    h.stop().unwrap();
    assert_eq!(
        h.color("background-primary"),
        base.get_color("background-primary")
    );
}
