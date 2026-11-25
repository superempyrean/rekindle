//! `wire-raw` and `elements` against a real component: the probe sets its
//! own work and changes the client's elements, on the mock.

use std::path::PathBuf;

use rekindle_plugin_host::backend::MockBackend;
use rekindle_plugin_host::model::{Place, PressMode, RAW_SURFACE};
use rekindle_plugin_host::testing::Harness;
use rekindle_wire_inject::RowAction;

const ID: &str = "test.rekindle.probe";

fn probe(named: bool) -> Harness {
    let mut b = MockBackend::new();
    if named {
        b.set_elements(&["composer.gift", "search.entry"]);
    }
    Harness::builder(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe"))
        .backend(b)
        .start()
        .unwrap()
}

#[test]
fn raw_work_is_kept_and_refused_plainly() {
    let mut h = probe(true);
    h.command("raw");
    assert!(h.log_text().contains("raw: Ok(())"), "{}", h.log_text());
    {
        let b = h.backend();
        let w = &b.named_elements().raw[ID];
        assert_eq!(w.rows[0].action, RowAction::Drop);
    }
    h.command("raw-bad");
    assert!(h.log_text().contains("can't take the system back"), "{}", h.log_text());
    h.command("raw-clear");
    assert!(h.backend().named_elements().raw.is_empty());
    h.command("raw");
    h.stop().unwrap();
    assert!(h.backend().named_elements().raw.is_empty(), "dropped on unload");
    assert_eq!(RAW_SURFACE, "wire-raw");
}

#[test]
fn raw_work_is_unavailable_without_the_wire() {
    let mut h = probe(false);
    h.command("raw");
    assert!(h.log_text().contains("raw selector: None"), "{}", h.log_text());
    assert!(h.log_text().contains("raw: Err("), "{}", h.log_text());
}

#[test]
fn elements_calls_reach_the_backend() {
    let mut h = probe(true);
    h.command("elements");
    let log = h.log_text();
    assert!(log.contains(r#"names: ["composer.gift", "search.entry"]"#), "{log}");
    assert!(log.contains("press: Ok(())"), "{log}");
    assert!(log.contains("retarget self: Err(ElementError { code: 1, name: \"invalid\""), "{log}");
    assert!(log.contains("insert: Ok(())"), "{log}");
    assert!(log.contains("missing: Err(ElementError { code: 0, name: \"unavailable\""), "{log}");
    let b = h.backend();
    let mine = &b.named_elements().plugins[ID];
    assert_eq!(mine.presses["composer.gift"], PressMode::Consume);
    assert_eq!(mine.inserts[0].place, Place::After);
}
