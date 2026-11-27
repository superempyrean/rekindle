//! No Profile Themes: profiles and message authors show without the
//! decoration their owners chose. The plugin takes `theme_colors`,
//! `profile_effect`, `avatar_decoration_data`, `collectibles` and
//! `display_name_styles` out of the client's REST responses for profiles
//! (`GET /users/{id}/profile`) and message history (`GET
//! /channels/{id}/messages`) before the client reads them.

use rekindle_sdk::events::{HttpResponse, ResponseVerdict};
use rekindle_sdk::prelude::*;
use serde_json::Value;

/// The fields a profile or a user is decorated with.
const DECORATION: [&str; 5] = [
    "theme_colors",
    "profile_effect",
    "avatar_decoration_data",
    "collectibles",
    "display_name_styles",
];

/// Remove [`DECORATION`] from every object in `value`. True if any was
/// there.
fn strip(value: &mut Value) -> bool {
    match value {
        Value::Array(items) => items.iter_mut().fold(false, |found, v| strip(v) | found),
        Value::Object(map) => {
            let mut found = false;
            for key in DECORATION {
                found |= map.remove(key).is_some();
            }
            map.values_mut().fold(found, |found, v| strip(v) | found)
        }
        _ => false,
    }
}

/// The body without decoration, or `None` if it had none.
fn stripped(body: &str) -> Option<String> {
    if !DECORATION.iter().any(|k| body.contains(k)) {
        return None;
    }
    let mut value: Value = serde_json::from_str(body).ok()?;
    strip(&mut value).then(|| value.to_string())
}

struct NoProfileThemes;

impl Plugin for NoProfileThemes {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let refused = events::intercept_http(&[EventKind::Member, EventKind::Message]);
        if !refused.is_empty() {
            return Err(format!("can't see profiles and messages: {refused:?}"));
        }
        Ok(NoProfileThemes)
    }

    fn on_response(&mut self, res: &HttpResponse) -> ResponseVerdict {
        let path = res.path.split('?').next().unwrap_or_default();
        let wanted = path.ends_with("/profile") || path.ends_with("/messages");
        match &res.body {
            Some(body) if res.method == "GET" && wanted && (200..300).contains(&res.status) => {
                stripped(body).map_or(ResponseVerdict::Pass, ResponseVerdict::Replace)
            }
            _ => ResponseVerdict::Pass,
        }
    }
}

rekindle_sdk::plugin!(NoProfileThemes);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_a_profile_and_nested_users() {
        let body = r#"{"user":{"id":"1","avatar_decoration_data":{"asset":"a"},"collectibles":{}},
            "user_profile":{"bio":"hi","theme_colors":[1,2],"profile_effect":{"id":"3"}}}"#;
        let out: Value = serde_json::from_str(&stripped(body).unwrap()).unwrap();
        assert_eq!(
            out,
            serde_json::json!({"user": {"id": "1"}, "user_profile": {"bio": "hi"}})
        );
    }

    #[test]
    fn a_body_without_decoration_passes() {
        assert_eq!(stripped(r#"[{"author":{"id":"1"}}]"#), None);
        assert_eq!(stripped("not json theme_colors"), None);
    }
}
