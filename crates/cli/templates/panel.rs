use rekindle_sdk::prelude::*;

struct Settings {
    accent: Color,
    enabled: bool,
}

impl Settings {
    fn apply(&self) {
        if self.enabled {
            theme::set("background-brand", self.accent);
        } else {
            theme::reset("background-brand");
        }
    }

    fn show(&self) {
        Panel::new("settings", "{{TITLE}}")
            .text("Pick an accent color for the client.")
            .toggle("enabled", "Enabled", self.enabled)
            .color_picker("accent", "Accent", self.accent)
            .show();
    }
}

impl Plugin for Settings {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let s = Settings {
            accent: storage::get("accent").and_then(|h| Color::from_hex(&h)).unwrap_or(Color::rgb(0xeb, 0x45, 0x9e)),
            enabled: storage::get_or("enabled", true),
        };
        s.apply();
        s.show();
        Ok(s)
    }

    fn on_ui(&mut self, ev: UiEvent) {
        if let Some(on) = ev.toggled("settings", "enabled") {
            self.enabled = on;
            let _ = storage::set("enabled", on);
        }
        if let Some(c) = ev.color("settings", "accent") {
            self.accent = c;
            let _ = storage::set("accent", c.to_hex());
        }
        self.apply();
        self.show();
    }
}

rekindle_sdk::plugin!(Settings);
