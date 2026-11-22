//! Message Click Actions, after Vencord's `MessageClickActions`: double-click
//! your own message to edit it, someone else's to reply; Shift+double-click
//! to delete (yours, or anyone's with Manage Messages).
//!
//! Click bindings (owner decision 2026-11-07): the plugin claims a click the
//! client doesn't use; if another plugin claims the same one, the user picks
//! which wins in rekindle's settings. Each click is a gesture, so the edit,
//! reply and delete happen only because the user clicked.

use rekindle_sdk::prelude::*;
use rekindle_sdk::slots::{self, ClickArea, MenuPredicate, NO_MODS};

struct ClickActions;

impl Plugin for ClickActions {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let shift = slots::mods(true, false, false);
        let r = [
            slots::bind_click(
                "edit",
                "Edit on double-click",
                ClickArea::Content,
                2,
                NO_MODS,
                &[MenuPredicate::OwnMessage],
            ),
            slots::bind_click(
                "reply",
                "Reply on double-click",
                ClickArea::Content,
                2,
                NO_MODS,
                &[MenuPredicate::OthersMessage],
            ),
            slots::bind_click(
                "delete",
                "Delete on Shift+double-click",
                ClickArea::Content,
                2,
                shift,
                &[MenuPredicate::CanDelete],
            ),
        ];
        for (i, e) in r.iter().enumerate() {
            if let Err(e) = e {
                warn!("click binding {i} not available: {e:?}");
            }
        }
        Ok(ClickActions)
    }

    fn on_slot(&mut self, ev: SlotEvent) {
        let SlotEvent::MessageClicked((binding, m)) = ev else {
            return;
        };
        // The host fires a binding only where its predicate holds: "edit" on
        // the user's own messages, "reply" on everyone else's.
        let r = match binding.as_str() {
            "edit" => actions::start_edit(&m),
            "reply" => actions::start_reply(&m, settings::bool("reply-mention")),
            "delete" => actions::delete_message(&m),
            _ => return,
        };
        if let Err(e) = r {
            ui::toast_kind(&format!("Couldn't {binding}: {e:?}"), ToastKind::Error);
        }
    }
}

rekindle_sdk::plugin!(ClickActions);
