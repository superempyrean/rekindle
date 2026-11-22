use rekindle_plugin_host::api::ComposeKind;
use rekindle_plugin_host::backend::fixtures::GENERAL;
use rekindle_plugin_host::settings::SettingValue;
use rekindle_plugin_host::testing::Harness;

use crate::toolchain::needs;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/ts/split-long-messages"
);

/// `n` paragraphs of about 700 characters each.
fn paragraphs(n: usize) -> String {
    (0..n)
        .map(|i| format!("paragraph {i} ") + &"word ".repeat(140))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The words of `s`, to compare text across the cuts.
fn words(s: &str) -> Vec<&str> {
    s.split_whitespace().collect()
}

#[test]
fn a_short_message_goes_as_one() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    let out = h.send_message(GENERAL, "hello");
    assert!(
        out.parts.is_empty() && out.rewritten_by.is_empty(),
        "{out:?}"
    );
}

#[test]
fn a_long_message_is_split_at_paragraphs() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    let text = paragraphs(7);
    let out = h.send_message(GENERAL, &text);
    assert!(
        out.parts.len() >= 3,
        "{} parts\n{}",
        out.parts.len(),
        h.log_text()
    );
    assert!(
        out.parts.iter().all(|p| p.chars().count() <= 2000),
        "{:?}",
        out.parts
            .iter()
            .map(std::string::String::len)
            .collect::<Vec<_>>()
    );
    // Cut at paragraph breaks: every part starts a paragraph, and nothing is lost.
    assert!(
        out.parts.iter().all(|p| p.starts_with("paragraph ")),
        "{:?}",
        out.parts
    );
    assert_eq!(
        out.parts.iter().flat_map(|p| words(p)).collect::<Vec<_>>(),
        words(&text)
    );
    assert_eq!(
        out.rewritten_by,
        vec![h.id().to_string()],
        "the user is told"
    );
}

#[test]
fn the_nitro_limit_makes_fewer_parts() {
    let mut h = Harness::builder(needs!(PLUGIN))
        .setting("limit", SettingValue::Selected(1))
        .start()
        .unwrap();
    let out = h.send_message(GENERAL, &paragraphs(7));
    assert_eq!(
        out.parts.len(),
        2,
        "{:?}",
        out.parts
            .iter()
            .map(std::string::String::len)
            .collect::<Vec<_>>()
    );
}

#[test]
fn text_without_breaks_is_cut_at_the_limit() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    let out = h.send_message(GENERAL, &"x".repeat(4500));
    assert_eq!(
        out.parts
            .iter()
            .map(std::string::String::len)
            .collect::<Vec<_>>(),
        [2000, 2000, 500]
    );
}

#[test]
fn too_long_for_ten_parts_is_not_sent() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    let out = h.send_message(GENERAL, &"x".repeat(20_500));
    assert!(out.parts.is_empty());
    assert!(
        out.cancelled
            .as_ref()
            .is_some_and(|(_, why)| why.contains("too long")),
        "{out:?}"
    );
}

#[test]
fn an_edit_is_not_split() {
    let mut h = Harness::for_crate(needs!(PLUGIN)).unwrap();
    let alices = h.with_world(|w| w.messages[&GENERAL][0].id);
    let mut d = h.reply_draft(GENERAL, alices, &paragraphs(7), false);
    d.reply = None;
    d.kind = ComposeKind::Edit;
    let out = h.compose(&d);
    assert!(out.parts.is_empty(), "{out:?}");
}
