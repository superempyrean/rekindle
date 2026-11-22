use rekindle_plugin_host::api::Transform;
use rekindle_plugin_host::backend::fixtures::{CAROL, GENERAL};
use rekindle_plugin_host::settings::SettingValue;
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/keyword-highlight"
);

fn ranges(t: &[Transform]) -> Vec<(u32, u32)> {
    t.iter()
        .flat_map(|t| match t {
            Transform::Highlight(hs) => hs.iter().map(|h| (h.start, h.end)).collect(),
            _ => vec![],
        })
        .collect()
}

#[test]
fn highlights_every_keyword_occurrence() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let m = h.receive_message(GENERAL, CAROL, "Rekindle and rekindle again");
    assert_eq!(
        ranges(&h.render_message(GENERAL, m.id)),
        vec![(0, 8), (13, 21)]
    );
    let plain = h.receive_message(GENERAL, CAROL, "nothing to see");
    assert!(h.render_message(GENERAL, plain.id).is_empty());
}

#[test]
fn settings_changes_rerender() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let m = h.receive_message(GENERAL, CAROL, "deploy friday");
    assert!(h.render_message(GENERAL, m.id).is_empty());
    h.set_setting("keywords", SettingValue::Items(vec!["friday".into()]));
    h.set_setting("tint-rows", SettingValue::Boolean(true));
    let t = h.render_message(GENERAL, m.id);
    assert_eq!(ranges(&t), vec![(7, 13)]);
    assert!(matches!(t.last(), Some(Transform::Background(_))));
}
