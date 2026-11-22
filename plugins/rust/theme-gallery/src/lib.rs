//! Theme Gallery: a network plugin.
//!
//! Pick a seed color and a scheme mode; the plugin asks The Color API
//! (www.thecolorapi.com, its only declared host) for a 5-color scheme and maps
//! it onto the client's theme tokens. The last generated palette is saved, so
//! it is re-applied at startup without a network call.

use rekindle_sdk::prelude::*;
use serde::Deserialize;

const PANEL: &str = "gallery";
const MODES: &[&str] = &["analogic", "monochrome-dark", "complement", "triad", "quad"];

#[derive(Deserialize)]
struct Scheme {
    colors: Vec<SchemeColor>,
}

#[derive(Deserialize)]
struct SchemeColor {
    hex: Hex,
}

#[derive(Deserialize)]
struct Hex {
    value: String,
}

struct Gallery {
    seed: Color,
    mode: u32,
    palette: Vec<Color>,
    status: String,
}

fn url(seed: Color, mode: &str) -> String {
    let hex = seed.to_hex();
    format!(
        "https://www.thecolorapi.com/scheme?hex={}&mode={mode}&count=5&format=json",
        &hex[1..7]
    )
}

/// The tokens a palette sets, in the order [`apply`] sets them.
const TOKENS: [&str; 7] = [
    "background-brand",
    "text-link",
    "background-base-low",
    "background-base-lower",
    "background-base-lowest",
    "background-surface-higher",
    "input-background-default",
];

/// Map a 5-color scheme onto [`TOKENS`]: the darkest three become
/// backgrounds, the most saturated-looking (first) becomes the accent.
fn apply(palette: &[Color]) {
    let [a, b, c, d, e] = palette else { return };
    let dark = |col: &Color, t: f32| col.mix(Color::rgb(0, 0, 0), t);
    let colors = [
        *a,
        *b,
        dark(c, 0.78),
        dark(d, 0.82),
        dark(e, 0.86),
        dark(e, 0.9),
        dark(c, 0.7),
    ];
    theme::set_many(TOKENS.into_iter().zip(colors));
}

fn save(key: &str, value: impl ToString) {
    if let Err(e) = storage::set(key, value) {
        warn!("couldn't save {key}: {e:?}");
    }
}

impl Gallery {
    fn show(&self) {
        let mut p = Panel::new(PANEL, "Theme Gallery")
            .color_picker("seed", "Seed color", self.seed)
            .select("mode", "Scheme", MODES, self.mode)
            .button("generate", "Generate palette")
            .text(&self.status);
        if !self.palette.is_empty() {
            let hexes: Vec<String> = self.palette.iter().map(|c| c.to_hex()).collect();
            p = p
                .text(&format!("Current: {}", hexes.join("  ")))
                .button("clear", "Remove palette");
        }
        p.show();
    }

    fn generate(&mut self) {
        let mode = MODES[self.mode as usize % MODES.len()];
        let started = now_millis();
        let palette = match http::get(&url(self.seed, mode)) {
            Ok(r) if r.ok() => match serde_json::from_slice::<Scheme>(r.bytes()) {
                Ok(s) => s
                    .colors
                    .iter()
                    .filter_map(|c| Color::from_hex(&c.hex.value))
                    .collect::<Vec<_>>(),
                Err(e) => {
                    self.status = format!("Unexpected response: {e}");
                    return;
                }
            },
            Ok(r) => {
                self.status = format!("The Color API returned HTTP {}", r.status());
                return;
            }
            Err(e) => {
                self.status = format!("Request failed: {e:?}");
                return;
            }
        };
        if palette.len() != 5 {
            self.status = format!("Expected 5 colors, got {}", palette.len());
            return;
        }
        apply(&palette);
        let hexes: Vec<String> = palette.iter().map(|c| c.to_hex()).collect();
        save("palette", hexes.join(","));
        self.palette = palette;
        self.status = format!(
            "Generated a {mode} palette in {} ms.",
            now_millis() - started
        );
        info!("{}", self.status);
    }
}

impl Plugin for Gallery {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let palette: Vec<Color> = storage::get("palette")
            .map(|s| s.split(',').filter_map(Color::from_hex).collect())
            .unwrap_or_default();
        apply(&palette);
        let g = Gallery {
            seed: storage::get("seed")
                .and_then(|h| Color::from_hex(&h))
                .unwrap_or(Color::rgb(0x58, 0x65, 0xf2)),
            mode: storage::get_or("mode", 0),
            status: if palette.is_empty() {
                "No palette yet.".into()
            } else {
                "Restored saved palette.".into()
            },
            palette,
        };
        g.show();
        Ok(g)
    }

    fn on_ui(&mut self, ev: UiEvent) {
        if ev.surface != PANEL {
            return;
        }
        match (ev.element.as_str(), &ev.value) {
            ("seed", UiValue::Color(c)) => {
                self.seed = *c;
                save("seed", c.to_hex());
            }
            ("mode", UiValue::Selected(i)) if (*i as usize) < MODES.len() => {
                self.mode = *i;
                save("mode", i);
            }
            ("generate", UiValue::Clicked) => self.generate(),
            ("clear", UiValue::Clicked) => {
                for t in TOKENS {
                    theme::reset(t);
                }
                storage::delete("palette");
                self.palette.clear();
                self.status = "Palette removed.".into();
            }
            _ => return,
        }
        self.show();
    }
}

rekindle_sdk::plugin!(Gallery);
