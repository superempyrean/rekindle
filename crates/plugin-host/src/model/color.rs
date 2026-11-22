//! Colors and theme values: [`Color`], with its `OKLab` mixing, the
//! value a theme token holds, and the whole-theme tint.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// 8-bit sRGB with straight alpha. Serializes as `"#rrggbb"` / `"#rrggbbaa"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn hex(v: u32) -> Self {
        Self::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        let byte = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
        match s.len() {
            6 => Some(Self::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Self {
                r: byte(0)?,
                g: byte(2)?,
                b: byte(4)?,
                a: byte(6)?,
            }),
            _ => None,
        }
    }

    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }
}

impl Color {
    /// Mix towards `other` by `t` (0..=1) in `OKLab`, like CSS
    /// `color-mix(in oklab, self, other t)`. Alpha mixes linearly.
    pub fn mix_oklab(self, other: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        let a = to_oklab(self);
        let b = to_oklab(other);
        let m = [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ];
        let (r, g, bl) = from_oklab(m);
        let alpha =
            (f32::from(self.a) + (f32::from(other.a) - f32::from(self.a)) * t).round() as u8;
        Color {
            r,
            g,
            b: bl,
            a: alpha,
        }
    }
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let v = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

fn to_oklab(c: Color) -> [f32; 3] {
    let (r, g, b) = (
        srgb_to_linear(c.r),
        srgb_to_linear(c.g),
        srgb_to_linear(c.b),
    );
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn from_oklab([l, a, b]: [f32; 3]) -> (u8, u8, u8) {
    let l_ = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m_ = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s_ = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    (
        linear_to_srgb(4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_),
        linear_to_srgb(-1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_),
        linear_to_srgb(-0.004_196_086_3 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_),
    )
}

/// A theme token's value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum ThemeValue {
    Color(Color),
    /// Logical pixels.
    Length(f32),
    /// Font family name.
    Font(String),
    /// Keyword from the token's `choices`.
    Choice(String),
}

impl ThemeValue {
    pub fn as_color(&self) -> Option<Color> {
        match self {
            ThemeValue::Color(c) => Some(*c),
            _ => None,
        }
    }
}

/// A whole-theme tint (Nitro-style custom theme): `color` is mixed into every
/// surface token by `amount`, and into text tokens by `text_amount`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Tint {
    pub color: Color,
    pub amount: f32,
    pub text_amount: f32,
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Color::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid color `{s}`")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oklab_mix_endpoints_and_midpoint() {
        let a = Color::hex(0x1d1d21);
        let b = Color::hex(0xff0080);
        assert_eq!(a.mix_oklab(b, 0.0), a);
        assert_eq!(a.mix_oklab(b, 1.0), b);
        let m = a.mix_oklab(b, 0.5);
        assert!(m.r > a.r && m.r < b.r);
        // Round-trips are stable for in-gamut colors.
        assert_eq!(
            Color::hex(0x5865f2).mix_oklab(Color::hex(0x5865f2), 0.3),
            Color::hex(0x5865f2)
        );
    }
}
