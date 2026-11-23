// Run with `rekindle test` or `cargo test`. Either starts the simulated client.
use rekindle_test::Preview;

#[test]
fn recolors_the_client() {
    let p = Preview::reset();
    p.assert_active();
    assert_eq!(p.theme("background-brand").as_deref(), Some("#cba6f7"));
    assert_eq!(p.theme("text-default").as_deref(), Some("#cdd6f4"));
}
