//! Channel Notes: an events plugin.
//!
//! When you switch channels, shows a panel with your own private note for that
//! channel (and a palette command to clear it). Notes are yours, about where
//! you are. The plugin only ever sees the channel you selected, never other
//! people's messages.

use rekindle_sdk::prelude::*;
use rekindle_sdk::state::Channel;

const PANEL: &str = "notes";
const MAX_NOTE: usize = 4000;

struct Notes {
    current: Option<Channel>,
}

fn key(ch: &Channel) -> String {
    format!("note:{}", ch.id)
}

impl Notes {
    fn show(&self) {
        let Some(ch) = &self.current else {
            Panel::new(PANEL, "Channel notes")
                .text("Open a channel to see its note.")
                .show();
            return;
        };
        let note = storage::get(&key(ch)).unwrap_or_default();
        let where_ = if ch.guild.is_some() {
            format!("#{}", ch.name)
        } else {
            format!("DM: {}", ch.name)
        };
        Panel::new(PANEL, "Channel notes")
            .heading(&where_)
            .text_input(
                "note",
                "Note to self",
                &note,
                "Anything you want to remember here…",
            )
            .text(&format!("{} channel(s) with notes", Self::count()))
            .show();
    }

    fn count() -> usize {
        storage::keys()
            .iter()
            .filter(|k| k.starts_with("note:"))
            .count()
    }
}

impl Plugin for Notes {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        events::subscribe(&[EventKind::SelectionChanged]);
        let n = Notes {
            current: state::selection().channel.and_then(state::channel),
        };
        n.show();
        ui::register_command("clear-note", "Channel Notes: clear note for this channel");
        Ok(n)
    }

    fn on_event(&mut self, event: Event) {
        if let Event::SelectionChanged(sel) = event {
            self.current = sel.channel.and_then(state::channel);
            self.show();
        }
    }

    fn on_ui(&mut self, ev: UiEvent) {
        let (Some(ch), Some(text)) = (
            &self.current,
            ev.value.as_text().filter(|_| ev.is(PANEL, "note")),
        ) else {
            return;
        };
        let text: String = text.chars().take(MAX_NOTE).collect();
        if text.trim().is_empty() {
            storage::delete(&key(ch));
        } else if let Err(e) = storage::set(&key(ch), &text) {
            error!("could not save note: {e:?}");
        }
        self.show();
    }

    fn on_command(&mut self, id: &str) {
        if let ("clear-note", Some(ch)) = (id, &self.current) {
            storage::delete(&key(ch));
            info!("cleared note for channel {}", ch.id);
            self.show();
        }
    }
}

rekindle_sdk::plugin!(Notes);
