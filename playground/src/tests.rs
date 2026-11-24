use rekindle_wire::capture;
use rekindle_wire::{ElementKind, NodeOp, Turn, Value, ids};

use crate::Session;

/// A capture of `turns`.
fn capture_of(turns: &[Turn]) -> Vec<u8> {
    let mut out = capture::header().to_vec();
    for (i, t) in turns.iter().enumerate() {
        capture::encode_record(i as u64, 0, &t.to_bytes(), &mut out);
    }
    out
}

/// A root with a text under it.
fn screen() -> Turn {
    let mut t = Turn::new();
    t.push(NodeOp::Create {
        node: 1,
        kind: ElementKind::new("view"),
        props: vec![(ids::BACKGROUND, Value::color(0xff1a_1a1e))],
    });
    t.push(NodeOp::Create {
        node: 2,
        kind: ElementKind::new("text"),
        props: vec![
            (
                rekindle_wire::PropId::from_name("text").unwrap(),
                Value::Str("hello".into()),
            ),
            (ids::COLOR, Value::color(0xff1a_1a1e)),
        ],
    });
    t.push(NodeOp::Children {
        node: 1,
        children: vec![2],
    });
    t.push(NodeOp::Root { node: 1 });
    t
}

fn cell(json: &str, node: &str, prop: &str) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_str(json).unwrap();
    v["nodes"][node]["p"][prop].clone()
}

/// A colour cell as `0xAARRGGBB`, whether it was written as an `i32` or an
/// `i64`.
fn color(json: &str, node: &str, prop: &str) -> u32 {
    cell(json, node, prop).as_i64().unwrap() as u32
}

#[test]
fn the_host_tree_takes_the_work_and_the_guest_tree_keeps_its_colours() {
    let mut s = Session::new(&capture_of(&[screen()])).unwrap();
    assert_eq!(s.turns(), 1);
    s.set_work_json(r#"{"recolors": [{"from": "1a1a1e", "to": "102030ff"}]}"#)
        .unwrap();
    let host = s.tree_json(true);
    let guest = s.tree_json(false);
    assert_eq!(color(&host, "1", "background"), 0xff10_2030);
    assert_eq!(color(&host, "2", "color"), 0xff10_2030);
    assert_eq!(color(&guest, "1", "background"), 0xff1a_1a1e);
    assert_eq!(cell(&host, "2", "text"), serde_json::json!("hello"));
}

#[test]
fn a_replay_keeps_the_work() {
    let mut s = Session::new(&capture_of(&[screen(), Turn::new()])).unwrap();
    s.set_work_json(r#"{"recolors": [{"from": "1a1a1e", "to": "102030ff"}]}"#)
        .unwrap();
    s.replay(1);
    assert_eq!(s.upto(), 1);
    assert_eq!(color(&s.tree_json(true), "2", "color"), 0xff10_2030);
}

#[test]
fn a_sanitized_snapshot_replays_to_the_same_layout() {
    let s = Session::new(&capture_of(&[screen()])).unwrap();
    let turn = crate::sanitize::Sanitizer::new([]).snapshot(s.engine().guest());
    let again = Session::new(&capture_of(&[turn])).unwrap();
    let (a, b) = (s.tree_json(false), again.tree_json(false));
    assert_eq!(cell(&a, "1", "background"), cell(&b, "1", "background"));
    assert_ne!(cell(&b, "2", "text"), serde_json::json!("hello"));
}

/// The fixture and the Midnight theme file, as the page sends them.
fn midnight_request(screen: &str) -> (Vec<u8>, String) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/");
    let index: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{dir}index.json")).unwrap())
            .unwrap();
    let s = index["screens"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == screen)
        .unwrap();
    let capture = std::fs::read(format!("{dir}{}", s["capture"].as_str().unwrap())).unwrap();
    let binding: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{dir}binding.json")).unwrap())
            .unwrap();
    let theme: serde_json::Value =
        serde_json::from_str(include_str!("../../plugins/themes/midnight.theme.json")).unwrap();
    let mut tokens = theme["tokens"].as_object().unwrap().clone();
    tokens.extend(theme["variants"][0]["tokens"].as_object().unwrap().clone());
    let request = serde_json::json!({
        "binding": binding,
        "palette": index["palettes"][s["palette"].as_str().unwrap()],
        "tokens": tokens,
    });
    (capture, request.to_string())
}

/// What a host does: the theme's recolours are the engine's work before
/// the guest's turns arrive, and each turn goes through `filter`.
#[test]
fn a_theme_set_after_the_turns_paints_what_the_host_paints() {
    for screen in ["welcome-dark", "welcome-light", "chat"] {
        let (capture, request) = midnight_request(screen);
        let mut s = Session::new(&capture).unwrap();
        s.set_theme_json(&request).unwrap();

        let theme = crate::theme::Theme::from_json(&request).unwrap();
        assert!(!theme.recolors().is_empty(), "{screen}: no recolours");
        let mut host = rekindle_wire_inject::Engine::new(theme.work()).unwrap();
        let (records, _) = capture::parse(&capture).unwrap();
        for r in &records {
            host.filter(&r.bytes).unwrap();
        }
        assert_eq!(
            s.tree_json(true),
            crate::tree_json(host.mirror()),
            "{screen}: the playground's tree differs from the host's"
        );
        assert_ne!(
            s.tree_json(true),
            s.tree_json(false),
            "{screen}: nothing recoloured"
        );
    }
}
