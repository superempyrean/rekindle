//! Mute Words: drops messages that contain a muted word before the client
//! processes them, so they never show up, unread counts included. An edit
//! that adds a muted word is dropped too, which leaves the message as it
//! was before the edit.

use rekindle_sdk::events::Verdict;
use rekindle_sdk::prelude::*;

struct MuteWords;

fn muted(content: &str) -> bool {
    let text = content.to_lowercase();
    settings::items("words")
        .iter()
        .map(|w| w.trim().to_lowercase())
        .any(|w| !w.is_empty() && text.contains(&w))
}

impl Plugin for MuteWords {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let refused = events::intercept(&[EventKind::Message]);
        if !refused.is_empty() {
            return Err(format!("can't intercept messages: {refused:?}"));
        }
        Ok(MuteWords)
    }

    fn on_intercept(&mut self, event: &Event) -> Verdict {
        match event {
            Event::MessageCreated(m) | Event::MessageEdited((m, _)) if muted(&m.content) => {
                Verdict::Drop
            }
            _ => Verdict::Pass,
        }
    }
}

rekindle_sdk::plugin!(MuteWords);
