//! Quick React: your three most-used emoji as buttons in the message hover
//! bar, one click to react.
//!
//! It asks for `react`, not `act` (owner decision 2026-11-07): reactions
//! only, gesture-bound and undoable. Clicking the same emoji again on the
//! same message removes the reaction this plugin added.

use std::collections::BTreeSet;

use rekindle_sdk::prelude::*;
use rekindle_sdk::state::Emoji;

struct QuickReact {
    /// Hover-bar button id -> emoji (`name:id` for custom ones).
    emoji: Vec<(String, String)>,
    /// Reactions this plugin added: (message id, emoji).
    mine: BTreeSet<(u64, String)>,
}

fn emoji_text(e: &Emoji) -> String {
    if e.id == 0 {
        e.name.clone()
    } else {
        format!("{}:{}", e.name, e.id)
    }
}

impl Plugin for QuickReact {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let mut emoji = Vec::new();
        for (i, e) in state::frequent_emoji(3).iter().enumerate() {
            let id = format!("react-{i}");
            let tooltip = format!("React with {}", e.name);
            match slots::button(slots::MESSAGE_HOVER_BAR, &id, Icon::Smile, &tooltip, false) {
                Ok(()) => emoji.push((id, emoji_text(e))),
                Err(err) => warn!("no hover-bar button: {err:?}"),
            }
        }
        Ok(QuickReact {
            emoji,
            mine: BTreeSet::new(),
        })
    }

    fn on_slot(&mut self, ev: SlotEvent) {
        let SlotEvent::Clicked(SlotClick {
            item,
            target: Target::Message(m),
            ..
        }) = ev
        else {
            return;
        };
        let Some((_, emoji)) = self.emoji.iter().find(|(id, _)| *id == item).cloned() else {
            return;
        };
        let key = (m.id, emoji.clone());
        let r = if self.mine.remove(&key) {
            actions::unreact(&m, &emoji)
        } else {
            let r = actions::react(&m, &emoji);
            if r.is_ok() {
                self.mine.insert(key);
            }
            r
        };
        if let Err(e) = r {
            ui::toast_kind(&format!("Couldn't react: {e:?}"), ToastKind::Error);
        }
    }
}

rekindle_sdk::plugin!(QuickReact);
