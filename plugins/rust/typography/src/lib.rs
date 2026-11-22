//! Typography: theming v2 beyond colors.
//!
//! Bundles two fonts inside the plugin (IBM Plex Sans and Lilex, both under
//! the SIL Open Font License; see `assets/`) and exposes a panel for the
//! interface font, the code font, chat avatar size, corner radius and density.
//! Nothing is fetched from the network.

use rekindle_sdk::prelude::*;
use rekindle_sdk::theme::Density;

const PLEX: &[u8] = include_bytes!("../assets/IBMPlexSans-Regular.ttf");
const LILEX: &[u8] = include_bytes!("../assets/Lilex-Regular.ttf");

// Family names as stored inside the font files (clients match on these).
const PLEX_FAMILY: &str = "IBM Plex Sans";
const LILEX_FAMILY: &str = "Lilex";

const PANEL: &str = "typography";
const DENSITIES: &[&str] = &["compact", "default", "cozy"];

struct Typography {
    plex: bool,
    lilex: bool,
    avatar: f64,
    radius: f64,
    density: u32,
    fonts_ok: bool,
}

impl Typography {
    fn load() -> Self {
        Self {
            plex: storage::get_or("plex", true),
            lilex: storage::get_or("lilex", true),
            avatar: storage::get_or("avatar", 40.0f64).clamp(16.0, 80.0),
            radius: storage::get_or("radius", 8.0f64).clamp(0.0, 24.0),
            density: storage::get_or("density", 1u32).min(2),
            fonts_ok: true,
        }
    }

    fn save(&self) {
        let r = [
            storage::set("plex", self.plex),
            storage::set("lilex", self.lilex),
            storage::set("avatar", self.avatar),
            storage::set("radius", self.radius),
            storage::set("density", self.density),
        ];
        if r.iter().any(Result::is_err) {
            warn!("could not save settings: {r:?}");
        }
    }

    fn apply(&self) {
        let font = |token: &str, on: bool, family: &str| {
            if on && self.fonts_ok {
                if let Err(e) = theme::set_font(token, family) {
                    warn!("{token} = {family}: {e:?}");
                }
            } else {
                theme::reset(token);
            }
        };
        font("font-primary", self.plex, PLEX_FAMILY);
        font("font-headline", self.plex, PLEX_FAMILY);
        font("font-code", self.lilex, LILEX_FAMILY);

        let mut results = vec![(
            "chat-avatar-size",
            theme::set_length("chat-avatar-size", self.avatar as f32),
        )];
        // Scale the whole radius ladder from one "medium" value.
        for (token, factor) in [
            ("radius-xs", 0.25),
            ("radius-sm", 0.5),
            ("radius-md", 1.0),
            ("radius-lg", 1.5),
            ("radius-xl", 2.0),
        ] {
            let px = (self.radius * factor).min(64.0) as f32;
            results.push((token, theme::set_radius(token, px)));
        }
        let d = [Density::Compact, Density::Default, Density::Cozy][self.density as usize];
        results.push(("density", theme::set_density(d)));
        for (token, r) in results {
            if let Err(e) = r {
                warn!("{token}: {e:?}");
            }
        }
    }

    fn show(&self) {
        let mut p = Panel::new(PANEL, "Typography");
        if !self.fonts_ok {
            p = p.text("This client couldn't load the bundled fonts; font options are off.");
        }
        p.toggle("plex", "Interface font: IBM Plex Sans", self.plex)
            .toggle("lilex", "Code font: Lilex", self.lilex)
            .separator()
            .slider(
                "avatar",
                "Chat avatar size (px)",
                16.0..=80.0,
                2.0,
                self.avatar,
            )
            .slider("radius", "Corner radius (px)", 0.0..=24.0, 1.0, self.radius)
            .select("density", "Density", DENSITIES, self.density)
            .separator()
            .button("reset", "Reset to client defaults")
            .text("Fonts: IBM Plex Sans and Lilex, SIL Open Font License 1.1.")
            .show();
    }
}

impl Plugin for Typography {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let mut t = Typography::load();
        for (family, data) in [(PLEX_FAMILY, PLEX), (LILEX_FAMILY, LILEX)] {
            if let Err(e) = theme::register_font(family, data) {
                warn!("could not register {family}: {e}");
                t.fonts_ok = false;
            }
        }
        t.apply();
        t.show();
        Ok(t)
    }

    fn on_ui(&mut self, ev: UiEvent) {
        if ev.surface != PANEL {
            return;
        }
        match (ev.element.as_str(), &ev.value) {
            ("plex", UiValue::Toggled(on)) => self.plex = *on,
            ("lilex", UiValue::Toggled(on)) => self.lilex = *on,
            ("avatar", UiValue::Number(n)) => self.avatar = n.clamp(16.0, 80.0),
            ("radius", UiValue::Number(n)) => self.radius = n.clamp(0.0, 24.0),
            ("density", UiValue::Selected(i)) if (*i as usize) < DENSITIES.len() => {
                self.density = *i;
            }
            ("reset", UiValue::Clicked) => {
                let radius = theme::group("radius");
                let tokens = [
                    "font-primary",
                    "font-headline",
                    "font-code",
                    "chat-avatar-size",
                    "density",
                ];
                for t in tokens.into_iter().chain(radius.iter().map(String::as_str)) {
                    theme::reset(t);
                }
                for k in storage::keys() {
                    storage::delete(&k);
                }
                let fonts_ok = self.fonts_ok;
                *self = Typography {
                    fonts_ok,
                    ..Typography::load()
                };
                // The fonts stay off after a restart too.
                self.plex = false;
                self.lilex = false;
                self.save();
                self.show();
                return;
            }
            _ => return,
        }
        self.save();
        self.apply();
        self.show();
    }
}

rekindle_sdk::plugin!(Typography);
