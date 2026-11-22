//! Accent: a UI-panel plugin.
//!
//! A settings panel with a color picker for the accent, a toggle + slider that
//! tint the whole theme towards it (one `theme::tint` call, like a custom
//! client theme), and a reset button. Every change is applied live and
//! persisted.

use rekindle_sdk::prelude::*;

const PANEL: &str = "settings";
const DEFAULT_ACCENT: Color = Color::rgb(0xeb, 0x45, 0x9e);

struct Accent {
    color: Color,
    tint: bool,
    /// Percent, 0..=40.
    strength: f64,
}

impl Accent {
    fn load() -> Self {
        Self {
            color: storage::get("accent")
                .and_then(|h| Color::from_hex(&h))
                .unwrap_or(DEFAULT_ACCENT),
            tint: storage::get_or("tint", false),
            strength: storage::get_or("strength", 12.0f64).clamp(0.0, 40.0),
        }
    }

    fn save(&self) {
        let results = [
            storage::set("accent", self.color.to_hex()),
            storage::set("tint", self.tint),
            storage::set("strength", self.strength),
        ];
        if results.iter().any(Result::is_err) {
            warn!("could not save settings: {results:?}");
        }
    }

    fn apply(&self) {
        theme::set("background-brand", self.color);
        if self.tint {
            theme::tint(self.color, (self.strength / 100.0) as f32);
        } else {
            theme::clear_tint();
        }
    }

    fn show(&self) {
        let mut p = Panel::new(PANEL, "Accent")
            .color_picker("accent", "Accent color", self.color)
            .toggle("tint", "Tint backgrounds", self.tint);
        if self.tint {
            p = p.slider(
                "strength",
                "Tint strength (%)",
                0.0..=40.0,
                1.0,
                self.strength,
            );
        }
        p.separator().button("reset", "Reset to defaults").show();
    }
}

impl Plugin for Accent {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let a = Accent::load();
        a.apply();
        a.show();
        Ok(a)
    }

    fn on_ui(&mut self, ev: UiEvent) {
        if ev.surface != PANEL {
            return;
        }
        match (ev.element.as_str(), &ev.value) {
            ("accent", UiValue::Color(c)) => self.color = *c,
            ("tint", UiValue::Toggled(on)) => self.tint = *on,
            ("strength", UiValue::Number(n)) => self.strength = n.clamp(0.0, 40.0),
            ("reset", UiValue::Clicked) => {
                self.color = DEFAULT_ACCENT;
                self.tint = false;
                self.strength = 12.0;
            }
            other => {
                debug!("ignoring ui event {other:?}");
                return;
            }
        }
        self.save();
        self.apply();
        self.show();
    }
}

rekindle_sdk::plugin!(Accent);
