use rekindle_plugin_host::model::{ThemeValue, ToastKind};
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/compact-mode"
);

fn density(h: &Harness) -> Option<ThemeValue> {
    h.value("density")
}

#[test]
fn registers_its_command_and_starts_at_the_default() {
    let h = Harness::for_crate(PLUGIN).unwrap();
    assert_eq!(h.commands(), vec!["toggle".to_string()]);
    assert_eq!(density(&h), Some(ThemeValue::Choice("default".into())));
}

#[test]
fn toggles_and_says_so() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.command("toggle");
    assert_eq!(density(&h), Some(ThemeValue::Choice("compact".into())));
    let t = h.toasts().pop().unwrap();
    assert_eq!(
        (t.kind, t.text.as_str()),
        (ToastKind::Info, "Compact mode on")
    );
    h.command("toggle");
    assert_eq!(density(&h), Some(ThemeValue::Choice("default".into())));
    assert_eq!(h.toasts().pop().unwrap().text, "Compact mode off");
}

#[test]
fn remembers_across_restart() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.command("toggle");
    assert_eq!(h.stored("compact").as_deref(), Some("true"));
    h.restart().unwrap();
    assert_eq!(density(&h), Some(ThemeValue::Choice("compact".into())));
}

#[test]
fn unloading_restores_the_clients_density() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.command("toggle");
    h.stop().unwrap();
    assert_eq!(density(&h), Some(ThemeValue::Choice("default".into())));
}
