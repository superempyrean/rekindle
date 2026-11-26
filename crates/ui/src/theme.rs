//! Theme tokens and their values.
//!
//! A token is a canonical name plus a value. The same names travel to both
//! backends: the wire backend turns a colour into an `0xAARRGGBB` integer on
//! a cell, the GPUI backend turns it into the `background`/`color` fields of
//! a `StyleRefinement`. A [`Theme`] is the resolved values a plugin sees.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Straight sRGB, alpha last. `0` is transparent, `255` opaque.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    #[must_use]
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// `0xRRGGBB`, opaque.
    #[must_use]
    pub const fn hex(rgb: u32) -> Self {
        Self {
            r: (rgb >> 16) as u8,
            g: (rgb >> 8) as u8,
            b: rgb as u8,
            a: 255,
        }
    }

    /// `0xRRGGBBAA`.
    #[must_use]
    pub const fn hex_alpha(rgba: u32) -> Self {
        Self {
            r: (rgba >> 24) as u8,
            g: (rgba >> 16) as u8,
            b: (rgba >> 8) as u8,
            a: rgba as u8,
        }
    }

    /// The wire's colour value, `0xAARRGGBB`.
    #[must_use]
    pub const fn argb(self) -> u32 {
        (self.a as u32) << 24 | (self.r as u32) << 16 | (self.g as u32) << 8 | self.b as u32
    }

    /// `#rrggbb` or `#rrggbbaa`, lower case.
    #[must_use]
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }
}

/// A canonical theme token name.
///
/// The names are the host's (`rekindle_plugin_host::tokens`); this crate does
/// not keep its own list, so a token a host does not know still round-trips
/// as a name and resolves to the caller's fallback.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Token(String);

impl Token {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub const BACKGROUND_BASE_LOW: &'static str = "background-base-low";
    pub const BACKGROUND_BASE_LOWER: &'static str = "background-base-lower";
    pub const BACKGROUND_SURFACE_HIGH: &'static str = "background-surface-high";
    pub const BACKGROUND_SURFACE_HIGHER: &'static str = "background-surface-higher";
    pub const BACKGROUND_MOD_SUBTLE: &'static str = "background-mod-subtle";
    pub const BACKGROUND_MOD_NORMAL: &'static str = "background-mod-normal";
    pub const BACKGROUND_BRAND: &'static str = "background-brand";
    pub const TEXT_DEFAULT: &'static str = "text-default";
    pub const TEXT_STRONG: &'static str = "text-strong";
    pub const TEXT_SUBTLE: &'static str = "text-subtle";
    pub const TEXT_MUTED: &'static str = "text-muted";
    pub const ICON_DEFAULT: &'static str = "icon-default";
    pub const ICON_MUTED: &'static str = "icon-muted";
    pub const BORDER_SUBTLE: &'static str = "border-subtle";
    pub const BORDER_NORMAL: &'static str = "border-normal";
    pub const INTERACTIVE_TEXT_DEFAULT: &'static str = "interactive-text-default";
    pub const CHANNELTEXTAREA_BACKGROUND: &'static str = "channeltextarea-background";
    pub const INPUT_BACKGROUND_DEFAULT: &'static str = "input-background-default";
}

impl From<&str> for Token {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for Token {
    fn from(value: String) -> Self {
        Self(value)
    }
}

/// A colour a style asks for: a token, or an explicit RGBA.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum Brush {
    Token(Token),
    Color(Color),
}

impl Brush {
    #[must_use]
    pub fn token(name: impl Into<String>) -> Self {
        Self::Token(Token::new(name))
    }

    #[must_use]
    pub fn color(color: Color) -> Self {
        Self::Color(color)
    }
}

impl From<Color> for Brush {
    fn from(value: Color) -> Self {
        Self::Color(value)
    }
}

impl From<&str> for Brush {
    fn from(value: &str) -> Self {
        Self::Token(Token::new(value))
    }
}

impl From<String> for Brush {
    fn from(value: String) -> Self {
        Self::Token(Token::new(value))
    }
}

impl From<Token> for Brush {
    fn from(value: Token) -> Self {
        Self::Token(value)
    }
}

/// A resolved token table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Tokens(BTreeMap<String, Color>);

impl Tokens {
    /// An empty table. [`Tokens::resolve`] falls back for every token.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The neutral dark defaults the host ships, enough for a plugin to look
    /// finished before a theme sets anything.
    #[must_use]
    pub fn defaults() -> Self {
        let mut t = Self::new();
        t.set(Token::BACKGROUND_BASE_LOWER, Color::hex(0x18181b));
        t.set(Token::BACKGROUND_BASE_LOW, Color::hex(0x1d1d21));
        t.set(Token::BACKGROUND_SURFACE_HIGH, Color::hex(0x232328));
        t.set(Token::BACKGROUND_SURFACE_HIGHER, Color::hex(0x28282d));
        t.set(Token::BACKGROUND_MOD_SUBTLE, Color::hex_alpha(0xffffff1f));
        t.set(Token::BACKGROUND_MOD_NORMAL, Color::hex_alpha(0xffffff29));
        t.set(Token::BACKGROUND_BRAND, Color::hex(0x5865f2));
        t.set(Token::TEXT_DEFAULT, Color::hex(0xdbdee1));
        t.set(Token::TEXT_STRONG, Color::hex(0xf2f3f5));
        t.set(Token::TEXT_SUBTLE, Color::hex(0xb5bac1));
        t.set(Token::TEXT_MUTED, Color::hex(0x949ba4));
        t.set(Token::ICON_DEFAULT, Color::hex(0xdbdee1));
        t.set(Token::ICON_MUTED, Color::hex(0x80848e));
        t.set(Token::BORDER_SUBTLE, Color::hex_alpha(0xffffff14));
        t.set(Token::BORDER_NORMAL, Color::hex_alpha(0xffffff1f));
        t.set(Token::INTERACTIVE_TEXT_DEFAULT, Color::hex(0xb5bac1));
        t.set(Token::CHANNELTEXTAREA_BACKGROUND, Color::hex(0x383a40));
        t.set(Token::INPUT_BACKGROUND_DEFAULT, Color::hex(0x1e1f22));
        t
    }

    pub fn set(&mut self, token: impl Into<String>, color: Color) {
        self.0.insert(token.into(), color);
    }

    #[must_use]
    pub fn get(&self, token: &str) -> Option<Color> {
        self.0.get(token).copied()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Entries in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, Color)> {
        self.0.iter().map(|(k, v)| (k.as_str(), *v))
    }

    /// The value of a token, or `fallback` when the table has no such name.
    #[must_use]
    pub fn resolve_or(&self, token: &Token, fallback: Color) -> Color {
        self.get(token.as_str()).unwrap_or(fallback)
    }
}

/// A theme: a token table with a name, enough to render the same tree on
/// either backend.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    pub tokens: Tokens,
}

impl Theme {
    #[must_use]
    pub fn new(name: impl Into<String>, tokens: Tokens) -> Self {
        Self {
            name: name.into(),
            tokens,
        }
    }

    #[must_use]
    pub fn dark() -> Self {
        Self::new("dark", Tokens::defaults())
    }

    /// Resolve a brush. An unknown token falls back to the caller's colour,
    /// so one missing token never blanks a surface.
    #[must_use]
    pub fn color(&self, brush: &Brush, fallback: Color) -> Color {
        match brush {
            Brush::Color(c) => *c,
            Brush::Token(t) => self.tokens.resolve_or(t, fallback),
        }
    }

    /// Resolve to the wire's `0xAARRGGBB`.
    #[must_use]
    pub fn argb(&self, brush: &Brush, fallback: Color) -> u32 {
        self.color(brush, fallback).argb()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argb_is_alpha_first() {
        assert_eq!(Color::hex(0x5865f2).argb(), 0xff5865f2);
        assert_eq!(Color::hex_alpha(0x11223344).argb(), 0x44112233);
    }

    #[test]
    fn token_round_trips() {
        let t = Token::from("background-brand");
        assert_eq!(t.as_str(), "background-brand");
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, "\"background-brand\"");
        assert_eq!(serde_json::from_str::<Token>(&json).unwrap(), t);
    }

    #[test]
    fn unknown_token_falls_back() {
        let theme = Theme::dark();
        let c = theme.color(&Brush::token("not-a-token"), Color::hex(0x123456));
        assert_eq!(c, Color::hex(0x123456));
        let c = theme.color(&Brush::token(Token::BACKGROUND_BRAND), Color::hex(0));
        assert_eq!(c, Color::hex(0x5865f2));
    }

    #[test]
    fn defaults_cover_the_vocabulary() {
        let theme = Theme::dark();
        for token in [
            Token::BACKGROUND_BASE_LOW,
            Token::TEXT_DEFAULT,
            Token::TEXT_STRONG,
            Token::TEXT_SUBTLE,
            Token::TEXT_MUTED,
            Token::ICON_DEFAULT,
            Token::BORDER_SUBTLE,
            Token::BACKGROUND_BRAND,
        ] {
            assert!(theme.tokens.get(token).is_some(), "missing {token}");
        }
    }
}
