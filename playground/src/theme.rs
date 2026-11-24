//! A theme as the host applies it: a `.theme.json`'s tokens, bound to the
//! client's palette by the build's [`ThemeBinding`], made into the
//! engine's recolours by [`ThemeBinding::recolors`].
//!
//! The page sends one JSON value:
//!
//! ```json
//! {"binding": {"call": {"method": 84, "dark": 583}, "tokens": [...]},
//!  "palette": {"dark": true, "colors": {"587": "#121214"}},
//!  "tokens": {"background-base-low": "#0b0d12", "text-muted": "#949ba4cc"}}
//! ```
//!
//! `binding` is the build's binding, in [`ThemeBinding::from_json`]'s
//! form. `palette` is the record the guest named for the screen, its
//! colours by record key, or null for none. `tokens` are the theme's, the
//! top level with the chosen variant over it. The reply is each bound
//! token's colour in the palette, which the page shows as the token's
//! default, and the recolours made.

use std::collections::BTreeMap;

use rekindle_wire::PropId;
use rekindle_wire_inject::theme::{Bound, Palette, ThemeBinding};
use rekindle_wire_inject::{ColorRole, Recolor, Work};
use serde_json::{Value as Json, json};

/// A parsed theme request: see the module docs.
pub struct Theme {
    pub binding: ThemeBinding,
    pub palette: Option<Palette>,
    /// Token to `0xRRGGBBAA`.
    pub overrides: BTreeMap<String, u32>,
}

/// `#rrggbb` as `0xRRGGBBAA` with full alpha, or `#rrggbbaa`.
pub fn rgba(s: &str) -> Option<u32> {
    let h = s.strip_prefix('#').unwrap_or(s);
    let v = u32::from_str_radix(h, 16).ok()?;
    match h.len() {
        6 => Some((v << 8) | 0xff),
        8 => Some(v),
        _ => None,
    }
}

impl Theme {
    pub fn from_json(json: &str) -> Result<Theme, String> {
        let v: Json = serde_json::from_str(json).map_err(|e| format!("theme: {e}"))?;
        let binding = ThemeBinding::from_json(&v["binding"].to_string()).map_err(|e| e.0)?;
        let palette = match &v["palette"] {
            Json::Null => None,
            p => {
                let mut palette = Palette {
                    dark: p["dark"].as_bool(),
                    ..Palette::default()
                };
                for (k, c) in p["colors"].as_object().into_iter().flatten() {
                    let key = k.parse::<u16>().map_err(|_| format!("palette key {k:?}"))?;
                    let c = c
                        .as_str()
                        .and_then(rgba)
                        .ok_or_else(|| format!("palette colour {c}"))?;
                    palette.colors.insert(PropId(key), 0xff00_0000 | (c >> 8));
                }
                Some(palette)
            }
        };
        let mut overrides = BTreeMap::new();
        for (t, c) in v["tokens"].as_object().into_iter().flatten() {
            // A token that isn't a colour (a length, a font) doesn't recolour.
            if let Some(c) = c.as_str().and_then(rgba) {
                overrides.insert(t.clone(), c);
            }
        }
        Ok(Theme {
            binding,
            palette,
            overrides,
        })
    }

    pub fn values(&self) -> Vec<Bound> {
        self.binding.values(self.palette.as_ref())
    }

    pub fn recolors(&self) -> Vec<Recolor> {
        self.binding
            .recolors(self.palette.as_ref(), &self.overrides)
    }

    /// The engine's work for this theme.
    pub fn work(&self) -> Work {
        Work {
            recolors: self.recolors(),
            ..Work::default()
        }
    }

    /// What the page shows: each bound token's colour and role, and the
    /// recolours.
    pub fn report(&self) -> Json {
        let role = |r: ColorRole| match r {
            ColorRole::Any => "any",
            ColorRole::Fill => "fill",
            ColorRole::Border => "border",
            ColorRole::Content => "content",
        };
        let values: Vec<Json> = self
            .values()
            .iter()
            .map(|b| {
                json!({
                    "token": b.token,
                    "role": role(b.role),
                    "value": format!("#{:06x}", b.value),
                    "live": b.live,
                })
            })
            .collect();
        let recolors: Vec<Json> = self
            .recolors()
            .iter()
            .map(|r| {
                json!({
                    "from": format!("#{:06x}", r.from),
                    "to": format!("#{:08x}", r.to),
                    "role": role(r.role),
                })
            })
            .collect();
        json!({ "values": values, "recolors": recolors })
    }
}
