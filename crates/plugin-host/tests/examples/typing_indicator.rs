use rekindle_plugin_host::backend::fixtures::{ALICE, CAROL, GENERAL, ME, OFF_TOPIC};
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/typing-indicator"
);

const KIND: &str = "channel-list.trailing";

#[test]
fn badges_channels_where_others_type() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.typing(OFF_TOPIC, vec![ALICE, CAROL]);
    let d = h.decoration(KIND, OFF_TOPIC).expect("badge");
    assert_eq!(d.text.as_deref(), Some("2"));
    assert_eq!(d.tooltip.as_deref(), Some("Alice and Carol are typing…"));
    // Only you typing isn't news.
    h.typing(OFF_TOPIC, vec![ME]);
    assert!(h.decoration(KIND, OFF_TOPIC).is_none());
}

#[test]
fn the_open_channel_is_never_badged() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    h.typing(GENERAL, vec![ALICE]);
    assert!(
        h.decoration(KIND, GENERAL).is_none(),
        "the sample client has #general open"
    );
    h.typing(OFF_TOPIC, vec![ALICE]);
    h.select_channel(OFF_TOPIC);
    assert!(h.decoration(KIND, OFF_TOPIC).is_none(), "cleared on open");
}
