//! The colour picker's arithmetic: HSV and hex conversions, and which colour
//! a point in the picker stands for. No client types and no state, so it is
//! tested on any platform.

/// The saturation/value square, in logical pixels and cells. rekindle's
/// elements paint solid fills only, so the gradient is a grid of cells.
pub(crate) const SV_SIZE: (f32, f32) = (200.0, 120.0);
pub(crate) const SV_CELLS: (usize, usize) = (20, 12);
/// The hue bar under it.
pub(crate) const HUE_SIZE: (f32, f32) = (200.0, 12.0);
pub(crate) const HUE_CELLS: usize = 40;

/// Hue in degrees `[0, 360)`, saturation and value in `[0, 1]`.
pub(crate) type Hsv = [f32; 3];
/// Logical bounds: x, y, width, height.
pub(crate) type Rect = (f32, f32, f32, f32);

pub(crate) fn hsv_to_rgb([h, s, v]: Hsv) -> [u8; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r, g, b].map(|u| ((u + m) * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// `[r, g, b]` as HSV. A grey has no hue and black no saturation, so those
/// come back as 0: keep the picker's own hue and saturation for them.
#[expect(
    clippy::float_cmp,
    reason = "max is one of r, g and b, and equality says which"
)]
pub(crate) fn rgb_to_hsv([r, g, b]: [u8; 3]) -> Hsv {
    let [r, g, b] = [r, g, b].map(|u| f32::from(u) / 255.0);
    let max = r.max(g).max(b);
    let d = max - r.min(g).min(b);
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    [h, if max == 0.0 { 0.0 } else { d / max }, max]
}

/// `#rrggbb`, or `#rrggbbaa` when not opaque.
pub(crate) fn to_hex([r, g, b]: [u8; 3], a: u8) -> String {
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

/// `rrggbb` or `rrggbbaa`, with or without `#`, as `([r, g, b], a)`. Shorter
/// forms are refused, so a colour isn't applied halfway through typing one.
pub(crate) fn parse_hex(s: &str) -> Option<([u8; 3], u8)> {
    let s = s.trim().trim_start_matches('#');
    if !(s.len() == 6 || s.len() == 8) || !s.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some((
        [byte(0)?, byte(2)?, byte(4)?],
        if s.len() == 8 { byte(6)? } else { 255 },
    ))
}

/// `0xRRGGBBAA`.
pub(crate) fn to_rgba([r, g, b]: [u8; 3], a: u8) -> u32 {
    u32::from_be_bytes([r, g, b, a])
}

/// Saturation and value at pointer `(x, y)` over a square at `bounds`:
/// saturation rises to the right and value to the top. A pointer outside
/// (a drag carried past the edge) is clamped.
pub(crate) fn sv_at(bounds: Rect, x: f32, y: f32) -> (f32, f32) {
    let s = ((x - bounds.0) / bounds.2).clamp(0.0, 1.0);
    let v = 1.0 - ((y - bounds.1) / bounds.3).clamp(0.0, 1.0);
    (s, v)
}

/// Hue at pointer `x` over a bar at `bounds`, clamped below 360.
pub(crate) fn hue_at(bounds: Rect, x: f32) -> f32 {
    (((x - bounds.0) / bounds.2).clamp(0.0, 1.0) * 360.0).min(359.9)
}

/// The colour of cell `(col, row)` of the square at hue `h`: the cell's centre.
pub(crate) fn sv_cell(h: f32, col: usize, row: usize) -> [u8; 3] {
    let s = (col as f32 + 0.5) / SV_CELLS.0 as f32;
    let v = 1.0 - (row as f32 + 0.5) / SV_CELLS.1 as f32;
    hsv_to_rgb([h, s, v])
}

/// The colour of cell `col` of the hue bar.
pub(crate) fn hue_cell(col: usize) -> [u8; 3] {
    hsv_to_rgb([(col as f32 + 0.5) / HUE_CELLS as f32 * 360.0, 1.0, 1.0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(clippy::float_cmp, reason = "these conversions are exact")]
    fn primaries_and_greys_convert() {
        assert_eq!(hsv_to_rgb([0.0, 1.0, 1.0]), [255, 0, 0]);
        assert_eq!(hsv_to_rgb([120.0, 1.0, 1.0]), [0, 255, 0]);
        assert_eq!(hsv_to_rgb([240.0, 1.0, 1.0]), [0, 0, 255]);
        assert_eq!(hsv_to_rgb([77.0, 0.0, 0.5]), [128, 128, 128]);
        assert_eq!(rgb_to_hsv([255, 255, 255]), [0.0, 0.0, 1.0]);
        assert_eq!(rgb_to_hsv([0, 0, 0]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn every_colour_survives_the_round_trip() {
        // Stepping by primes covers each channel's whole range.
        for r in (0..=255u8).step_by(7) {
            for g in (0..=255u8).step_by(11) {
                for b in (0..=255u8).step_by(13) {
                    assert_eq!(hsv_to_rgb(rgb_to_hsv([r, g, b])), [r, g, b]);
                }
            }
        }
    }

    #[test]
    fn hex_reads_six_or_eight_digits_only() {
        assert_eq!(parse_hex("#eb459e"), Some(([0xeb, 0x45, 0x9e], 255)));
        assert_eq!(parse_hex("EB459E80"), Some(([0xeb, 0x45, 0x9e], 0x80)));
        assert_eq!(to_hex([0xeb, 0x45, 0x9e], 255), "#eb459e");
        assert_eq!(to_hex([0xeb, 0x45, 0x9e], 0x80), "#eb459e80");
        for partial in ["", "#", "#eb4", "#eb459", "#eb459e8", "#eb459g", "#ébb459"] {
            assert_eq!(parse_hex(partial), None, "{partial}");
        }
        assert_eq!(to_rgba([0xeb, 0x45, 0x9e], 0x80), 0xeb459e80);
    }

    #[test]
    #[expect(clippy::float_cmp, reason = "these positions map to exact values")]
    fn a_point_in_the_picker_is_a_colour() {
        let square = (100.0, 50.0, SV_SIZE.0, SV_SIZE.1);
        assert_eq!(sv_at(square, 100.0, 50.0), (0.0, 1.0), "top left: white");
        assert_eq!(
            sv_at(square, 300.0, 170.0),
            (1.0, 0.0),
            "bottom right: black"
        );
        assert_eq!(sv_at(square, 200.0, 110.0), (0.5, 0.5));
        assert_eq!(
            sv_at(square, -40.0, 900.0),
            (0.0, 0.0),
            "a drag past the edge is clamped"
        );
        let bar = (100.0, 180.0, HUE_SIZE.0, HUE_SIZE.1);
        assert_eq!(hue_at(bar, 100.0), 0.0);
        assert_eq!(hue_at(bar, 200.0), 180.0);
        assert!(hue_at(bar, 5000.0) < 360.0);
    }

    #[test]
    fn the_grid_runs_white_to_the_hue_and_down_to_black() {
        let [r, g, b] = sv_cell(0.0, 0, 0);
        assert!(r > 230 && g > 220 && b > 220, "near white at the top left");
        let [r, g, b] = sv_cell(0.0, SV_CELLS.0 - 1, 0);
        assert!(r > 230 && g < 20 && b < 20, "the hue at the top right");
        let [r, g, b] = sv_cell(0.0, SV_CELLS.0 - 1, SV_CELLS.1 - 1);
        assert!(r < 20 && g == 0 && b == 0, "near black at the bottom");
        assert_eq!(hue_cell(0)[0], 255);
    }
}
