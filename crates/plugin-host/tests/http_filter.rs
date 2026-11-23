//! `intercept-http` against a real component: the probe's `http-filter`
//! sees the mock's REST traffic for the kinds it registered, and drops or
//! rewrites it within the host's bounds.

use std::path::PathBuf;

use rekindle_plugin_host::model::LogLevel;
use rekindle_plugin_host::testing::Harness;

const GENERAL: u64 = 1002;

fn probe() -> Harness {
    Harness::for_crate(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/probe"))
        .unwrap()
}

#[test]
fn registering_needs_a_kind_with_known_routes() {
    let h = probe();
    // Message and typing have routes; presence doesn't.
    let line = h
        .logs()
        .iter()
        .find(|r| r.message.starts_with("activate intercept-http:"))
        .map(|r| r.message.clone())
        .unwrap_or_default();
    assert!(
        line.contains("Presence") && line.contains("Unavailable") && !line.contains("Typing"),
        "{}",
        h.log_text()
    );
}

#[test]
fn requests_pass_drop_or_go_with_another_body() {
    let mut h = probe();
    let path = format!("/channels/{GENERAL}/messages");
    let sent = |h: &Harness| {
        h.backend().world.messages[&GENERAL]
            .last()
            .map(|m| m.content.clone())
    };

    let (status, _) = h
        .rest("POST", &path, Some(r#"{"content":"plain"}"#))
        .unwrap();
    assert_eq!(status, 200);
    assert_eq!(sent(&h).as_deref(), Some("plain"));

    assert_eq!(
        h.rest("POST", &path, Some(r#"{"content":"drop me"}"#)),
        None
    );
    assert_eq!(
        sent(&h).as_deref(),
        Some("plain"),
        "a dropped request isn't sent"
    );

    h.rest("POST", &path, Some(r#"{"content":"rewrite me"}"#))
        .unwrap();
    assert_eq!(sent(&h).as_deref(), Some("rewritten"));

    // A replacement that isn't JSON is ignored, with a warning.
    h.rest("POST", &path, Some(r#"{"content":"break me"}"#))
        .unwrap();
    assert_eq!(sent(&h).as_deref(), Some("break me"));
    assert!(h.logged(LogLevel::Warn, "isn't JSON"), "{}", h.log_text());
}

#[test]
fn responses_reach_the_filter_and_can_be_rewritten() {
    let mut h = probe();
    let path = format!("/channels/{GENERAL}/messages?limit=5");
    h.rest(
        "POST",
        &format!("/channels/{GENERAL}/messages"),
        Some(r#"{"content":"a secret"}"#),
    );
    let (status, body) = h.rest("GET", &path, None).unwrap();
    assert_eq!(status, 200);
    assert_eq!(body.as_deref(), Some("[]"), "the filter hid the page");
    assert!(
        h.logged(LogLevel::Info, &format!("response GET {path} 200")),
        "{}",
        h.log_text()
    );
}

#[test]
fn routes_the_plugin_did_not_register_or_cannot_see_pass_untouched() {
    let mut h = probe();
    // A channel route: the probe registered messages and typing, not
    // channels, so its filter never hears of it.
    let (status, body) = h
        .rest("GET", &format!("/channels/{GENERAL}"), None)
        .unwrap();
    assert_eq!(status, 200);
    assert!(body.unwrap().contains("\"id\""));
    // A route with credentials in its body has no kind at all.
    let (status, _) = h
        .rest(
            "POST",
            "/auth/login",
            Some(r#"{"login":"a","password":"drop me"}"#),
        )
        .unwrap();
    assert_eq!(
        status, 404,
        "the mock doesn't know it, and no filter saw it"
    );
    assert!(!h.logged(LogLevel::Info, "response POST /auth/login"));
    assert!(!h.logged(
        LogLevel::Info,
        &format!("response GET /channels/{GENERAL} ")
    ));
}

#[test]
fn bodies_over_the_limit_pass_without_the_filter() {
    let mut h = probe();
    let big = format!(
        r#"{{"content":"drop me {}"}}"#,
        "x".repeat(rekindle_plugin_host::runtime::MAX_BODY)
    );
    let path = format!("/channels/{GENERAL}/messages");
    assert!(
        h.rest("POST", &path, Some(&big)).is_some(),
        "too big to filter, so not dropped"
    );
}
