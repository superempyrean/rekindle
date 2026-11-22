//! Typing Indicator, after Vencord's `TypingIndicator`: a badge on each
//! channel in the channel list while someone's typing there.
//!
//! The push model in action (design doc §6.4): the plugin keeps the
//! decorations up to date from `typing` events, and the host draws them
//! while it paints the channel list, without ever calling into the plugin.

use rekindle_sdk::decorations::{self, CHANNEL_LIST_TRAILING, Decoration};
use rekindle_sdk::prelude::*;

struct TypingIndicator;

fn names(users: &[u64]) -> String {
    let me = state::me().id;
    let names: Vec<String> = users
        .iter()
        .filter(|u| **u != me)
        .filter_map(|u| state::display_name(*u))
        .collect();
    match names.as_slice() {
        [] => String::new(),
        [a] => format!("{a} is typing…"),
        [a, b] => format!("{a} and {b} are typing…"),
        [a, b, rest @ ..] => format!("{a}, {b} and {} others are typing…", rest.len()),
    }
}

fn update(channel: u64, users: &[u64]) {
    let me = state::me().id;
    let others = users.iter().filter(|u| **u != me).count();
    // The channel you're looking at shows typing itself.
    let here = state::selection().channel == Some(channel);
    let d = (others > 0 && !here).then(|| {
        Decoration::icon(Icon::Chat)
            .with_text(&others.to_string())
            .tooltip(&names(users))
    });
    if let Err(e) = decorations::set(CHANNEL_LIST_TRAILING, channel, d) {
        debug!("couldn't decorate channel {channel}: {e:?}");
    }
}

impl Plugin for TypingIndicator {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        events::subscribe(&[EventKind::Typing, EventKind::SelectionChanged]);
        Ok(TypingIndicator)
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::Typing((channel, users)) => update(channel, &users),
            // The badge for the channel you just opened goes away.
            Event::SelectionChanged(sel) => {
                if let Some(ch) = sel.channel
                    && let Err(e) = decorations::set(CHANNEL_LIST_TRAILING, ch, None)
                {
                    debug!("couldn't clear channel {ch}: {e:?}");
                }
                if let Some(prev) = sel.previous_channel {
                    update(prev, &users::typing(prev));
                }
            }
            _ => {}
        }
    }
}

rekindle_sdk::plugin!(TypingIndicator);
