use rekindle_plugin_host::api::{Event, MyVoice};
use rekindle_plugin_host::backend::fixtures::{LAB, LAB_VOICE};
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/rust/call-timer");

#[test]
fn shows_a_live_timer_while_in_voice() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    assert_eq!(h.voice_status("timer"), None, "not in a call");
    let v = MyVoice {
        channel: LAB_VOICE,
        guild: Some(LAB),
        since_ms: 1_767_225_600_000,
        self_mute: false,
        self_deaf: false,
        self_video: false,
        streaming: false,
        connection: "connected".into(),
        ping_ms: Some(30),
    };
    h.with_world(|w| w.my_voice = Some(v.clone()));
    h.event(Event::MyVoice(v));
    assert_eq!(
        h.voice_status("timer"),
        Some(("Lounge · In call".into(), Some(1_767_225_600_000)))
    );
    h.with_world(|w| w.my_voice = None);
    h.event(Event::MyVoiceLeft);
    assert_eq!(h.voice_status("timer"), None);
}
