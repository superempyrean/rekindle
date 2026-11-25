//! Kaomoji: a picker for text faces, in or beside the client's emoji picker.
//!
//! - Where the client has room for it, a Kaomoji tab in the emoji picker,
//!   beside Emoji, GIFs and Stickers: the categories, with the recently used
//!   faces first, and a button per face. Picking a face puts it in the
//!   composer at the cursor, and the picker stays open.
//! - Elsewhere, a chat-bar button (and a keybind) opens a modal with a
//!   search field as well. Clicking a face puts it in the composer and
//!   closes the modal, as picking an emoji does.
//! - `/kaomoji <search>` with autocomplete puts the chosen face in the
//!   composer.
//!
//! The recently used faces are stored on this device. Nothing is sent
//! anywhere: the user still sends the message.

mod faces;

use faces::CATEGORIES;
use rekindle_sdk::actions;
use rekindle_sdk::prelude::*;
use rekindle_sdk::slots::UiError;

/// The modal's id, and the picker tab's and its panel's.
const MODAL: &str = "kaomoji";
const RECENT_KEY: &str = "recent";
const MAX_RECENT: usize = 18;
/// Search results shown at once.
const MAX_RESULTS: usize = 40;
/// The category list's first entry, before [`CATEGORIES`].
const RECENT: &str = "Recently used";

struct Kaomoji {
    query: String,
    /// Index into the category list: 0 is recently used, then [`CATEGORIES`].
    category: u32,
    recent: Vec<String>,
    /// The faces on the modal's buttons now, by button index.
    shown: Vec<String>,
    /// The faces are in a tab of the client's emoji picker, not a modal.
    in_picker: bool,
}

impl Kaomoji {
    fn save_recent(&self) {
        if let Err(e) = storage::set(RECENT_KEY, self.recent.join("\n")) {
            warn!("couldn't save recently used kaomoji: {e:?}");
        }
    }

    fn used(&mut self, face: &str) {
        self.recent.retain(|f| f != face);
        self.recent.insert(0, face.to_string());
        self.recent.truncate(MAX_RECENT);
        self.save_recent();
    }

    fn open(&mut self) {
        self.query.clear();
        self.category = u32::from(self.recent.is_empty());
        self.show();
    }

    fn show(&mut self) {
        let mut p = Panel::new(MODAL, "Kaomoji").text_input(
            "search",
            "Search",
            &self.query,
            "happy, shrug, cat, table…",
        );
        self.shown = if self.query.trim().is_empty() {
            let mut options = vec![RECENT];
            options.extend(CATEGORIES.iter().map(|c| c.name));
            p = p.select("category", "Category", &options, self.category);
            match self.category {
                0 => self.recent.clone(),
                i => CATEGORIES
                    .get(i as usize - 1)
                    .map(|c| c.faces.iter().map(|(f, _)| f.to_string()).collect())
                    .unwrap_or_default(),
            }
        } else {
            faces::search(&self.query)
                .into_iter()
                .take(MAX_RESULTS)
                .map(String::from)
                .collect()
        };
        if self.shown.is_empty() {
            p = p.text(if self.query.trim().is_empty() {
                "Faces you pick show up here."
            } else {
                "No kaomoji match that search."
            });
        }
        for (i, face) in self.shown.iter().enumerate() {
            p = p.button(&format!("face-{i}"), face);
        }
        if self.in_picker {
            p.show();
        } else {
            p.show_modal();
        }
    }

    fn pick(&mut self, face: &str) {
        match actions::insert_text(face) {
            Ok(()) => {
                self.used(face);
                if self.in_picker {
                    // The picker stays open, with the face first in
                    // Recently used.
                    self.show();
                } else {
                    ui::close_modal();
                }
            }
            Err(e) => ui::toast_kind(
                &format!("Couldn't put the kaomoji in the composer: {e:?}"),
                ToastKind::Error,
            ),
        }
    }
}

impl Plugin for Kaomoji {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let recent = storage::get(RECENT_KEY)
            .unwrap_or_default()
            .split('\n')
            .filter(|f| !f.is_empty())
            .map(String::from)
            .collect();
        let in_picker = match slots::picker_tab(MODAL, "Kaomoji") {
            Ok(()) => true,
            Err(UiError::Unavailable) => false,
            Err(e) => {
                error!("emoji picker tab: {e:?}");
                false
            }
        };
        if !in_picker {
            match slots::button(slots::CHAT_BAR, "open", Icon::Smile, "Kaomoji", false) {
                Err(UiError::Unavailable) => {
                    warn!("no chat-bar button on this client build; use /kaomoji or the keybind");
                }
                Err(e) => error!("chat-bar button: {e:?}"),
                Ok(()) => {}
            }
        }
        if let Err(e) =
            ui::register_keybind("open", "Open the kaomoji picker", Some("Ctrl+Shift+K"))
        {
            warn!("no keybind: {e:?}");
        }
        let cmd = SlashCommand::new("kaomoji", "Put a kaomoji in the composer")
            .text("search", "A mood or a name: happy, shrug, cat…", true)
            .autocomplete();
        if let Err(e) = commands::register(cmd) {
            warn!("no slash command: {e:?}");
        }
        let mut k = Kaomoji {
            query: String::new(),
            category: 1,
            recent,
            shown: Vec::new(),
            in_picker,
        };
        if in_picker {
            // The tab's body is there before the user first picks it.
            k.open();
        }
        Ok(k)
    }

    fn on_slot(&mut self, ev: SlotEvent) {
        match &ev {
            SlotEvent::Clicked(c) if c.slot == slots::EXPRESSION_PICKER_TAB && c.item == MODAL => {
                self.open();
            }
            SlotEvent::Clicked(c) if c.item == "open" => self.open(),
            _ => {}
        }
    }

    fn on_keybind(&mut self, id: &str) {
        if id == "open" {
            self.open();
        }
    }

    fn on_ui(&mut self, ev: UiEvent) {
        if ev.surface != MODAL {
            return;
        }
        match (ev.element.as_str(), ev.value) {
            ("search", UiValue::Text(q)) => {
                self.query = q;
                self.show();
            }
            ("category", UiValue::Selected(i)) => {
                self.category = i;
                self.show();
            }
            (id, UiValue::Clicked) => {
                if let Some(face) = id
                    .strip_prefix("face-")
                    .and_then(|i| i.parse::<usize>().ok())
                    .and_then(|i| self.shown.get(i).cloned())
                {
                    self.pick(&face);
                }
            }
            _ => {}
        }
    }

    fn on_autocomplete(
        &mut self,
        command: &str,
        _option: &str,
        partial: &str,
    ) -> Vec<(String, String)> {
        if command != "kaomoji" {
            return Vec::new();
        }
        let found = if partial.trim().is_empty() {
            self.recent.clone()
        } else {
            faces::search(partial)
                .into_iter()
                .map(String::from)
                .collect()
        };
        found.into_iter().take(25).map(|f| (f.clone(), f)).collect()
    }

    fn on_slash_command(&mut self, inv: Invocation) -> CommandReply {
        let search = commands::text(&inv, "search").unwrap_or_default();
        // A face picked from autocomplete arrives as itself.
        let face = faces::all()
            .map(|(_, f, _)| f)
            .find(|f| *f == search.trim())
            .or_else(|| faces::search(&search).into_iter().next());
        match face {
            Some(f) => {
                self.used(f);
                CommandReply::Insert(f.to_string())
            }
            None => commands::local(&format!("No kaomoji match \"{search}\".")),
        }
    }
}

rekindle_sdk::plugin!(Kaomoji);
