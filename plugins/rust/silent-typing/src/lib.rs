//! Silent Typing, after Vencord's `SilentTyping`: stop sending typing
//! indicators, toggled from a chat-bar button or `/silenttyping`.
//!
//! Plugins can't patch the client, so the behaviour itself is a
//! **host-maintained tweak** (design doc §9.1, first-party only): this plugin
//! is the UI for it. If the user set the tweak by hand, theirs wins.

use rekindle_sdk::commands::{self, CommandReply, Invocation, SlashCommand};
use rekindle_sdk::prelude::*;

const TWEAK: &str = "silent-typing";

struct SilentTyping;

fn on() -> bool {
    tweaks::all()
        .iter()
        .any(|t| t.id == TWEAK && t.value == "true")
}

fn set(enabled: bool) -> String {
    match tweaks::set(TWEAK, if enabled { "true" } else { "false" }) {
        Ok(()) => {
            // Without the button (see `activate`) there is nothing to update.
            let _ = slots::set_active(slots::CHAT_BAR, "toggle", enabled);
            if enabled {
                "Silent typing on: others won't see you typing.".into()
            } else {
                "Silent typing off.".into()
            }
        }
        Err(e) => format!("Couldn't change silent typing: {e:?}"),
    }
}

impl Plugin for SilentTyping {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        if !tweaks::all().iter().any(|t| t.id == TWEAK && t.available) {
            warn!("the silent-typing tweak isn't available on this client build");
        }
        if let Err(e) = slots::button(
            slots::CHAT_BAR,
            "toggle",
            Icon::EyeOff,
            "Silent typing",
            on(),
        ) {
            warn!("no chat-bar button on this build; use /silenttyping: {e:?}");
        }
        let cmd = SlashCommand::new("silenttyping", "Turn sending typing indicators off or on")
            .boolean("enabled", "On or off", false);
        if let Err(e) = commands::register(cmd) {
            warn!("no slash commands: {e:?}");
        }
        Ok(SilentTyping)
    }

    fn on_slot(&mut self, ev: SlotEvent) {
        if matches!(&ev, SlotEvent::Clicked(c) if c.item == "toggle") {
            let msg = set(!on());
            ui::toast(&msg);
        }
    }

    fn on_slash_command(&mut self, inv: Invocation) -> CommandReply {
        let enabled = commands::boolean(&inv, "enabled").unwrap_or(!on());
        commands::local(&set(enabled))
    }
}

rekindle_sdk::plugin!(SilentTyping);
