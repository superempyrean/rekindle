use rekindle_plugin_host::model::{Color, Element, UiValue};
use rekindle_plugin_host::testing::Harness;
use rekindle_plugin_host::{ClientBackend, MockBackend};

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/accent-panel"
);

fn harness() -> Harness {
    Harness::for_crate(PLUGIN).unwrap()
}

#[test]
fn shows_panel_and_applies_default_accent() {
    let h = harness();
    h.assert_active();
    let panel = h.panel("settings").expect("panel shown");
    assert!(matches!(panel.elements[0], Element::ColorPicker { ref id, .. } if id == "accent"));
    assert_eq!(
        h.color("accent"),
        Some(Color::hex(0xeb459e)),
        "legacy alias"
    );
    assert_eq!(h.color("background-brand"), Some(Color::hex(0xeb459e)));
}

#[test]
fn picking_a_color_applies_and_persists() {
    let mut h = harness();
    h.pick_color("settings", "accent", Color::hex(0x00ff88));
    assert_eq!(h.color("accent"), Some(Color::hex(0x00ff88)));
    assert_eq!(h.stored("accent").as_deref(), Some("#00ff88"));
    h.restart().unwrap();
    assert_eq!(h.color("accent"), Some(Color::hex(0x00ff88)));
}

#[test]
fn tint_reveals_slider_and_mixes_backgrounds() {
    let mut h = harness();
    let base = MockBackend::new().get_color("background-primary").unwrap();
    assert!(
        h.panel("settings")
            .unwrap()
            .elements
            .iter()
            .all(|e| !matches!(e, Element::Slider { .. }))
    );

    h.toggle("settings", "tint", true);
    assert!(
        h.panel("settings")
            .unwrap()
            .elements
            .iter()
            .any(|e| matches!(e, Element::Slider { .. }))
    );
    let tinted = h.color("background-primary").unwrap();
    assert_ne!(tinted, base);

    h.ui("settings", "strength", UiValue::Number(0.0));
    assert_eq!(h.color("background-primary"), Some(base));

    h.toggle("settings", "tint", false);
    assert_eq!(h.backend().tint(), None);
    assert_eq!(h.color("background-base-low"), Some(base));
}

#[test]
fn reset_restores_defaults() {
    let mut h = harness();
    h.pick_color("settings", "accent", Color::hex(0x123456));
    h.toggle("settings", "tint", true);
    h.click("settings", "reset");
    assert_eq!(h.color("accent"), Some(Color::hex(0xeb459e)));
    assert_eq!(h.stored("tint").as_deref(), Some("false"));
}
