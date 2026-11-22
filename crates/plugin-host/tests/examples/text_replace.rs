use rekindle_plugin_host::model::UiValue;
use rekindle_plugin_host::testing::Harness;

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/text-replace"
);

const ID: &str = "dev.rekindle.text-replace";

fn text(h: &mut Harness, element: &str, value: &str) {
    h.ui("settings", element, UiValue::Text(value.into()));
}

/// Add a rule through the panel, as a user would. `i` is its index.
fn add_rule(h: &mut Harness, i: usize, find: &str, replace: &str, regex: bool) {
    h.click("settings", "add");
    text(h, &format!("find-{i}"), find);
    text(h, &format!("replace-{i}"), replace);
    if regex {
        h.toggle("settings", &format!("regex-{i}"), true);
    }
}

#[test]
fn leaves_messages_alone_with_no_rules() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let out = h.send_message(1, "hello there");
    assert_eq!(out.text, "hello there");
    assert!(out.rewritten_by.is_empty());
    assert!(h.panel("settings").is_some());
}

#[test]
fn plain_and_regex_rules_apply_in_order() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    add_rule(&mut h, 0, "teh", "the", false);
    add_rule(&mut h, 1, r"(\w+)@example\.com", "<$1 at example>", true);
    let out = h.send_message(1, "teh mail is maple@example.com");
    assert_eq!(out.text, "the mail is <maple at example>");
    assert_eq!(out.rewritten_by, vec![ID.to_string()]);
    // Unchanged text isn't reported as a rewrite.
    assert!(h.send_message(1, "nothing here").rewritten_by.is_empty());
}

#[test]
fn rules_persist_across_restart_and_can_be_deleted() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    add_rule(&mut h, 0, "brb", "be right back", false);
    h.restart().unwrap();
    assert_eq!(h.send_message(1, "brb").text, "be right back");
    h.click("settings", "delete-0");
    assert_eq!(h.send_message(1, "brb").text, "brb");
}

#[test]
fn invalid_regex_is_flagged_and_skipped() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    add_rule(&mut h, 0, "(unclosed", "x", true);
    let panel = h.panel("settings").unwrap();
    assert!(format!("{:?}", panel.elements).contains("Invalid regex"));
    assert_eq!(h.send_message(1, "(unclosed").text, "(unclosed");
}

#[test]
fn host_refuses_a_rewrite_to_nothing() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    add_rule(&mut h, 0, ".*", "", true);
    let out = h.send_message(1, "keep me");
    assert_eq!(out.text, "keep me");
    assert_eq!(out.rejected.len(), 1);
    assert!(out.rewritten_by.is_empty());
}

/// The replacement syntax every port must read the same way: `${1}`, `$2`,
/// `$$` for a `$`, and a missing group as nothing.
#[test]
fn capture_references() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    add_rule(&mut h, 0, r"(\w+)@(\w+)", "${1} at $2 ($$5, [$9])", true);
    assert_eq!(h.send_message(1, "ann@home").text, "ann at home ($5, [])");
}
