// Run with `rekindle test` or `cargo test`. Either starts the simulated client. Each
// test starts with `Preview::reset()`: a fresh client and empty storage.
use rekindle_test::Preview;

#[test]
fn activates() {
    let p = Preview::reset();
    p.assert_active();
    assert!(p.logs().iter().any(|l| l.contains("activated")), "{:?}", p.logs());
}
