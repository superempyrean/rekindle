use rekindle_plugin_host::model::Color;
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/rust/focus-mode");
const ID: &str = "dev.rekindle.focus-mode";

fn hidden(h: &Harness, surface: &str) -> Option<bool> {
    h.backend().surface_style(surface).map(|(_, s)| s.hidden)
}

fn background(h: &Harness, surface: &str) -> Option<Color> {
    h.backend()
        .surface_style(surface)
        .and_then(|(_, s)| s.background)
}

#[test]
fn starts_on_hiding_the_member_list_and_restyling_the_user_area() {
    let h = Harness::for_crate(PLUGIN).unwrap();
    assert_eq!(h.commands(), vec!["toggle".to_string()]);
    assert_eq!(
        h.backend().surface_style("member-list").map(|(p, _)| p),
        Some(ID)
    );
    assert_eq!(hidden(&h, "member-list"), Some(true));
    assert_eq!(hidden(&h, "user-area"), Some(false));
    assert_eq!(
        background(&h, "user-area"),
        Some(Color {
            r: 0x2a,
            g: 0x1f,
            b: 0x3d,
            a: 0xff
        })
    );
}

#[test]
fn toggles_off_and_on_and_remembers_it() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.command("toggle");
    assert_eq!(hidden(&h, "member-list"), None);
    assert_eq!(background(&h, "user-area"), None);
    assert_eq!(h.toasts().pop().unwrap().text, "Focus mode off");
    h.restart().unwrap();
    assert_eq!(hidden(&h, "member-list"), None);
    h.command("toggle");
    assert_eq!(hidden(&h, "member-list"), Some(true));
    assert_eq!(h.toasts().pop().unwrap().text, "Focus mode on");
}

#[test]
fn unloading_brings_both_surfaces_back() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.stop().unwrap();
    assert_eq!(hidden(&h, "member-list"), None);
    assert_eq!(background(&h, "user-area"), None);
}
