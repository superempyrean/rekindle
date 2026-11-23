//! The 0.9.0 additions against a real component: value-returning actions,
//! `moderation`, `history`, `view` and the `patch` verdict.

use std::path::PathBuf;

use rekindle_plugin_host::ClientBackend as _;
use rekindle_plugin_host::model::{Action, LogLevel};
use rekindle_plugin_host::testing::Harness;

fn probe() -> Harness {
    Harness::for_crate(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe"))
        .unwrap()
}

fn said(h: &Harness, needle: &str) -> bool {
    h.logged(LogLevel::Info, needle)
}

/// A log line starting with `prefix` reports the action error `name`.
fn refused(h: &Harness, prefix: &str, name: &str) -> bool {
    h.logs()
        .iter()
        .any(|r| r.message.starts_with(prefix) && r.message.contains(&format!("name: \"{name}\"")))
}

#[test]
fn moderation_needs_a_gesture_and_valid_arguments() {
    let mut h = probe();
    assert!(
        refused(&h, "activate timeout:", "no-gesture"),
        "{}",
        h.log_text()
    );

    h.command("timeout");
    assert!(said(&h, "timeout: Ok(())"), "{}", h.log_text());
    assert!(
        h.performed()
            .iter()
            .any(|a| matches!(a, Action::Timeout { reason: Some(r), .. } if r == "spam"))
    );
    h.command("timeout-far");
    assert!(refused(&h, "timeout-far:", "invalid"), "{}", h.log_text());

    // Two messages go; one is too few, and the gesture's one outward action
    // is spent anyway.
    h.command("bulk");
    assert!(said(&h, "bulk: Ok(())"), "{}", h.log_text());
    assert!(refused(&h, "bulk one:", "invalid"), "{}", h.log_text());
}

#[test]
fn value_actions_return_what_the_client_made() {
    let mut h = probe();
    h.command("open-dm");
    assert!(said(&h, "open-dm: Ok(9001)"), "{}", h.log_text());
    h.command("invite");
    assert!(said(&h, "invite: Ok(\"mock3ea\")"), "{}", h.log_text());
}

#[test]
fn history_pages_with_a_cursor() {
    let mut h = probe();
    h.command("history");
    let line = h
        .logs()
        .into_iter()
        .find(|r| r.message.starts_with("history: "))
        .expect("history logged")
        .message;
    assert!(line.starts_with("history: 2 then Ok(("), "{line}");
}

#[test]
fn view_overrides_the_current_user_until_reset() {
    let mut h = probe();
    let before = h.backend().me().unwrap().global_name;
    h.command("self-name");
    assert!(said(&h, "self-name: Ok(())"), "{}", h.log_text());
    assert!(said(&h, "me: Some(\"Override\")"), "{}", h.log_text());
    assert_eq!(
        h.backend().me().unwrap().global_name.as_deref(),
        Some("Override")
    );
    h.command("self-setting");
    assert!(said(&h, "setting: Some(\"true\")"), "{}", h.log_text());
    h.command("self-setting-bad");
    assert!(
        refused(&h, "self-setting-bad:", "invalid"),
        "{}",
        h.log_text()
    );

    h.command("view-reset");
    assert_eq!(h.backend().me().unwrap().global_name, before);
    assert_eq!(
        h.backend()
            .my_setting("appearance.message-display-compact")
            .as_deref(),
        Some("false")
    );
}

#[test]
fn view_overrides_other_users_and_members_and_goes_on_unload() {
    use rekindle_plugin_host::api::Nameplate;

    let mut h = probe();
    h.with_world(|w| {
        w.users.get_mut(&101).unwrap().nameplate = Some(Nameplate {
            asset: "nameplates/x".into(),
            palette: "berry".into(),
            label: "x".into(),
            sku: 1,
            expires_ms: None,
        });
    });
    h.command("strip-others");
    assert!(
        said(&h, "alice cosmetics: Some((false, false))"),
        "{}",
        h.log_text()
    );
    // The current user isn't one of the others.
    let me = h.backend().me().unwrap().id;
    assert_ne!(me, 101);

    h.command("rename-member");
    assert!(
        said(&h, "alice nick: Some(\"Renamed\")"),
        "{}",
        h.log_text()
    );

    h.stop().unwrap();
    assert!(h.backend().user(101).unwrap().nameplate.is_some());
    assert_ne!(
        h.backend()
            .member(1000, 101)
            .and_then(|m| m.nick)
            .as_deref(),
        Some("Renamed")
    );
}

#[test]
fn account_fields_need_read_account() {
    let mut h = probe();
    let me = h.backend().me().unwrap().id;
    h.with_world(|w| w.users.get_mut(&me).unwrap().email = Some("me@example.com".into()));
    h.command("me-email");
    assert!(said(&h, "me email: None"), "{}", h.log_text());
}

#[test]
fn patch_changes_fields_of_the_same_event() {
    let mut h = probe();
    let m = h.receive_message(1002, 101, "please strip this");
    assert_eq!(m.content, "cleaned");
    assert!(m.embeds.is_empty());
    // Empty text fails the compose checks: the original text is kept.
    let m = h.receive_message(1002, 101, "a bad one");
    assert_eq!(m.content, "a bad one");
    assert!(h.logged(LogLevel::Warn, "patched"), "{}", h.log_text());
}
