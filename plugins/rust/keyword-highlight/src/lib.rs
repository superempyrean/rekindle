//! Keyword Highlight: highlights words you choose in every message, and
//! optionally tints the whole row.
//!
//! A `render-messages` plugin: [`Plugin::render`] runs in the render
//! isolate, which sees message content but can't keep or send it, so this
//! doesn't need `read-messages`. The keyword list is a
//! `string-list` setting from the manifest; changing it re-renders
//! automatically.

use rekindle_sdk::SpanColor;
use rekindle_sdk::prelude::*;

struct KeywordHighlight;

impl Plugin for KeywordHighlight {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        Ok(KeywordHighlight)
    }

    fn render(input: &RenderInput) -> Vec<Transform> {
        let color = || Some(SpanColor::Token("text-feedback-warning".into()));
        let found: Vec<Transform> = settings::items("keywords")
            .iter()
            .filter_map(|k| render::highlight_all(&input.message.content, k.trim(), color(), true))
            .collect();
        if !found.is_empty() && settings::bool("tint-rows") {
            let mut out = found;
            out.push(render::background(SpanColor::Token(
                "background-mentioned".into(),
            )));
            return out;
        }
        found
    }
}

rekindle_sdk::plugin!(KeywordHighlight);
