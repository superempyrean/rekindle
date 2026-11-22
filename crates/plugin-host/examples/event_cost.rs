//! How long a plugin takes to handle events, for comparing builds.
//!
//! `cargo run --release -p rekindle-plugin-host --example event_cost -- <plugin.wasm>...`
//!
//! For each plugin: 20,000 `message-created` events through the event
//! filters and handlers, one per call, and the same in batches of 50.

use std::time::Instant;

use rekindle_plugin_host::backend::sample_message;
use rekindle_plugin_host::testing::Harness;

fn main() {
    for path in std::env::args().skip(1) {
        let mut h = Harness::from_wasm(std::fs::read(&path).unwrap())
            .start()
            .expect("start");
        let msg = |i: u64| {
            rekindle_plugin_host::api::Event::MessageCreated(sample_message(
                10,
                None,
                1_000_000 + i,
                (7, "someone"),
                "hello there, a normal sized message with a link https://example.com",
            ))
        };
        for i in 0..200 {
            h.event(msg(i));
        }
        let n = 20_000u64;
        let t = Instant::now();
        for i in 0..n {
            h.event(msg(i));
        }
        let one = t.elapsed().as_secs_f64() * 1e6 / n as f64;
        let t = Instant::now();
        for b in 0..n / 50 {
            h.events((0..50).map(|i| msg(b * 50 + i)).collect());
        }
        let batched = t.elapsed().as_secs_f64() * 1e6 / n as f64;
        println!(
            "{:<60} {one:>6.2} µs per event one at a time, {batched:>6.2} µs in batches of 50",
            path.rsplit(['/', '\\']).next().unwrap()
        );
    }
}
