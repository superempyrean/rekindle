// Run with `rekindle test` or `cargo test`. Either starts the simulated client.
use rekindle_test::Preview;

#[test]
fn shows_its_panel() {
    let p = Preview::reset();
    assert_eq!(p.panel_text("main"), ["Last response: (nothing yet)"]);
    // Clicking Fetch makes a real request to httpbin.org:
    // p.ui("main", "fetch", rekindle_test::json!({ "kind": "clicked" }));
}
