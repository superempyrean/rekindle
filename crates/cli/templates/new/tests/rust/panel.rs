// Run with `rekindle test`, which starts the simulated client first.
use rekindle_test::{Preview, json};

#[test]
fn the_accent_starts_on() {
    let p = Preview::reset();
    assert_eq!(p.theme("background-brand").as_deref(), Some("#eb459e"));
}

#[test]
fn picking_a_color_recolors_and_is_saved() {
    let p = Preview::reset();
    p.ui("settings", "accent", json!({ "kind": "color", "value": "#0080ff" }));
    assert_eq!(p.theme("background-brand").as_deref(), Some("#0080ff"));
    assert_eq!(p.storage()["accent"], "#0080ff");
}

#[test]
fn turning_it_off_restores_the_theme() {
    let p = Preview::reset();
    p.ui("settings", "enabled", json!({ "kind": "toggled", "value": false }));
    assert_ne!(p.theme("background-brand").as_deref(), Some("#eb459e"));
    // It stays off after a restart.
    p.reload();
    assert_ne!(p.theme("background-brand").as_deref(), Some("#eb459e"));
}
