// Run with `rekindle test`, which starts the simulated client first.
use rekindle_test::Preview;

#[test]
fn counts_channel_switches() {
    let p = Preview::reset();
    assert!(p.panel_text("status").contains(&"Channel switches this session: 0".to_string()));
    p.select(p.channel("general"));
    p.select(p.channel("off-topic"));
    let text = p.panel_text("status");
    assert!(text.contains(&"Channel switches this session: 2".to_string()), "{text:?}");
    assert!(text.contains(&"Current channel: #off-topic".to_string()), "{text:?}");
}
