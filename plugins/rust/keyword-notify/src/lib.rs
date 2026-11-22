//! Keyword Notify: a `read-messages` plugin.
//!
//! Watches the content of messages your client receives for words you choose,
//! and shows a toast when one appears. It reads message content (that's the
//! `read-messages` capability) but stores only your keyword list and a per-word
//! match count, never message text. Everything stays on this device.
//!
//! This is the minimal, benign shape of message access: process in the moment,
//! keep nothing of other people's content.

use rekindle_sdk::prelude::*;

const PANEL: &str = "keyword-notify";

struct KeywordNotify {
    keywords: Vec<String>,
    mute: bool,
}

fn parse_keywords(s: &str) -> Vec<String> {
    s.split(',')
        .map(|w| w.trim().to_lowercase())
        .filter(|w| !w.is_empty())
        .collect()
}

impl KeywordNotify {
    fn load() -> Self {
        Self {
            keywords: parse_keywords(&storage::get("keywords").unwrap_or_default()),
            mute: storage::get_or("mute", false),
        }
    }

    fn total_hits(&self) -> u64 {
        self.keywords
            .iter()
            .map(|k| storage::get_or(&format!("hits:{k}"), 0u64))
            .sum()
    }

    fn show(&self) {
        let shown = if self.keywords.is_empty() {
            "(none)".to_string()
        } else {
            self.keywords.join(", ")
        };
        Panel::new(PANEL, "Keyword Notify")
            .text("Toast me when one of these words appears in a message. Comma-separated.")
            .text_input(
                "keywords",
                "Keywords",
                &self.keywords.join(", "),
                "e.g. deploy, release, @me",
            )
            .toggle("mute", "Don't toast, just count", self.mute)
            .separator()
            .text(&format!("Watching: {shown}"))
            .text(&format!("Matches so far: {}", self.total_hits()))
            .show();
    }

    /// Check one message's content against the keyword list. Returns
    /// whether a keyword matched.
    fn scan(&self, m: &Message) -> bool {
        let lower = m.content.to_lowercase();
        let mut matched = false;
        for k in &self.keywords {
            if lower.contains(k) {
                matched = true;
                let key = format!("hits:{k}");
                if let Err(e) = storage::set(&key, storage::get_or(&key, 0u64) + 1) {
                    warn!("couldn't count a match: {e:?}");
                }
                if !self.mute {
                    ui::toast(&format!(
                        "“{k}” mentioned by {} in channel {}",
                        m.author.name, m.channel
                    ));
                }
            }
        }
        matched
    }
}

fn save(key: &str, value: impl ToString) {
    if let Err(e) = storage::set(key, value) {
        warn!("couldn't save {key}: {e:?}");
    }
}

impl Plugin for KeywordNotify {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        events::subscribe(&[EventKind::Message]);
        let k = KeywordNotify::load();
        k.show();
        Ok(k)
    }

    fn on_event(&mut self, event: Event) {
        // Subscribed to message events only (see `activate`).
        if let Event::MessageCreated(message) | Event::MessageEdited((message, _)) = event
            && self.scan(&message)
        {
            self.show();
        }
    }

    fn on_ui(&mut self, ev: UiEvent) {
        if ev.surface != PANEL {
            return;
        }
        match (ev.element.as_str(), &ev.value) {
            ("keywords", UiValue::Text(t)) => {
                self.keywords = parse_keywords(t);
                save("keywords", self.keywords.join(","));
            }
            ("mute", UiValue::Toggled(on)) => {
                self.mute = *on;
                save("mute", *on);
            }
            _ => return,
        }
        self.show();
    }
}

rekindle_sdk::plugin!(KeywordNotify);
