//! View Raw, after Vencord's `ViewRaw`: right-click a message, user or server
//! to see what the client holds about it as JSON, with a button to copy it.
//!
//! `state::raw_json` is the host's own view with secrets stripped, and is
//! **unstable by design** (owner decision 2026-11-07): its shape can change
//! in any build, so this plugin shows it and never parses it.

use rekindle_sdk::prelude::*;

const MODAL: &str = "view-raw";

struct ViewRaw {
    last: Option<String>,
}

impl Plugin for ViewRaw {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        for slot in [
            slots::MESSAGE_MENU,
            slots::USER_MENU,
            slots::GUILD_MENU,
            slots::CHANNEL_MENU,
        ] {
            let item = MenuItem::new("view-raw", "View raw")
                .icon(Icon::Code)
                .group("developer");
            if let Err(e) = slots::add(slot, item) {
                debug!("{slot}: {e:?}");
            }
        }
        Ok(ViewRaw { last: None })
    }

    fn on_slot(&mut self, ev: SlotEvent) {
        let SlotEvent::Clicked(c) = ev else { return };
        if c.item != "view-raw" {
            return;
        }
        let Some(json) = state::raw_json(&c.target) else {
            ui::toast("Nothing to show for that");
            return;
        };
        Panel::new(MODAL, "Raw data")
            .text("What the client holds, secrets removed. The shape changes between builds.")
            .rich_text(vec![span(&json).code()])
            .button("copy", "Copy")
            .button("close", "Close")
            .show_modal();
        self.last = Some(json);
    }

    fn on_ui(&mut self, ev: UiEvent) {
        if ev.clicked(MODAL, "copy") {
            if let Some(j) = &self.last {
                match clipboard::write_text(j) {
                    Ok(()) => ui::toast_kind("Copied", ToastKind::Success),
                    Err(e) => ui::toast_kind(&format!("Couldn't copy: {e:?}"), ToastKind::Error),
                }
            }
        } else if ev.clicked(MODAL, "close") {
            ui::close_modal();
        }
    }
}

rekindle_sdk::plugin!(ViewRaw);
