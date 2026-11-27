use rekindle_plugin_host::runtime::RestRequest;
use rekindle_plugin_host::testing::Harness;
use serde_json::{Value, json};

const PLUGIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../plugins/rust/no-profile-themes"
);

fn get(path: &str) -> RestRequest {
    RestRequest {
        id: 1,
        method: "GET".into(),
        path: path.into(),
        body: None,
    }
}

#[test]
fn a_profile_reaches_the_client_without_its_theme() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let profile = json!({
        "user": {"id": "42", "username": "a", "avatar_decoration_data": {"asset": "x"}},
        "user_profile": {"bio": "hi", "theme_colors": [1, 2], "profile_effect": {"id": "3"}},
        "badges": []
    });
    let read = h.runtime().filter_response(
        &get("/users/42/profile?with_mutual_guilds=true"),
        200,
        Some(profile.to_string()),
    );
    let read: Value = serde_json::from_str(&read.unwrap()).unwrap();
    assert_eq!(
        read,
        json!({"user": {"id": "42", "username": "a"}, "user_profile": {"bio": "hi"}, "badges": []})
    );
    h.assert_active();
}

#[test]
fn message_authors_lose_their_decorations() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let messages = json!([{"id": "1", "author": {"id": "42", "collectibles": {"nameplate": {}}}}]);
    let read = h
        .runtime()
        .filter_response(
            &get("/channels/7/messages?limit=50"),
            200,
            Some(messages.to_string()),
        )
        .unwrap();
    assert!(!read.contains("collectibles"), "{read}");
}

#[test]
fn other_routes_and_plain_bodies_pass() {
    let mut h = Harness::for_crate(PLUGIN).unwrap();
    let plain = r#"[{"id":"1","author":{"id":"42"}}]"#.to_string();
    let read = h
        .runtime()
        .filter_response(&get("/channels/7/messages"), 200, Some(plain.clone()));
    assert_eq!(read.as_deref(), Some(plain.as_str()));
    let themed = r#"{"theme_colors":[1]}"#.to_string();
    let read = h
        .runtime()
        .filter_response(&get("/guilds/3/members/42"), 200, Some(themed.clone()));
    assert_eq!(read.as_deref(), Some(themed.as_str()));
}
