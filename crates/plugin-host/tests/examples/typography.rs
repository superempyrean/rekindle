use rekindle_plugin_host::model::{ThemeValue, UiValue};
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/rust/typography");

fn harness() -> Harness {
    Harness::for_crate(PLUGIN).unwrap()
}

#[test]
fn registers_bundled_fonts_and_applies_them() {
    let h = harness();
    h.assert_active();
    let fonts: Vec<String> = h
        .backend()
        .fonts()
        .iter()
        .map(|f| f.family.clone())
        .collect();
    assert_eq!(
        fonts,
        vec!["IBM Plex Sans".to_string(), "Lilex".to_string()]
    );
    assert_eq!(
        h.value("font-primary"),
        Some(ThemeValue::Font("IBM Plex Sans".into()))
    );
    assert_eq!(h.value("font-code"), Some(ThemeValue::Font("Lilex".into())));
}

#[test]
fn sliders_drive_avatar_radius_and_density() {
    let mut h = harness();
    h.ui("typography", "avatar", UiValue::Number(64.0));
    h.ui("typography", "radius", UiValue::Number(0.0));
    h.ui("typography", "density", UiValue::Selected(0));
    assert_eq!(h.value("chat-avatar-size"), Some(ThemeValue::Length(64.0)));
    assert_eq!(h.value("radius-xl"), Some(ThemeValue::Length(0.0)));
    assert_eq!(
        h.value("density"),
        Some(ThemeValue::Choice("compact".into()))
    );
    h.restart().unwrap();
    assert_eq!(
        h.value("chat-avatar-size"),
        Some(ThemeValue::Length(64.0)),
        "persisted"
    );
}

#[test]
fn reset_returns_to_client_defaults() {
    use rekindle_plugin_host::ClientBackend;
    let mut h = harness();
    let defaults = rekindle_plugin_host::MockBackend::new();
    h.ui("typography", "radius", UiValue::Number(0.0));
    h.click("typography", "reset");
    for t in ["font-primary", "radius-md", "density", "chat-avatar-size"] {
        assert_eq!(h.value(t), defaults.get_value(t), "{t}");
    }
}
