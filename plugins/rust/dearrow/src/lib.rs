//! DeArrow: YouTube embeds in message history show the title and thumbnail
//! that DeArrow's community submitted, in place of the uploader's.
//!
//! The plugin rewrites the embeds in the client's `GET
//! /channels/{id}/messages` responses. Each video is looked up once on
//! `sponsor.ajay.app` and kept for the session. A response looks up at most
//! [`LOOKUPS`] new videos, so a channel full of them waits for a few; the
//! rest are rewritten the next time the channel loads.

use std::collections::HashMap;

use rekindle_sdk::events::{HttpResponse, ResponseVerdict};
use rekindle_sdk::prelude::*;
use serde::Deserialize;
use serde_json::Value;

/// New videos looked up per response.
const LOOKUPS: usize = 3;
/// Videos kept before the cache starts again.
const CACHE: usize = 512;

/// What DeArrow has for a video.
#[derive(Clone, Debug, Default, PartialEq)]
struct Branding {
    title: Option<String>,
    thumbnail: Option<String>,
}

#[derive(Deserialize)]
struct Submission {
    #[serde(default)]
    title: String,
    #[serde(default)]
    timestamp: Option<f64>,
    #[serde(default)]
    original: bool,
    #[serde(default)]
    votes: i64,
    #[serde(default)]
    locked: bool,
}

#[derive(Deserialize)]
struct Answer {
    #[serde(default)]
    titles: Vec<Submission>,
    #[serde(default)]
    thumbnails: Vec<Submission>,
}

/// A submission DeArrow shows by default: not the original, and not voted
/// down unless locked.
fn trusted(s: &Submission) -> bool {
    !s.original && (s.votes >= 0 || s.locked)
}

/// DeArrow marks a word to keep as typed with a leading `>`.
fn clean_title(title: &str) -> String {
    title
        .split(' ')
        .map(|w| w.strip_prefix('>').filter(|r| !r.is_empty()).unwrap_or(w))
        .collect::<Vec<_>>()
        .join(" ")
}

fn branding(video: &str, answer: &str) -> Branding {
    let Ok(a) = serde_json::from_str::<Answer>(answer) else {
        return Branding::default();
    };
    Branding {
        title: a
            .titles
            .iter()
            .find(|s| trusted(s))
            .map(|s| clean_title(&s.title)),
        thumbnail: a.thumbnails.iter().find(|s| trusted(s)).and_then(|s| {
            s.timestamp.map(|t| {
                format!(
                    "https://dearrow-thumb.ajay.app/api/v1/getThumbnail?videoID={video}&time={t}"
                )
            })
        }),
    }
}

/// The 11-character video id of a YouTube URL.
fn video_id(url: &str) -> Option<&str> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let rest = rest.strip_prefix("m.").unwrap_or(rest);
    let tail = rest
        .strip_prefix("youtu.be/")
        .or_else(|| rest.strip_prefix("youtube.com/shorts/"))
        .or_else(|| {
            rest.strip_prefix("youtube.com/watch?")?
                .split('&')
                .find_map(|kv| kv.strip_prefix("v="))
        })?;
    let id = tail.get(..11)?;
    id.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        .then_some(id)
}

#[derive(Default)]
struct DeArrow {
    cache: HashMap<String, Branding>,
}

impl DeArrow {
    /// What DeArrow has for `video`, looked up while `budget` lasts.
    fn lookup(&mut self, video: &str, budget: &mut usize) -> Option<Branding> {
        if let Some(b) = self.cache.get(video) {
            return Some(b.clone());
        }
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        let url = format!("https://sponsor.ajay.app/api/branding?videoID={video}");
        let found = match http::get(&url) {
            Ok(r) if r.ok() => branding(video, &r.text()),
            Ok(r) if r.status() == 404 => Branding::default(),
            Ok(r) => {
                warn!("DeArrow answered {} for {video}", r.status());
                return None;
            }
            Err(e) => {
                warn!("DeArrow lookup for {video}: {e:?}");
                return None;
            }
        };
        if self.cache.len() >= CACHE {
            self.cache.clear();
        }
        self.cache.insert(video.to_string(), found.clone());
        Some(found)
    }

    /// Rewrite the YouTube embeds in a list of messages. True if any changed.
    fn rewrite(&mut self, messages: &mut [Value]) -> bool {
        let (titles, thumbnails) = (settings::bool("titles"), settings::bool("thumbnails"));
        let mut budget = LOOKUPS;
        let mut changed = false;
        for embed in messages
            .iter_mut()
            .filter_map(|m| m.get_mut("embeds").and_then(Value::as_array_mut))
            .flatten()
        {
            let Some(video) = embed
                .get("url")
                .and_then(Value::as_str)
                .and_then(video_id)
                .map(str::to_string)
            else {
                continue;
            };
            let Some(b) = self.lookup(&video, &mut budget) else {
                continue;
            };
            if let Some(t) = b.title.filter(|_| titles)
                && embed.get("title").and_then(Value::as_str) != Some(t.as_str())
            {
                embed["title"] = Value::String(t);
                changed = true;
            }
            if let Some(url) = b.thumbnail.filter(|_| thumbnails)
                && let Some(thumb) = embed.get_mut("thumbnail").and_then(Value::as_object_mut)
            {
                thumb.insert("url".into(), Value::String(url.clone()));
                thumb.insert("proxy_url".into(), Value::String(url));
                changed = true;
            }
        }
        changed
    }
}

impl Plugin for DeArrow {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let refused = events::intercept_http(&[EventKind::Message]);
        if !refused.is_empty() {
            return Err(format!("can't see message history: {refused:?}"));
        }
        Ok(DeArrow::default())
    }

    fn on_response(&mut self, res: &HttpResponse) -> ResponseVerdict {
        let path = res.path.split('?').next().unwrap_or_default();
        let Some(body) = res.body.as_deref().filter(|b| {
            res.method == "GET"
                && res.status == 200
                && path.ends_with("/messages")
                && b.contains("youtu")
        }) else {
            return ResponseVerdict::Pass;
        };
        let Ok(Value::Array(mut messages)) = serde_json::from_str(body) else {
            return ResponseVerdict::Pass;
        };
        if self.rewrite(&mut messages) {
            ResponseVerdict::Replace(Value::Array(messages).to_string())
        } else {
            ResponseVerdict::Pass
        }
    }
}

rekindle_sdk::plugin!(DeArrow);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_ids() {
        assert_eq!(
            video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1"),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            video_id("https://youtu.be/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            video_id("https://youtube.com/shorts/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(video_id("https://example.com/watch?v=dQw4w9WgXcQ"), None);
    }

    #[test]
    fn takes_the_first_trusted_submission() {
        let answer = r#"{"titles":[{"title":"Original","original":true,"votes":5},
            {"title":"The >TRUE title","original":false,"votes":0}],
            "thumbnails":[{"timestamp":12.5,"original":false,"votes":-1,"locked":false},
            {"timestamp":30,"original":false,"votes":2}]}"#;
        let b = branding("abc", answer);
        assert_eq!(b.title.as_deref(), Some("The TRUE title"));
        assert_eq!(
            b.thumbnail.as_deref(),
            Some("https://dearrow-thumb.ajay.app/api/v1/getThumbnail?videoID=abc&time=30")
        );
    }
}
