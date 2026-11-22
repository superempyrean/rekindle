use rekindle_plugin_host::model::{Color, Element};
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/theme-gallery"
);

const URL: &str = "https://www.thecolorapi.com/scheme?hex=5865f2&mode=analogic&count=5&format=json";
const BODY: &str = r##"{"mode":"analogic","colors":[
  {"hex":{"value":"#505DEE"}},{"hex":{"value":"#5082EE"}},{"hex":{"value":"#50A6EE"}},
  {"hex":{"value":"#6F50EE"}},{"hex":{"value":"#9350EE"}}]}"##;

fn status(h: &Harness) -> Vec<String> {
    h.panel("gallery")
        .unwrap()
        .elements
        .into_iter()
        .filter_map(|e| match e {
            Element::Text { text } => Some(text),
            _ => None,
        })
        .collect()
}

#[test]
fn generates_and_restores_palette() {
    let mut h = Harness::builder(PLUGIN)
        .http_get(URL, 200, BODY)
        .start()
        .unwrap();
    h.click("gallery", "generate");
    h.assert_active();
    assert_eq!(h.color("accent"), Some(Color::hex(0x505dee)));
    assert_eq!(h.requests().len(), 1);
    // No credentials or cookies were attached by the host.
    assert!(h.requests()[0].headers.is_empty());

    h.restart().unwrap();
    assert_eq!(h.color("accent"), Some(Color::hex(0x505dee)));
    assert_eq!(h.requests().len(), 1, "restore must not hit the network");
}

#[test]
fn http_errors_are_reported_not_fatal() {
    let mut h = Harness::builder(PLUGIN)
        .http_get(URL, 503, "")
        .start()
        .unwrap();
    h.click("gallery", "generate");
    h.assert_active();
    assert!(
        status(&h).iter().any(|s| s.contains("HTTP 503")),
        "{:?}",
        status(&h)
    );
}

/// Hits the real API: `cargo test -p rekindle-plugin-host --test examples theme_gallery -- --ignored`.
#[test]
#[ignore = "uses the network"]
fn live_api() {
    let mut h = Harness::builder(PLUGIN).real_network().start().unwrap();
    h.click("gallery", "generate");
    h.assert_active();
    assert!(
        status(&h).iter().any(|s| s.starts_with("Generated")),
        "{:?}\n{}",
        status(&h),
        h.log_text()
    );
    assert!(h.backend().override_owner("accent").is_some());
}
