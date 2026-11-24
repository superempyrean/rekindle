//! rekindle's wire engine between the guest and the renderer.
//!
//! A theme file names Discord's colour tokens. The guest names its palette
//! once, in its `AppState.setTheme` call, as keys like `background` and
//! `brand`. [`TOKENS`] pairs the two, and each token the theme sets becomes a
//! [`Recolor`] from the guest's colour to the theme's. The pairing is this
//! platform's reading of the welcome and login screens, not a recovered
//! table.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use rekindle_wire::{PropId, Value};
use rekindle_wire_inject::{Recolor, Work};

/// Theme token, and the `AppState.setTheme` record key (by id) whose colour
/// it replaces.
const TOKENS: &[(&str, u16, &str)] = &[
    ("background-base-lowest", 587, "background"),
    ("background-base-lower", 587, "background"),
    ("background-base-low", 603, "baseLow"),
    ("background-surface-high", 588, "surface"),
    ("background-surface-higher", 594, "raised"),
    ("background-brand", 596, "brand"),
    ("text-strong", 472, "text"),
    ("text-default", 602, "textDefault"),
    ("text-muted", 599, "textMuted"),
];

pub enum ThemeSource {
    /// A `.theme.json`, with its tokens merged with the chosen variant's.
    Tokens(Vec<(String, u32)>),
    /// A `Work` given as JSON, used as it is.
    Work(Work),
}

fn hex(s: &str) -> Option<u32> {
    let h = s.strip_prefix('#')?;
    match h.len() {
        6 => u32::from_str_radix(h, 16).ok().map(|v| (v << 8) | 0xff),
        8 => u32::from_str_radix(h, 16).ok(),
        _ => None,
    }
}

impl ThemeSource {
    pub fn theme_file(path: &Path, variant: Option<&str>) -> Result<ThemeSource> {
        let json: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?,
        )?;
        let mut tokens: Vec<(String, u32)> = Vec::new();
        let mut add = |obj: Option<&serde_json::Value>| {
            for (k, v) in obj.and_then(|o| o.as_object()).into_iter().flatten() {
                if let Some(c) = v.as_str().and_then(hex) {
                    tokens.retain(|(t, _)| t != k);
                    tokens.push((k.clone(), c));
                }
            }
        };
        add(json.get("tokens"));
        let want = variant.map(str::to_string).or_else(|| {
            json.get("default-variant")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
        if let Some(want) = want {
            let found = json
                .get("variants")
                .and_then(|v| v.as_array())
                .and_then(|vs| {
                    vs.iter()
                        .find(|v| v.get("name").and_then(|n| n.as_str()) == Some(&want))
                });
            match found {
                Some(v) => add(v.get("tokens")),
                None if variant.is_some() => bail!("{} has no variant {want}", path.display()),
                None => {}
            }
        }
        Ok(ThemeSource::Tokens(tokens))
    }

    pub fn work_file(path: &Path) -> Result<ThemeSource> {
        let json =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Ok(ThemeSource::Work(
            Work::from_json(&json).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?,
        ))
    }

    /// The work for the guest's palette (the `AppState.setTheme` record).
    pub fn work(&self, palette: &Value) -> Result<Work> {
        let tokens = match self {
            ThemeSource::Work(w) => return Ok(w.clone()),
            ThemeSource::Tokens(t) => t,
        };
        let Value::Object(fields) = palette else {
            bail!("the palette is not a record")
        };
        let mut recolors: Vec<Recolor> = Vec::new();
        for (token, key, _) in TOKENS {
            let Some((_, to)) = tokens.iter().find(|(t, _)| t == token) else {
                continue;
            };
            let Some(from) = fields
                .iter()
                .find(|(k, _)| *k == PropId(*key))
                .and_then(|(_, v)| v.as_color())
            else {
                continue;
            };
            let from = from & 0x00ff_ffff;
            if recolors.iter().all(|r| r.from != from) {
                recolors.push(Recolor { from, to: *to });
            }
        }
        if recolors.is_empty() {
            bail!("the theme sets none of the tokens the guest's palette has");
        }
        Ok(Work {
            recolors,
            ..Work::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_recolours_the_guests_colour_for_its_key() {
        let palette = Value::Object(vec![
            (PropId(587), Value::color(0xff12_1214)),
            (PropId(596), Value::color(0xff58_65f2)),
        ]);
        let theme = ThemeSource::Tokens(vec![
            ("background-brand".into(), hex("#6d8cff").unwrap()),
            ("background-base-lower".into(), hex("#080a0e").unwrap()),
            ("background-base-lowest".into(), hex("#050609").unwrap()),
        ]);
        let work = theme.work(&palette).unwrap();
        // The first token for a key decides.
        assert_eq!(
            work.recolors,
            vec![
                Recolor {
                    from: 0x12_1214,
                    to: 0x0506_09ff
                },
                Recolor {
                    from: 0x58_65f2,
                    to: 0x6d8c_ffff
                },
            ]
        );
        assert!(ThemeSource::Tokens(vec![]).work(&palette).is_err());
    }
}
