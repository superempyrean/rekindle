//! Writing a semantic style into the client's own `StyleRefinement`.
//!
//! The offsets come from [`crate::debug`] on the running build. The
//! encodings are gpui-pre 0.3.14's, each one checked against the crate's
//! `Debug` output:
//!
//! | field | bytes |
//! |---|---|
//! | `background` | `Option<Fill>` (72): fill tag 0 (`Solid`), colour space 0, `Hsla` at `+8` |
//! | `border_color`, `text.color` | `Option<Hsla>` (20): `1`, `Hsla` at `+4` |
//! | `opacity`, `flex_grow`, `flex_shrink` | `Option<f32>` (8): `1`, `f32` |
//! | `padding`, `gap`, `size`, `min_size` | `Length`/`DefiniteLength` (8): tag `0` px or `2` fraction, `f32` |
//! | `border_widths`, `corner_radii` | `AbsoluteLength` (8): tag `0` px (`Pixels`), `f32` |
//!
//! A field the build's `Debug` does not name is never written; the caller
//! sees which ones were skipped in [`Written`].

use rekindle_ui::{Color, GpuiAxis, GpuiLen, GpuiStyle};

use crate::debug::StyleOffsets;

/// What one write covered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub background: bool,
    pub text_color: bool,
    pub border_color: bool,
    pub border_widths: bool,
    pub corner_radii: bool,
    pub padding: bool,
    pub gap: bool,
    pub size: bool,
    pub min_size: bool,
    pub opacity: bool,
    pub flex_grow: bool,
    pub flex_shrink: bool,
    pub flex_direction: bool,
    pub skipped: Vec<String>,
}

/// Write `style` into `buffer` at the offsets the build's `Debug` named.
#[expect(
    clippy::too_many_lines,
    reason = "one block per StyleRefinement field, each with its own encoding; splitting hides the field names"
)]
pub fn write_style(offsets: &StyleOffsets, buffer: &mut [u8], style: &GpuiStyle) -> Written {
    let mut written = Written::default();
    let len = buffer.len();
    let at = |name: &str, width: usize| -> Option<usize> {
        let offset = offsets.fields.get(name).copied()?;
        (offset + width <= len).then_some(offset)
    };

    if let Some(color) = style.background {
        if let Some(off) = at("background", 72) {
            write_fill(&mut buffer[off..off + 72], color);
            written.background = true;
        } else {
            written.skipped.push("background".into());
        }
    }
    if let Some(color) = style.text_color {
        if let Some(off) = at("text.color", 20) {
            write_hsla_option(&mut buffer[off..off + 20], color);
            written.text_color = true;
        } else {
            written.skipped.push("text.color".into());
        }
    }
    if let Some(color) = style.border_color {
        if let Some(off) = at("border_color", 20) {
            write_hsla_option(&mut buffer[off..off + 20], color);
            written.border_color = true;
        } else {
            written.skipped.push("border_color".into());
        }
    }
    if let Some(widths) = style.border_widths {
        if let Some(off) = at("border_widths", 32) {
            for (i, width) in widths.iter().enumerate() {
                write_absolute_length(&mut buffer[off + i * 8..off + i * 8 + 8], *width);
            }
            written.border_widths = true;
        } else {
            written.skipped.push("border_widths".into());
        }
    }
    if let Some(radii) = style.corner_radii {
        if let Some(off) = at("corner_radii", 32) {
            for (i, radius) in radii.iter().enumerate() {
                write_absolute_length(&mut buffer[off + i * 8..off + i * 8 + 8], *radius);
            }
            written.corner_radii = true;
        } else {
            written.skipped.push("corner_radii".into());
        }
    }
    if let Some(padding) = style.padding {
        if let Some(off) = at("padding", 32) {
            for (i, length) in padding.iter().enumerate() {
                write_length(&mut buffer[off + i * 8..off + i * 8 + 8], *length);
            }
            written.padding = true;
        } else {
            written.skipped.push("padding".into());
        }
    }
    if let Some(gap) = style.gap {
        if let Some(off) = at("gap", 16) {
            for (i, length) in gap.iter().enumerate() {
                write_length(&mut buffer[off + i * 8..off + i * 8 + 8], *length);
            }
            written.gap = true;
        } else {
            written.skipped.push("gap".into());
        }
    }
    if let Some(size) = style.size {
        if let Some(off) = at("size", 16) {
            for (i, length) in size.iter().enumerate() {
                write_length(&mut buffer[off + i * 8..off + i * 8 + 8], *length);
            }
            written.size = true;
        } else {
            written.skipped.push("size".into());
        }
    }
    if let Some(size) = style.min_size {
        if let Some(off) = at("min_size", 16) {
            for (i, length) in size.iter().enumerate() {
                write_length(&mut buffer[off + i * 8..off + i * 8 + 8], *length);
            }
            written.min_size = true;
        } else {
            written.skipped.push("min_size".into());
        }
    }
    for (name, value, done) in [
        ("opacity", style.opacity, &mut written.opacity),
        ("flex_grow", style.flex_grow, &mut written.flex_grow),
        ("flex_shrink", style.flex_shrink, &mut written.flex_shrink),
    ] {
        let Some(value) = value else { continue };
        if let Some(off) = at(name, 8) {
            write_f32_option(&mut buffer[off..off + 8], value);
            *done = true;
        } else {
            written.skipped.push(name.into());
        }
    }
    if style.flex_direction == GpuiAxis::Column
        && let Some(off) = at("flex_direction", 1)
    {
        // FlexDirection::Column is tag 1 (Row is 0).
        buffer[off] = 1;
        written.flex_direction = true;
    }
    written
}

/// `Option<Fill>` with a solid colour.
fn write_fill(out: &mut [u8], color: Color) {
    out.fill(0);
    out[0..4].copy_from_slice(&0u32.to_le_bytes()); // Fill's Background tag: Solid
    out[4..8].copy_from_slice(&0u32.to_le_bytes()); // ColorSpace::Srgb
    let (h, s, l) = rgb_to_hsl(color);
    out[8..12].copy_from_slice(&h.to_le_bytes());
    out[12..16].copy_from_slice(&s.to_le_bytes());
    out[16..20].copy_from_slice(&l.to_le_bytes());
    out[20..24].copy_from_slice(&(f32::from(color.a) / 255.0).to_le_bytes());
}

/// `Option<Hsla>`: tag 1, then the colour.
fn write_hsla_option(out: &mut [u8], color: Color) {
    out.fill(0);
    out[0..4].copy_from_slice(&1u32.to_le_bytes());
    let (h, s, l) = rgb_to_hsl(color);
    out[4..8].copy_from_slice(&h.to_le_bytes());
    out[8..12].copy_from_slice(&s.to_le_bytes());
    out[12..16].copy_from_slice(&l.to_le_bytes());
    out[16..20].copy_from_slice(&(f32::from(color.a) / 255.0).to_le_bytes());
}

/// `Option<f32>`: tag 1, then the value.
fn write_f32_option(out: &mut [u8], value: f32) {
    out[0..4].copy_from_slice(&1u32.to_le_bytes());
    out[4..8].copy_from_slice(&value.to_le_bytes());
}

/// `Length` / `DefiniteLength`: tag 0 px, 2 fraction, `u32::MAX` auto.
fn write_length(out: &mut [u8], length: GpuiLen) {
    let (tag, value) = match length {
        GpuiLen::Auto => (u32::MAX, 0.0),
        GpuiLen::Px(px) => (0, px),
        GpuiLen::Fraction(f) => (2, f),
    };
    out[0..4].copy_from_slice(&tag.to_le_bytes());
    out[4..8].copy_from_slice(&value.to_le_bytes());
}

/// `AbsoluteLength`: tag 0 `Pixels`, then the value.
fn write_absolute_length(out: &mut [u8], px: f32) {
    out[0..4].copy_from_slice(&0u32.to_le_bytes());
    out[4..8].copy_from_slice(&px.to_le_bytes());
}

/// sRGB to HSL, the 0..=1 form gpui's `Hsla` uses.
#[must_use]
pub fn rgb_to_hsl(color: Color) -> (f32, f32, f32) {
    let r = f32::from(color.r) / 255.0;
    let g = f32::from(color.g) / 255.0;
    let b = f32::from(color.b) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = f32::midpoint(max, min);
    if (max - min).abs() < f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if (max - r).abs() < f32::EPSILON {
        ((g - b) / d).rem_euclid(6.0)
    } else if (max - g).abs() < f32::EPSILON {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, s, l)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn offsets() -> StyleOffsets {
        let mut fields = BTreeMap::new();
        for (name, offset) in [
            ("text.color", 0),
            ("opacity", 256),
            ("border_widths", 272),
            ("corner_radii", 304),
            ("background", 336),
            ("padding", 408),
            ("gap", 440),
            ("inset", 456),
            ("size", 488),
            ("min_size", 504),
            ("max_size", 520),
            ("border_color", 592),
            ("flex_grow", 240),
            ("flex_shrink", 248),
            ("flex_direction", 632),
        ] {
            fields.insert(name.to_string(), offset);
        }
        StyleOffsets { fields, size: 640 }
    }

    fn style() -> GpuiStyle {
        GpuiStyle {
            background: Some(Color::hex(0xff0000)),
            text_color: Some(Color::hex(0x00ff00)),
            border_color: Some(Color::hex(0x0000ff)),
            border_widths: Some([1.0; 4]),
            corner_radii: Some([8.0; 4]),
            padding: Some([GpuiLen::Px(4.0); 4]),
            gap: Some([GpuiLen::Px(6.0); 2]),
            size: Some([GpuiLen::Fraction(1.0), GpuiLen::Auto]),
            min_size: Some([GpuiLen::Px(0.0), GpuiLen::Auto]),
            opacity: Some(0.5),
            flex_grow: Some(1.0),
            flex_shrink: Some(0.0),
            flex_direction: GpuiAxis::Column,
            ..GpuiStyle::default()
        }
    }

    #[test]
    fn background_is_a_solid_fill() {
        let mut buffer = vec![0u8; 640];
        let written = write_style(&offsets(), &mut buffer, &style());
        assert!(written.background);
        let at = &buffer[336..408];
        assert_eq!(u32::from_le_bytes(at[0..4].try_into().unwrap()), 0);
        assert_eq!(u32::from_le_bytes(at[4..8].try_into().unwrap()), 0);
        let (h, s, l) = rgb_to_hsl(Color::hex(0xff0000));
        assert!((h - 0.0).abs() < f32::EPSILON);
        assert!((s - 1.0).abs() < f32::EPSILON);
        assert!((l - 0.5).abs() < f32::EPSILON);
        assert!((f32::from_le_bytes(at[8..12].try_into().unwrap()) - 0.0).abs() < f32::EPSILON);
        assert!((f32::from_le_bytes(at[20..24].try_into().unwrap()) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn padding_and_size_use_the_length_encoding() {
        let mut buffer = vec![0u8; 640];
        write_style(&offsets(), &mut buffer, &style());
        let padding = &buffer[408..440];
        assert_eq!(u32::from_le_bytes(padding[0..4].try_into().unwrap()), 0);
        assert!((f32::from_le_bytes(padding[4..8].try_into().unwrap()) - 4.0).abs() < f32::EPSILON);
        let size = &buffer[488..504];
        assert_eq!(u32::from_le_bytes(size[0..4].try_into().unwrap()), 2);
        assert!((f32::from_le_bytes(size[4..8].try_into().unwrap()) - 1.0).abs() < f32::EPSILON);
        assert_eq!(
            u32::from_le_bytes(size[8..12].try_into().unwrap()),
            u32::MAX
        );
    }

    #[test]
    fn opacity_and_border_are_options() {
        let mut buffer = vec![0u8; 640];
        write_style(&offsets(), &mut buffer, &style());
        assert_eq!(u32::from_le_bytes(buffer[256..260].try_into().unwrap()), 1);
        assert!(
            (f32::from_le_bytes(buffer[260..264].try_into().unwrap()) - 0.5).abs() < f32::EPSILON
        );
        assert_eq!(u32::from_le_bytes(buffer[592..596].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(buffer[304..308].try_into().unwrap()), 0);
    }

    #[test]
    fn flex_factors_are_options() {
        let mut buffer = vec![0u8; 640];
        let written = write_style(&offsets(), &mut buffer, &style());
        assert!(written.flex_grow && written.flex_shrink && written.min_size);
        assert_eq!(u32::from_le_bytes(buffer[240..244].try_into().unwrap()), 1);
        assert!((f32::from_le_bytes(buffer[244..248].try_into().unwrap()) - 1.0).abs() < f32::EPSILON);
        assert_eq!(u32::from_le_bytes(buffer[248..252].try_into().unwrap()), 1);
        assert!(f32::from_le_bytes(buffer[252..256].try_into().unwrap()).abs() < f32::EPSILON);
        assert_eq!(u32::from_le_bytes(buffer[504..508].try_into().unwrap()), 0);
        assert_eq!(
            u32::from_le_bytes(buffer[512..516].try_into().unwrap()),
            u32::MAX
        );
    }

    #[test]
    fn missing_fields_are_skipped_not_guessed() {
        let mut fields = BTreeMap::new();
        fields.insert("opacity".to_string(), 256);
        let offsets = StyleOffsets { fields, size: 640 };
        let mut buffer = vec![0u8; 640];
        let written = write_style(&offsets, &mut buffer, &style());
        assert!(!written.background);
        assert!(written.skipped.contains(&"background".to_string()));
        assert!(written.opacity);
    }

    #[test]
    fn hsl_endpoints() {
        assert_eq!(rgb_to_hsl(Color::hex(0x000000)), (0.0, 0.0, 0.0));
        assert_eq!(rgb_to_hsl(Color::hex(0xffffff)), (0.0, 0.0, 1.0));
        let (h, _, _) = rgb_to_hsl(Color::hex(0x00ff00));
        assert!((h - 1.0 / 3.0).abs() < 1e-6);
    }
}
