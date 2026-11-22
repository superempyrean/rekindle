//! Methods on generated types. They live here because Rust allows inherent
//! impls only in the crate that defines the type.

#![warn(missing_docs)]

use crate::rekindle::plugin::types::{Color, UiEvent, UiValue};

impl Color {
    /// An opaque colour.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// A colour with straight alpha.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Parses `#rgb`, `#rrggbb` or `#rrggbbaa`. The `#` is optional, and
    /// surrounding whitespace is ignored.
    pub fn from_hex(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let nibble = |i: usize| u8::from_str_radix(&s[i..=i], 16).ok();
        let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
        match s.len() {
            3 => Some(Self::rgb(nibble(0)? * 17, nibble(1)? * 17, nibble(2)? * 17)),
            6 => Some(Self::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Self::rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }

    /// `#rrggbb`, or `#rrggbbaa` when not fully opaque.
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    /// Blends linearly towards `other` by `t`, clamped to `0.0..=1.0`.
    pub fn mix(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let lerp = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
        Self::rgba(
            lerp(self.r, other.r),
            lerp(self.g, other.g),
            lerp(self.b, other.b),
            lerp(self.a, other.a),
        )
    }
}

impl UiEvent {
    /// Whether the event came from `element` on `panel`.
    pub fn is(&self, panel: &str, element: &str) -> bool {
        self.surface == panel && self.element == element
    }

    /// The new state, if this is the toggle `element` of `panel`.
    ///
    /// ```
    /// # use rekindle_plugin_api::rekindle::plugin::types::{UiEvent, UiValue};
    /// let ev = UiEvent {
    ///     surface: "settings".into(),
    ///     element: "enabled".into(),
    ///     value: UiValue::Toggled(true),
    /// };
    /// assert_eq!(ev.toggled("settings", "enabled"), Some(true));
    /// assert_eq!(ev.toggled("settings", "other"), None);
    /// ```
    pub fn toggled(&self, panel: &str, element: &str) -> Option<bool> {
        self.is(panel, element)
            .then(|| self.value.as_bool())
            .flatten()
    }

    /// The new value, if this is the slider `element` of `panel`.
    pub fn number(&self, panel: &str, element: &str) -> Option<f64> {
        self.is(panel, element)
            .then(|| self.value.as_number())
            .flatten()
    }

    /// The new colour, if this is the colour picker `element` of `panel`.
    pub fn color(&self, panel: &str, element: &str) -> Option<Color> {
        self.is(panel, element)
            .then(|| self.value.as_color())
            .flatten()
    }

    /// The committed text, if this is the text input `element` of `panel`.
    /// The client sends it when the user commits it, not per keystroke.
    pub fn text(&self, panel: &str, element: &str) -> Option<&str> {
        self.is(panel, element)
            .then(|| self.value.as_text())
            .flatten()
    }

    /// The chosen option's index, if this is the select `element` of `panel`.
    pub fn selected(&self, panel: &str, element: &str) -> Option<u32> {
        self.is(panel, element)
            .then(|| self.value.as_selected())
            .flatten()
    }

    /// Whether this is a click on the button `element` of `panel`.
    pub fn clicked(&self, panel: &str, element: &str) -> bool {
        self.is(panel, element) && matches!(self.value, UiValue::Clicked)
    }
}

impl UiValue {
    /// A toggle's new state.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            UiValue::Toggled(b) => Some(*b),
            _ => None,
        }
    }

    /// A slider's new value.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            UiValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// A colour picker's new colour.
    pub fn as_color(&self) -> Option<Color> {
        match self {
            UiValue::Color(c) => Some(*c),
            _ => None,
        }
    }

    /// A text input's committed text.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            UiValue::Text(t) => Some(t),
            _ => None,
        }
    }

    /// A select's chosen index.
    pub fn as_selected(&self) -> Option<u32> {
        match self {
            UiValue::Selected(i) => Some(*i),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        assert_eq!(
            Color::from_hex("#0b0d12"),
            Some(Color::rgb(0x0b, 0x0d, 0x12))
        );
        assert_eq!(Color::from_hex(" fff "), Some(Color::rgb(255, 255, 255)));
        assert_eq!(Color::from_hex("#11223380").unwrap().to_hex(), "#11223380");
        assert_eq!(Color::from_hex("#12345"), None);
        assert_eq!(Color::from_hex("#gg0000"), None);
    }

    #[test]
    fn hex_takes_digits_only() {
        // `from_str_radix` alone would read `+f` as 15.
        assert_eq!(Color::from_hex("+f+f+f"), None);
        assert_eq!(Color::from_hex("#ffé"), None);
    }

    #[test]
    fn mix_clamps_its_factor() {
        let black = Color::rgb(0, 0, 0);
        let white = Color::rgb(255, 255, 255);
        assert_eq!(black.mix(white, 0.5), Color::rgb(128, 128, 128));
        assert_eq!(black.mix(white, 2.0), white);
        assert_eq!(black.mix(white, -1.0), black);
    }
}
