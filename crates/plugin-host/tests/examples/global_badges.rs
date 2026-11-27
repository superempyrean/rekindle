use std::thread::sleep;
use std::time::Duration;

use rekindle_plugin_host::runtime::RestRequest;
use rekindle_plugin_host::testing::Harness;
use serde_json::{Value, json};

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/global-badges"
);
const API: &str = "https://badges.equicord.org/users";

fn map() -> String {
    json!({"status": 200, "users": {
        "42": [
            {"tooltip": "Vencord Contributor", "mod": "vencord", "badge": "https://badges.equicord.org/public/badges/vencord.png", "rarity": 5},
            {"tooltip": "mine", "mod": "badgevault", "badge": "https://example.invalid/b.png", "rarity": 1}
        ]
    }})
    .to_string()
}

fn profile(user: &str) -> RestRequest {
    RestRequest {
        id: 1,
        method: "GET".into(),
        path: format!("/users/{user}/profile?with_mutual_guilds=true"),
        body: None,
    }
}

/// The plugin with its first download done.
fn downloaded() -> Harness {
    let mut h = Harness::builder(PLUGIN)
        .http_get(API, 200, map())
        .start()
        .unwrap();
    sleep(Duration::from_millis(60));
    h.runtime().fire_due_timers();
    h
}

#[test]
fn a_profile_shows_the_users_mod_badges_after_discords() {
    let mut h = downloaded();
    let body = json!({"user": {"id": "42"}, "badges": [{"id": "staff", "description": "Discord Staff", "icon": "x"}]});
    let read = h
        .runtime()
        .filter_response(&profile("42"), 200, Some(body.to_string()))
        .unwrap();
    let read: Value = serde_json::from_str(&read).unwrap();
    let names: Vec<&str> = read["badges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["description"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Discord Staff", "Vencord Contributor"]);
    assert_eq!(h.requests().len(), 1);
    h.assert_active();
}

#[test]
fn a_user_without_mod_badges_passes() {
    let mut h = downloaded();
    let body = json!({"user": {"id": "7"}, "badges": []}).to_string();
    let read = h
        .runtime()
        .filter_response(&profile("7"), 200, Some(body.clone()));
    assert_eq!(read, Some(body));
}

#[test]
fn before_the_download_profiles_pass() {
    let mut h = Harness::builder(PLUGIN)
        .http_get(API, 200, map())
        .start()
        .unwrap();
    let body = json!({"user": {"id": "42"}, "badges": []}).to_string();
    let read = h
        .runtime()
        .filter_response(&profile("42"), 200, Some(body.clone()));
    assert_eq!(read, Some(body));
}
