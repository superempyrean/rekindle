use rekindle_plugin_host::backend::MockBackend;
use rekindle_plugin_host::settings::SettingValue;
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/rust/declutter");
const ID: &str = "dev.rekindle.declutter";

fn backend() -> MockBackend {
    let mut b = MockBackend::new();
    b.set_elements(&["composer.gift", "search.entry"]);
    b
}

fn hidden(h: &Harness) -> Vec<String> {
    h.backend()
        .named_elements()
        .plugins
        .get(ID)
        .map(|p| p.hidden.iter().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn hides_the_gift_button_by_default() {
    let mut h = Harness::builder(PLUGIN).backend(backend()).start().unwrap();
    assert_eq!(hidden(&h), ["composer.gift"]);
    assert!(
        h.runtime()
            .features()
            .contains(&"element:search.entry".to_string())
    );
}

#[test]
fn a_changed_list_brings_back_what_left_it_and_skips_unknown_names() {
    let mut h = Harness::builder(PLUGIN).backend(backend()).start().unwrap();
    h.set_setting(
        "hide",
        SettingValue::Items(vec!["search.entry".into(), "nope".into()]),
    );
    assert_eq!(hidden(&h), ["search.entry"]);
    assert!(
        h.log_text().contains("couldn't hide `nope`"),
        "{}",
        h.log_text()
    );
}

#[test]
fn a_build_without_names_hides_nothing() {
    let h = Harness::for_crate(PLUGIN).unwrap();
    assert!(hidden(&h).is_empty());
    h.assert_active();
}

#[test]
fn unloading_brings_everything_back() {
    let mut h = Harness::builder(PLUGIN).backend(backend()).start().unwrap();
    h.stop().unwrap();
    assert!(h.backend().named_elements().plugins.is_empty());
}

#[test]
fn its_manifest_declares_hiding_any_element() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let m = h.runtime().manifest(ID).unwrap().clone();
    assert_eq!(
        rekindle_plugin_host::review::element_lines(&m.elements),
        ["It can hide any of the client's elements."]
    );
}
