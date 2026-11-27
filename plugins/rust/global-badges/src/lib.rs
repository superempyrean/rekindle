//! Global Badges: profiles show the badges other client mods (Vencord,
//! Equicord, Aliucord and others) give their users, next to Discord's own.
//!
//! The plugin downloads Equicord's map of every user's badges from
//! `badges.equicord.org` when it starts and every six hours after, and adds
//! a user's badges to the client's `GET /users/{id}/profile` response
//! before the client reads it. Until the first download arrives, profiles
//! show only Discord's badges.

use std::collections::HashMap;

use rekindle_sdk::events::{HttpResponse, ResponseVerdict};
use rekindle_sdk::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};

const API: &str = "https://badges.equicord.org/users";
/// How often the map is downloaded again, and how soon after a failure.
const REFRESH_MS: u64 = 6 * 60 * 60 * 1000;
const RETRY_MS: u64 = 60 * 1000;

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Badge {
    #[serde(default)]
    tooltip: String,
    #[serde(default, rename = "mod")]
    source: String,
    #[serde(default, rename = "badge")]
    image: String,
}

#[derive(Deserialize)]
struct Answer {
    users: HashMap<String, Vec<Badge>>,
}

/// The user id of a profile path, `/users/{id}/profile`.
fn profile_user(path: &str) -> Option<&str> {
    let path = path.split('?').next()?;
    let id = path.strip_prefix("/users/")?.strip_suffix("/profile")?;
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then_some(id)
}

/// `body` with `badges` added after Discord's own.
fn with_badges(body: &str, badges: &[Badge]) -> Option<String> {
    let mut profile: Value = serde_json::from_str(body).ok()?;
    let list = profile
        .as_object_mut()?
        .entry("badges")
        .or_insert_with(|| json!([]));
    let list = list.as_array_mut()?;
    for (k, b) in badges.iter().enumerate() {
        list.push(json!({
            "id": format!("rekindle-{}-{k}", b.source),
            "description": if b.tooltip.is_empty() { &b.source } else { &b.tooltip },
            "icon": b.image,
            "link": b.image,
        }));
    }
    Some(profile.to_string())
}

#[derive(Default)]
struct GlobalBadges {
    users: HashMap<String, Vec<Badge>>,
}

impl GlobalBadges {
    fn download(&mut self) -> bool {
        let r = match http::get(API) {
            Ok(r) if r.ok() => r,
            Ok(r) => {
                warn!("the badge API answered {}", r.status());
                return false;
            }
            Err(e) => {
                warn!("the badge API: {e:?}");
                return false;
            }
        };
        match serde_json::from_slice::<Answer>(r.bytes()) {
            Ok(a) => {
                info!("badges for {} users", a.users.len());
                self.users = a.users;
                true
            }
            Err(e) => {
                warn!("the badge API's answer: {e}");
                false
            }
        }
    }

    /// The badges shown for `user`.
    fn badges(&self, user: &str) -> Vec<Badge> {
        let vault = settings::bool("badgevault");
        self.users
            .get(user)
            .into_iter()
            .flatten()
            .filter(|b| {
                let source = b.source.to_ascii_lowercase();
                source != "discord" && (vault || source != "badgevault")
            })
            .filter(|b| b.image.starts_with("https://"))
            .cloned()
            .collect()
    }
}

impl Plugin for GlobalBadges {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let refused = events::intercept_http(&[EventKind::Member]);
        if !refused.is_empty() {
            return Err(format!("can't see profiles: {refused:?}"));
        }
        timer::set("download", 50).map_err(|e| format!("{e:?}"))?;
        Ok(GlobalBadges::default())
    }

    fn on_timer(&mut self, _id: &str) {
        let next = if self.download() {
            REFRESH_MS
        } else {
            RETRY_MS
        };
        let _ = timer::set("download", next);
    }

    fn on_response(&mut self, res: &HttpResponse) -> ResponseVerdict {
        let (Some(user), Some(body)) = (profile_user(&res.path), res.body.as_deref()) else {
            return ResponseVerdict::Pass;
        };
        if res.method != "GET" || !(200..300).contains(&res.status) {
            return ResponseVerdict::Pass;
        }
        let badges = self.badges(user);
        if badges.is_empty() {
            return ResponseVerdict::Pass;
        }
        with_badges(body, &badges).map_or(ResponseVerdict::Pass, ResponseVerdict::Replace)
    }
}

rekindle_sdk::plugin!(GlobalBadges);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_paths() {
        assert_eq!(
            profile_user("/users/42/profile?with_mutual_guilds=true"),
            Some("42")
        );
        assert_eq!(profile_user("/users/@me/profile"), None);
        assert_eq!(profile_user("/users/42/notes"), None);
    }

    #[test]
    fn badges_go_after_discords_own() {
        let body =
            r#"{"user":{"id":"42"},"badges":[{"id":"staff","description":"Staff","icon":"x"}]}"#;
        let added = with_badges(
            body,
            &[Badge {
                tooltip: "Vencord Contributor".into(),
                source: "vencord".into(),
                image: "https://badges.equicord.org/public/badges/vencord.png".into(),
            }],
        )
        .unwrap();
        let v: Value = serde_json::from_str(&added).unwrap();
        assert_eq!(v["badges"][0]["id"], "staff");
        assert_eq!(v["badges"][1]["description"], "Vencord Contributor");
    }
}
