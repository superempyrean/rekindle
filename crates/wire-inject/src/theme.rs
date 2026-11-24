//! A theme's tokens, bound to the colours the client's guest names for
//! them.
//!
//! A plugin's theme names semantic tokens (`background-base-low`,
//! `text-muted`). The guest writes resolved colours on the wire, so a
//! token becomes a [`Recolor`] from the colour the client gives it to the
//! theme's. Which colour that is comes from the client itself: the guest
//! names its palette to the platform in one host call, a record of colours
//! by role (`background`, `baseLow`, `textMuted`) and whether it is dark.
//! It makes the call at start and again whenever the user's theme
//! changes, so [`ThemeBinding::palette`] reads each one off the emit hook
//! and the recolours follow the theme the client shows.
//!
//! A [`ThemeBinding`] says, for one build, which host method carries the
//! palette and which record key holds each token's colour. The record has
//! a key for a dozen of the tokens. A token it has no key for carries its
//! colour in each of the client's themes instead, recorded from the
//! guest's own cells across a theme switch. The binding names each theme
//! by the colour one record key holds in it, so the palette the guest
//! names picks the theme exactly. In a theme the binding doesn't name,
//! only the tokens with a key are recoloured.
//!
//! Each token also has a [`ColorRole`], read from its name unless the
//! binding gives one: `text-*`, `icon-*` and `interactive-*` are text and
//! icons, `border-*` borders, and every other token a fill. Its recolour
//! reaches only the cells of that role, so a light theme's white surface
//! and the white label of a brand button take different colours.
//!
//! Two tokens of one role that hold the same colour in the shown palette
//! can't be told apart on the wire. The one listed first in the binding
//! decides.

use std::collections::BTreeMap;

use rekindle_wire::frame::{CALL, Frame, HEADER_LEN};
use rekindle_wire::{PropId, Value};
use serde::Deserialize;

use crate::{ColorRole, Recolor, WorkError};

/// For one build: where the guest names its palette, and the colour each
/// token has in it.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeBinding {
    /// The host method of the call that carries the palette.
    pub method: u16,
    /// The record key of the palette's dark flag.
    pub dark: PropId,
    /// The record key whose colour names the theme.
    pub theme: PropId,
    /// Each theme the binding has colours for, by name, and its colour
    /// under [`ThemeBinding::theme`], `0xRRGGBB`.
    pub themes: BTreeMap<String, u32>,
    /// In order: where two tokens of one role share a colour, the first
    /// decides.
    pub tokens: Vec<TokenBinding>,
}

/// One token's colour in the client's palette.
#[derive(Clone, Debug, PartialEq)]
pub struct TokenBinding {
    pub token: String,
    pub role: ColorRole,
    /// The palette record's key for the token's colour.
    pub key: Option<PropId>,
    /// The token's colour, `0xRRGGBB`, in each theme, by name, when the
    /// record has no key for it.
    pub values: BTreeMap<String, u32>,
}

/// The palette the guest named: its colours by record key, `0xAARRGGBB`,
/// and whether it is dark.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Palette {
    pub dark: Option<bool>,
    pub colors: BTreeMap<PropId, u32>,
}

/// A token's colour in the shown palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bound {
    pub token: String,
    pub role: ColorRole,
    /// `0xRRGGBB`.
    pub value: u32,
    /// Read from the guest's palette, rather than the binding's table.
    pub live: bool,
}

impl Palette {
    /// The palette in a call's record: every integer field is a colour, and
    /// `dark` the bool under that key.
    pub fn from_record(record: &Value, dark: PropId) -> Option<Palette> {
        let Value::Object(fields) = record else {
            return None;
        };
        let mut palette = Palette::default();
        for (k, v) in fields {
            if *k == dark {
                palette.dark = match v {
                    Value::Bool(b) => Some(*b),
                    _ => None,
                };
            } else if let Some(c) = v.as_color() {
                palette.colors.insert(*k, c);
            }
        }
        Some(palette)
    }

    /// Whether the palette is dark: its flag, or dark when it has none.
    pub fn is_dark(&self) -> bool {
        self.dark.unwrap_or(true)
    }
}

/// `ColorRole` of a token, from its name.
pub fn role_of(token: &str) -> ColorRole {
    if ["text-", "icon-", "interactive-"]
        .iter()
        .any(|p| token.starts_with(p))
    {
        ColorRole::Content
    } else if token.starts_with("border-") {
        ColorRole::Border
    } else {
        ColorRole::Fill
    }
}

/// `#rrggbb` as `0xRRGGBB`.
fn hex(s: &str) -> Option<u32> {
    let h = s.strip_prefix('#')?;
    (h.len() == 6).then(|| u32::from_str_radix(h, 16).ok())?
}

impl ThemeBinding {
    /// Read a binding. The JSON is
    ///
    /// ```json
    /// {"call": {"method": 84, "dark": 583, "theme": 587},
    ///  "themes": {"dark": "#2c2d32", "light": "#f3f3f4"},
    ///  "tokens": [{"token": "background-base-lowest", "key": 587},
    ///             {"token": "text-subtle", "values": {"dark": "#c5c6ca", "light": "#595a63"}},
    ///             {"token": "status-danger", "key": 590, "role": "any"}]}
    /// ```
    ///
    /// with other top-level keys ignored.
    pub fn from_json(json: &str) -> Result<ThemeBinding, WorkError> {
        #[derive(Deserialize)]
        struct File {
            call: Call,
            #[serde(default)]
            themes: BTreeMap<String, String>,
            tokens: Vec<Entry>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Call {
            method: u16,
            dark: u16,
            theme: u16,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Entry {
            token: String,
            #[serde(default)]
            role: Option<ColorRole>,
            #[serde(default)]
            key: Option<u16>,
            #[serde(default)]
            values: BTreeMap<String, String>,
        }
        let err = |m: String| WorkError(format!("theme binding: {m}"));
        let file: File = serde_json::from_str(json).map_err(|e| err(e.to_string()))?;
        let colour = |s: &str| hex(s).ok_or_else(|| err(format!("{s:?} is not #rrggbb")));
        let themes = file
            .themes
            .iter()
            .map(|(name, c)| Ok((name.clone(), colour(c)?)))
            .collect::<Result<BTreeMap<_, _>, WorkError>>()?;
        let mut tokens: Vec<TokenBinding> = Vec::new();
        for t in file.tokens {
            if t.key.is_none() && t.values.is_empty() {
                return Err(err(format!("{} has no key and no colour", t.token)));
            }
            if tokens.iter().any(|b| b.token == t.token) {
                return Err(err(format!("{} is listed twice", t.token)));
            }
            let mut values = BTreeMap::new();
            for (theme, c) in &t.values {
                if !themes.contains_key(theme) {
                    return Err(err(format!(
                        "{} names theme {theme:?}, not in themes",
                        t.token
                    )));
                }
                values.insert(theme.clone(), colour(c)?);
            }
            tokens.push(TokenBinding {
                role: t.role.unwrap_or_else(|| role_of(&t.token)),
                key: t.key.map(PropId),
                values,
                token: t.token,
            });
        }
        Ok(ThemeBinding {
            method: file.call.method,
            dark: PropId(file.call.dark),
            theme: PropId(file.call.theme),
            themes,
            tokens,
        })
    }

    /// The tokens plugins can set.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tokens.iter().map(|t| t.token.as_str())
    }

    /// The palette in `buf`, if it is the guest's call naming one. Only the
    /// header is read for any other buffer.
    pub fn palette(&self, buf: &[u8]) -> Option<Palette> {
        if buf.len() < HEADER_LEN
            || buf[0] != CALL
            || u16::from_le_bytes([buf[4], buf[5]]) != self.method
        {
            return None;
        }
        let frame = Frame::from_bytes(buf).ok()?;
        Palette::from_record(&frame.record_value().ok()?, self.dark)
    }

    /// The theme `palette` is, by the colour under
    /// [`ThemeBinding::theme`], if the binding names it.
    pub fn theme_of(&self, palette: &Palette) -> Option<&str> {
        let c = palette.colors.get(&self.theme)? & 0x00ff_ffff;
        self.themes
            .iter()
            .find(|(_, v)| **v == c)
            .map(|(name, _)| name.as_str())
    }

    /// Each token's colour in `palette`: the record's colour for its key,
    /// else the binding's for the palette's theme. A token with neither is
    /// left out, and so is every token before the guest names a palette.
    pub fn values(&self, palette: Option<&Palette>) -> Vec<Bound> {
        let Some(palette) = palette else {
            return Vec::new();
        };
        let theme = self.theme_of(palette);
        self.tokens
            .iter()
            .filter_map(|t| {
                let live = t
                    .key
                    .and_then(|k| palette.colors.get(&k))
                    .map(|c| c & 0x00ff_ffff);
                let table = theme.and_then(|name| t.values.get(name)).copied();
                Some(Bound {
                    token: t.token.clone(),
                    role: t.role,
                    value: live.or(table)?,
                    live: live.is_some(),
                })
            })
            .collect()
    }

    /// One recolour for each token `overrides` (token to `0xRRGGBBAA`) sets
    /// to a colour other than its own in `palette`.
    pub fn recolors(
        &self,
        palette: Option<&Palette>,
        overrides: &BTreeMap<String, u32>,
    ) -> Vec<Recolor> {
        let mut out: Vec<Recolor> = Vec::new();
        for b in self.values(palette) {
            let Some(&to) = overrides.get(&b.token) else {
                continue;
            };
            // The first token of a role with this colour decides, even when
            // it keeps the colour.
            if out.iter().any(|r| r.from == b.value && r.role == b.role) {
                continue;
            }
            out.push(Recolor {
                from: b.value,
                to,
                role: b.role,
            });
        }
        out.retain(|r| r.to != (r.from << 8 | 0xff));
        out
    }
}

/// What a theme's recolours reached in an [`Engine`](crate::Engine)'s
/// trees.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reach {
    /// Colour cells recoloured, by the token whose recolour applied.
    pub by_token: BTreeMap<String, usize>,
    /// Colour cells recoloured, by the cell's role.
    pub by_role: BTreeMap<ColorRole, usize>,
    /// Text spans whose colour changed.
    pub spans: usize,
    /// Text and icon cells whose colour is a fill or border token's.
    pub text_with_fill_colour: usize,
    /// Of those, the ones that took that token's colour. A recolour by
    /// value alone gives each of them the fill's colour.
    pub text_recoloured_as_fill: usize,
}

/// What `recolors` reached in the engine's trees: every colour cell of
/// the guest's nodes, compared with the host's tree. `values` names the
/// token behind each recolour. `recolors` are usually the engine's own,
/// but can be another set's, to check what an engine running the same
/// colours without roles did to the cells those roles keep apart.
pub fn reach(engine: &crate::Engine, recolors: &[Recolor], values: &[Bound]) -> Reach {
    let applied = |rgb: u32, role: ColorRole| {
        let find = |r: ColorRole| recolors.iter().rev().find(|x| x.from == rgb && x.role == r);
        find(role).or_else(|| find(ColorRole::Any))
    };
    let token = |r: &Recolor| {
        values
            .iter()
            .find(|v| v.value == r.from && (v.role == r.role || r.role == ColorRole::Any))
            .map_or_else(|| format!("#{:06x}", r.from), |v| v.token.clone())
    };
    let fill = |rgb: u32| {
        recolors
            .iter()
            .find(|r| r.from == rgb && matches!(r.role, ColorRole::Fill | ColorRole::Border))
    };
    let mut reach = Reach::default();
    let text = |reach: &mut Reach, from: u32, to: u32| {
        if let Some(r) = fill(from & 0x00ff_ffff) {
            reach.text_with_fill_colour += 1;
            if to & 0x00ff_ffff == r.to >> 8 {
                reach.text_recoloured_as_fill += 1;
            }
        }
    };
    let (guest, host) = (engine.guest(), engine.mirror());
    for n in guest.nodes() {
        for p in crate::COLOR_PROPS {
            let Some(role) = ColorRole::of(*p) else {
                continue;
            };
            let (Some(from), Some(to)) = (
                guest.props().get(n.id, *p).and_then(Value::as_color),
                host.props().get(n.id, *p).and_then(Value::as_color),
            ) else {
                continue;
            };
            if from != to
                && let Some(r) = applied(from & 0x00ff_ffff, role)
            {
                *reach.by_role.entry(role).or_default() += 1;
                *reach.by_token.entry(token(r)).or_default() += 1;
            }
            if role == ColorRole::Content {
                text(&mut reach, from, to);
            }
        }
        let spans = |t: &rekindle_wire::Tree| match t.props().get(n.id, rekindle_wire::turn::SPANS)
        {
            Some(Value::Array(spans)) => spans
                .iter()
                .map(|s| match s {
                    Value::Object(f) => f
                        .iter()
                        .find(|(p, _)| *p == rekindle_wire::ids::COLOR)
                        .and_then(|(_, v)| v.as_color()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        for (from, to) in spans(guest).into_iter().zip(spans(host)) {
            if let (Some(from), Some(to)) = (from, to) {
                if from != to {
                    reach.spans += 1;
                }
                text(&mut reach, from, to);
            }
        }
    }
    reach
}

impl std::fmt::Display for Reach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let roles: Vec<String> = self
            .by_role
            .iter()
            .map(|(r, n)| format!("{r:?}:{n}"))
            .collect();
        let tokens: Vec<String> = self
            .by_token
            .iter()
            .map(|(t, n)| format!("{t}:{n}"))
            .collect();
        write!(
            f,
            "cells_by_role=[{}] cells_by_token=[{}] spans={} \
             text_cells_with_a_fill_colour={} of_them_recoloured_as_fill={}",
            roles.join(" "),
            tokens.join(" "),
            self.spans,
            self.text_with_fill_colour,
            self.text_recoloured_as_fill,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARK: PropId = PropId(583);
    const BACKGROUND: PropId = PropId(587);
    const TEXT: PropId = PropId(472);

    fn binding() -> ThemeBinding {
        ThemeBinding::from_json(
            r##"{"_note": "x", "call": {"method": 84, "dark": 583, "theme": 587},
                "themes": {"dark": "#2c2d32", "light": "#f3f3f4"},
                "tokens": [
                {"token": "background-base-lowest", "key": 587},
                {"token": "background-base-lower", "key": 587},
                {"token": "text-strong", "key": 472},
                {"token": "text-subtle", "values": {"dark": "#c5c6ca", "light": "#595a63"}},
                {"token": "border-subtle", "values": {"dark": "#2e2e33"}},
                {"token": "background-brand", "key": 596, "role": "any"}
            ]}"##,
        )
        .unwrap()
    }

    fn palette(dark: bool, background: u32, text: u32) -> Palette {
        Palette::from_record(
            &Value::Object(vec![
                (DARK, Value::Bool(dark)),
                (BACKGROUND, Value::color(background)),
                (TEXT, Value::color(text)),
                (PropId(596), Value::color(0xff58_65f2)),
            ]),
            DARK,
        )
        .unwrap()
    }

    #[test]
    fn roles_come_from_the_token_name_unless_given() {
        let b = binding();
        let role = |t: &str| b.tokens.iter().find(|x| x.token == t).unwrap().role;
        assert_eq!(role("background-base-lowest"), ColorRole::Fill);
        assert_eq!(role("text-strong"), ColorRole::Content);
        assert_eq!(role("border-subtle"), ColorRole::Border);
        assert_eq!(role("background-brand"), ColorRole::Any);
        assert_eq!(role_of("icon-muted"), ColorRole::Content);
        assert_eq!(role_of("interactive-text-hover"), ColorRole::Content);
        assert_eq!(role_of("input-background-default"), ColorRole::Fill);
    }

    #[test]
    fn a_binding_refuses_a_token_with_no_colour_listed_twice_or_in_no_theme() {
        let json = |tokens: &str| {
            format!(
                r##"{{"call": {{"method": 84, "dark": 583, "theme": 587}}, "themes": {{"dark": "#2c2d32"}}, "tokens": [{tokens}]}}"##
            )
        };
        assert!(ThemeBinding::from_json(&json(r#"{"token": "text-muted", "key": 599}"#)).is_ok());
        assert!(ThemeBinding::from_json(&json(r#"{"token": "text-muted"}"#)).is_err());
        assert!(
            ThemeBinding::from_json(&json(
                r#"{"token": "text-muted", "key": 599}, {"token": "text-muted", "key": 599}"#
            ))
            .is_err()
        );
        assert!(
            ThemeBinding::from_json(&json(
                r#"{"token": "text-muted", "values": {"dark": "abc"}}"#
            ))
            .is_err()
        );
        assert!(
            ThemeBinding::from_json(&json(
                r##"{"token": "text-muted", "values": {"ash": "#abacb2"}}"##
            ))
            .is_err()
        );
    }

    #[test]
    fn the_palette_is_read_from_the_guests_call_only() {
        let b = binding();
        let p = palette(false, 0xfff3_f3f4, 0xff28_282d);
        let record = Value::Object(vec![
            (DARK, Value::Bool(false)),
            (BACKGROUND, Value::color(0xfff3_f3f4)),
            (TEXT, Value::color(0xff28_282d)),
            (PropId(596), Value::color(0xff58_65f2)),
        ]);
        let call = Frame::call(84, 84, 7, &record).to_bytes();
        assert_eq!(b.palette(&call), Some(p));
        assert_eq!(
            b.palette(&Frame::call(214, 214, 8, &record).to_bytes()),
            None
        );
        assert_eq!(b.palette(&[0, 0, 0, 0, 1]), None);
    }

    #[test]
    fn values_follow_the_palette_and_its_theme() {
        let b = binding();
        let value = |p: Option<&Palette>, t: &str| {
            b.values(p)
                .into_iter()
                .find(|v| v.token == t)
                .map(|v| (v.value, v.live))
        };
        let dark = palette(true, 0xff2c_2d32, 0xffff_ffff);
        let light = palette(false, 0xfff3_f3f4, 0xff28_282d);
        // A theme the binding doesn't name: keys only.
        let darker = palette(true, 0xff12_1214, 0xfffb_fbfb);
        assert_eq!(b.theme_of(&dark), Some("dark"));
        assert_eq!(b.theme_of(&darker), None);
        assert_eq!(
            value(Some(&dark), "background-base-lower"),
            Some((0x2c_2d32, true))
        );
        assert_eq!(
            value(Some(&light), "background-base-lower"),
            Some((0xf3_f3f4, true))
        );
        assert_eq!(value(Some(&dark), "text-subtle"), Some((0xc5_c6ca, false)));
        assert_eq!(value(Some(&light), "text-subtle"), Some((0x59_5a63, false)));
        assert_eq!(value(Some(&darker), "text-subtle"), None);
        assert_eq!(value(Some(&darker), "text-strong"), Some((0xfb_fbfb, true)));
        assert_eq!(value(Some(&light), "border-subtle"), None);
        // Before the guest names a palette, nothing.
        assert!(b.values(None).is_empty());
    }

    #[test]
    fn recolours_carry_the_tokens_role_and_the_first_token_decides() {
        let b = binding();
        let light = palette(false, 0xfff3_f3f4, 0xff28_282d);
        let overrides: BTreeMap<String, u32> = [
            ("background-base-lowest", 0x0506_09ff),
            ("background-base-lower", 0x080a_0eff),
            ("text-strong", 0xf2f3_f5ff),
            ("text-subtle", 0xb5ba_c1ff),
            ("background-brand", 0x5865_f2ff),
        ]
        .into_iter()
        .map(|(t, c)| (t.to_string(), c))
        .collect();
        assert_eq!(
            b.recolors(Some(&light), &overrides),
            vec![
                Recolor {
                    from: 0xf3_f3f4,
                    to: 0x0506_09ff,
                    role: ColorRole::Fill
                },
                Recolor {
                    from: 0x28_282d,
                    to: 0xf2f3_f5ff,
                    role: ColorRole::Content
                },
                Recolor {
                    from: 0x59_5a63,
                    to: 0xb5ba_c1ff,
                    role: ColorRole::Content
                },
            ]
        );
    }
}
