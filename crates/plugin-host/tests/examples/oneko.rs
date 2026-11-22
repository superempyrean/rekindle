use std::thread::sleep;
use std::time::Duration;

use rekindle_plugin_host::api::Event;
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../plugins/rust/oneko");

const ID: &str = "dev.rekindle.oneko";

fn harness() -> Harness {
    Harness::for_crate(PLUGIN).unwrap()
}

/// Let the repeating "step" timer come due, then fire it. Timers run on the
/// host's real clock, so wait past the 100 ms tick before firing.
fn tick(h: &mut Harness) {
    sleep(Duration::from_millis(110));
    h.runtime().fire_due_timers();
}

fn cat_center(h: &Harness) -> (f32, f32) {
    let b = h.backend();
    let s = b.overlay_sprite(ID, "cat").unwrap();
    (s.x + 16.0, s.y + 16.0)
}

#[test]
fn registers_its_atlas_and_draws_itself() {
    let mut h = harness();
    h.assert_active();
    tick(&mut h);
    let b = h.backend();
    assert_eq!(
        b.overlay_images().len(),
        1,
        "the cat registered its own atlas"
    );
    assert_eq!(
        b.overlay_images()[0].width,
        256,
        "oneko's 256x128 sheet layout"
    );
    assert_eq!(b.overlay_images()[0].height, 128);
    let s = b.overlay_sprite(ID, "cat").expect("the cat drew itself");
    assert_eq!((s.sw, s.sh), (32, 32), "32px frames");
}

#[test]
fn chases_the_cursor_running_east() {
    let mut h = harness();
    tick(&mut h);
    let (cx, cy) = cat_center(&h);
    let start_x = cx;

    // Cursor straight to the right at the same y forces a pure East run.
    for _ in 0..4 {
        h.event(Event::PointerMoved((cx + 600.0, cy)));
        tick(&mut h);
    }
    let s = h.backend().overlay_sprite(ID, "cat").unwrap().clone();
    assert!(
        s.x > start_x + 20.0,
        "cat moved toward the cursor ({start_x} -> {})",
        s.x
    );
    // East frames are column 3 of the sheet -> source x = 96.
    assert_eq!(s.sx, 96, "running east uses oneko's E column");
}

#[test]
fn settles_when_it_catches_up() {
    let mut h = harness();
    tick(&mut h);
    let (cx, cy) = cat_center(&h);
    // Park the cursor on the cat and let it stop.
    for _ in 0..6 {
        h.event(Event::PointerMoved((cx, cy)));
        tick(&mut h);
    }
    let before = h.backend().overlay_sprite(ID, "cat").unwrap().x;
    for _ in 0..3 {
        h.event(Event::PointerMoved((cx, cy)));
        tick(&mut h);
    }
    let after = h.backend().overlay_sprite(ID, "cat").unwrap().x;
    assert!(
        (after - before).abs() < 0.5,
        "cat is idle (not moving) once caught up"
    );
}
