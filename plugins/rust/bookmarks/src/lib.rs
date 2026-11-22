//! Bookmarks: a slots plugin.
//!
//! - "Bookmark message" and "Remove bookmark" in every message's right-click
//!   menu.
//! - A chat-bar button that opens a modal listing your bookmarks.
//! - A "Bookmarks" section in user profiles: how many of your bookmarks are
//!   that user's messages.
//!
//! Only message **ids** are stored (message, channel, author). The host never
//! gives this plugin message content. Everything stays on this device.

use rekindle_sdk::prelude::*;
use rekindle_sdk::slots::UiError;

const MAX_BOOKMARKS: usize = 200;
const MODAL: &str = "bookmarks";

#[derive(Clone, Copy, PartialEq)]
struct Bookmark {
    message: u64,
    channel: u64,
    author: u64,
}

struct Bookmarks {
    items: Vec<Bookmark>,
}

impl Bookmarks {
    fn load() -> Self {
        let items = storage::get("bookmarks")
            .unwrap_or_default()
            .split(';')
            .filter_map(|e| {
                let mut f = e.split(',').map(|x| x.parse::<u64>().ok());
                Some(Bookmark {
                    message: f.next()??,
                    channel: f.next()??,
                    author: f.next()??,
                })
            })
            .collect();
        Self { items }
    }

    fn save(&self) {
        let s: Vec<String> = self
            .items
            .iter()
            .map(|b| format!("{},{},{}", b.message, b.channel, b.author))
            .collect();
        if let Err(e) = storage::set("bookmarks", s.join(";")) {
            ui::toast_kind(&format!("Couldn't save bookmarks: {e:?}"), ToastKind::Error);
        }
    }

    fn show_modal(&self) {
        let mut p = Panel::new(MODAL, "Bookmarks");
        if self.items.is_empty() {
            p = p.text("No bookmarks yet. Right-click a message and choose \"Bookmark message\".");
        }
        for (i, b) in self.items.iter().enumerate().rev().take(25) {
            p = p
                .text(&format!(
                    "Message {} in channel {} (author {})",
                    b.message, b.channel, b.author
                ))
                .button(&format!("remove-{i}"), "Remove");
        }
        if !self.items.is_empty() {
            p = p.separator().button("clear", "Remove all");
        }
        p.button("close", "Close").show_modal();
    }
}

impl Plugin for Bookmarks {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let b = Bookmarks::load();
        // Each slot is optional: if this client build lacks one, keep going.
        for r in [
            slots::add(
                slots::MESSAGE_MENU,
                MenuItem::new("bookmark", "Bookmark message").icon(Icon::Bookmark),
            ),
            slots::add(
                slots::MESSAGE_MENU,
                MenuItem::new("unbookmark", "Remove bookmark").danger(),
            ),
            slots::button(slots::CHAT_BAR, "open", Icon::Bookmark, "Bookmarks", false),
            slots::profile_section("profile", "Bookmarks"),
        ] {
            if let Err(UiError::Unavailable) = r {
                warn!("a slot isn't available on this client build; that feature is off");
            } else if let Err(e) = r {
                error!("slot registration failed: {e:?}");
            }
        }
        Ok(b)
    }

    fn on_slot(&mut self, ev: SlotEvent) {
        match ev {
            SlotEvent::Clicked(SlotClick {
                item,
                target: Target::Message(m),
                ..
            }) => {
                let bm = Bookmark {
                    message: m.id,
                    channel: m.channel,
                    author: m.author,
                };
                let exists = self.items.contains(&bm);
                match (item.as_str(), exists) {
                    ("bookmark", false) if self.items.len() >= MAX_BOOKMARKS => {
                        ui::toast_kind("Bookmark limit reached", ToastKind::Warning);
                    }
                    ("bookmark", false) => {
                        self.items.push(bm);
                        self.save();
                        ui::toast_kind("Bookmarked", ToastKind::Success);
                    }
                    ("bookmark", true) => ui::toast("Already bookmarked"),
                    ("unbookmark", true) => {
                        self.items.retain(|b| *b != bm);
                        self.save();
                        ui::toast("Bookmark removed");
                    }
                    ("unbookmark", false) => ui::toast("That message isn't bookmarked"),
                    _ => {}
                }
            }
            SlotEvent::Clicked(c) if c.item == "open" => self.show_modal(),
            SlotEvent::ProfileOpened((_, user)) => {
                let n = self.items.iter().filter(|b| b.author == user.id).count();
                let text = match n {
                    0 => format!("You haven't bookmarked any messages from {}.", user.name),
                    1 => format!("You bookmarked 1 message from {}.", user.name),
                    n => format!("You bookmarked {n} messages from {}.", user.name),
                };
                slots::set_profile_section("profile", user.id, &[ui::Element::Text(text)]);
            }
            _ => {}
        }
    }

    fn on_ui(&mut self, ev: UiEvent) {
        if ev.surface != MODAL {
            return;
        }
        match ev.element.as_str() {
            "close" => {
                ui::close_modal();
                return;
            }
            "clear" => self.items.clear(),
            other => match other
                .strip_prefix("remove-")
                .and_then(|i| i.parse::<usize>().ok())
            {
                Some(i) if i < self.items.len() => {
                    self.items.remove(i);
                }
                _ => return,
            },
        }
        self.save();
        self.show_modal();
    }
}

rekindle_sdk::plugin!(Bookmarks);
