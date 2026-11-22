//! Read All, after Vencord's `ReadAllNotificationsButton`: a button on top of
//! the server list that marks everything read.
//!
//! The smallest gesture-bound action plugin: `actions::mark_all_read` only
//! works while the host is delivering the user's click (or command), so the
//! plugin can offer the button but could never mark things read on its own.

use rekindle_sdk::prelude::*;

struct ReadAll;

fn unread_tooltip() -> String {
    match state::unread_channels(200).len() {
        0 => "Nothing unread".into(),
        1 => "Mark 1 channel read".into(),
        n => format!("Mark {n} channels read"),
    }
}

/// Places the button, or updates its tooltip.
fn show_button() -> Result<(), slots::UiError> {
    slots::button(
        slots::GUILD_LIST,
        "read-all",
        Icon::Check,
        &unread_tooltip(),
        false,
    )
}

/// Updates the button's tooltip. The button is placed at activation, so a
/// failure here was already reported there.
fn update_button() {
    let _ = show_button();
}

fn read_all() {
    match actions::mark_all_read() {
        Ok(()) => ui::toast_kind("Marked everything read", ToastKind::Success),
        Err(e) => ui::toast_kind(&format!("Couldn't mark read: {e:?}"), ToastKind::Error),
    }
    update_button();
}

impl Plugin for ReadAll {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        if let Err(e) = show_button() {
            warn!("no server-list button on this build: {e:?}");
        }
        ui::register_command("read-all", "Mark everything read");
        events::subscribe(&[EventKind::ReadState]);
        Ok(ReadAll)
    }

    fn on_event(&mut self, event: Event) {
        if let Event::ReadState(_) = event {
            update_button();
        }
    }

    fn on_slot(&mut self, ev: SlotEvent) {
        if matches!(&ev, SlotEvent::Clicked(c) if c.item == "read-all") {
            read_all();
        }
    }

    fn on_command(&mut self, id: &str) {
        if id == "read-all" {
            read_all();
        }
    }
}

rekindle_sdk::plugin!(ReadAll);
