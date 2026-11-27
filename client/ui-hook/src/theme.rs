//! Framework theming: the scene GPUI built, recoloured from the client's
//! palette to the plugins' overrides.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rekindle_native::client;

/// One client theme's palette for framework theming: every token value in it
/// (for detecting which theme the client is showing) and the recolour pairs
/// (that value -> a plugin's override).
#[derive(Clone, Debug, Default)]
pub struct ThemePalette {
    pub name: String,
    pub colors: Vec<[f32; 4]>,
    pub pairs: Vec<([f32; 4], [f32; 4])>,
}

/// Framework theming: after GPUI draws a frame, rekindle picks the palette
/// whose colours the frame actually uses (re-checked every few frames, so a
/// theme switch in the client is followed), then repaints every colour equal
/// to one of its `from` values (h, s, l; alpha kept proportional) with the
/// `to`: quad fills and borders, text, underlines, shadows. Palettes are
/// known tables, not read from the client. Empty: nothing recoloured.
pub fn set_theme_palettes(palettes: Vec<ThemePalette>) {
    *PALETTES.lock().unwrap() = palettes;
}

/// The client theme the platform read off the client, by palette name. A
/// platform that knows which theme is shown sets it, and plugins' token
/// reads follow it. `None` goes back to the frame's colours.
pub fn set_shown_theme(name: Option<String>) {
    *SHOWN.lock().unwrap() = name;
}

static SHOWN: Mutex<Option<String>> = Mutex::new(None);

/// The client theme shown, by palette name: the platform's, or the one the
/// framework layer detected from the frame's colours.
pub fn detected_theme() -> Option<String> {
    if let Some(name) = SHOWN.lock().unwrap().clone() {
        return Some(name);
    }
    let i = DETECTED.load(Ordering::SeqCst);
    PALETTES
        .lock()
        .unwrap()
        .get(i.checked_sub(1)?)
        .map(|p| p.name.clone())
}

static PALETTES: Mutex<Vec<ThemePalette>> = Mutex::new(Vec::new());
/// 1-based index into PALETTES of the detected theme; 0 = none yet.
static DETECTED: AtomicUsize = AtomicUsize::new(0);
static DETECT_TICK: AtomicUsize = AtomicUsize::new(0);

/// Which palette a frame's colours fit best: the index of the one with the
/// most matching colours, if any matches at all.
fn best_palette(colors: &[[f32; 4]], palettes: &[ThemePalette]) -> Option<usize> {
    palettes
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let ids: Vec<_> = p.colors.iter().map(|c| (*c, *c)).collect();
            (
                colors
                    .iter()
                    .filter(|c| recolor_hsla(**c, &ids).is_some())
                    .count(),
                i,
            )
        })
        .filter(|(n, _)| *n > 0)
        .max_by_key(|(n, i)| (*n, std::cmp::Reverse(*i)))
        .map(|(_, i)| i)
}

/// Primitives recoloured in the last frame (for status).
pub fn recolored_last_frame() -> usize {
    RECOLORED.load(Ordering::SeqCst)
}

static RECOLORED: AtomicUsize = AtomicUsize::new(0);

/// The recoloured version of `c`, if it matches a `from` (hue ignored for
/// greys, alpha scaled so an element drawn at half opacity stays half).
fn recolor_hsla(c: [f32; 4], pairs: &[([f32; 4], [f32; 4])]) -> Option<[f32; 4]> {
    const EPS: f32 = 2e-3;
    let near = |a: f32, b: f32| (a - b).abs() < EPS;
    pairs.iter().find_map(|(from, to)| {
        let grey = from[1] < EPS && c[1] < EPS;
        let matched = (grey || near(c[0], from[0]) || near((c[0] - from[0]).abs(), 1.0))
            && near(c[1], from[1])
            && near(c[2], from[2])
            && from[3] > 0.0;
        matched.then(|| {
            [
                to[0],
                to[1],
                to[2],
                (to[3] * c[3] / from[3]).clamp(0.0, 1.0),
            ]
        })
    })
}

/// Recolour the scene GPUI just built (including what it replayed for
/// cached views) by the theme pairs.
///
/// # Safety
///
/// `window` is the client's live `Window`, on its UI thread, after a draw.
pub(crate) unsafe fn recolor_scene(window: *mut u8) {
    let palettes = PALETTES.lock().unwrap().clone();
    if palettes.is_empty() {
        RECOLORED.store(0, Ordering::SeqCst);
        return;
    }
    // Which theme is the client showing? Re-check every 15 frames, from
    // its fills and text.
    if DETECTED.load(Ordering::SeqCst) == 0
        || DETECT_TICK
            .fetch_add(1, Ordering::SeqCst)
            .is_multiple_of(15)
    {
        let mut seen: Vec<[f32; 4]> = Vec::new();
        // SAFETY: the caller's contract.
        let ok = unsafe {
            client::scene_colors(window, |kind, c| {
                if matches!(kind, client::Paint::Fill | client::Paint::Text) {
                    seen.push(*c);
                }
            })
        };
        if !ok {
            return; // not a scene: wrong layout for this gpui build
        }
        if let Some(i) = best_palette(&seen, &palettes) {
            DETECTED.store(i + 1, Ordering::SeqCst);
        }
    }
    let Some(pairs) = DETECTED
        .load(Ordering::SeqCst)
        .checked_sub(1)
        .and_then(|i| palettes.get(i))
        .map(|p| p.pairs.clone())
    else {
        return;
    };
    if pairs.is_empty() {
        RECOLORED.store(0, Ordering::SeqCst);
        return;
    }
    let mut n = 0usize;
    // SAFETY: the caller's contract.
    unsafe {
        client::scene_colors(window, |_, c| {
            if let Some(v) = recolor_hsla(*c, &pairs) {
                *c = v;
                n += 1;
            }
        })
    };
    RECOLORED.store(n, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::{ThemePalette, best_palette, recolor_hsla};

    #[test]
    fn a_new_brand_colour_leaves_rekindles_buttons_alone() {
        let hsla = |v: u32| rekindle_native::Rgba::hex(v).to_hsla();
        // A plugin moves background-brand (blurple) to a dark red.
        let pairs = [(hsla(0x5865f2ff), hsla(0x473232ff))];
        assert!(
            recolor_hsla(hsla(0x5865f2ff), &pairs).is_some(),
            "the client's brand fills follow it"
        );
        assert_eq!(
            recolor_hsla(hsla(0x5865f6ff), &pairs),
            None,
            "rekindle's buttons keep their colour"
        );
    }

    #[test]
    fn picks_the_palette_the_frame_uses() {
        let pal = |name: &str, colors: &[[f32; 4]]| ThemePalette {
            name: name.into(),
            colors: colors.to_vec(),
            pairs: Vec::new(),
        };
        let (a, b, shared) = (
            [0.6, 0.1, 0.2, 1.0],
            [0.1, 0.1, 0.9, 1.0],
            [0.6, 0.05, 0.3, 1.0],
        );
        let palettes = [pal("dark", &[a, shared]), pal("light", &[b, shared])];
        // Mostly dark's colours, one shared: dark.
        assert_eq!(
            best_palette(&[a, a, shared, [0.3, 0.3, 0.3, 1.0]], &palettes),
            Some(0)
        );
        assert_eq!(best_palette(&[b, b, shared], &palettes), Some(1));
        assert_eq!(best_palette(&[[0.3, 0.3, 0.3, 1.0]], &palettes), None);
    }

    #[test]
    fn matches_exactly_and_keeps_alpha_proportional() {
        let from = [0.6, 0.05, 0.2, 1.0];
        let to = [0.7, 0.4, 0.1, 1.0];
        let pairs = [(from, to)];
        assert_eq!(recolor_hsla(from, &pairs), Some(to));
        // Drawn at half opacity: stays half.
        assert_eq!(
            recolor_hsla([0.6, 0.05, 0.2, 0.5], &pairs),
            Some([0.7, 0.4, 0.1, 0.5])
        );
        assert_eq!(recolor_hsla([0.6, 0.05, 0.25, 1.0], &pairs), None);
        // Greys match whatever their hue says.
        let grey = [(([0.0, 0.0, 0.5, 1.0]), [0.1, 0.5, 0.5, 1.0])];
        assert!(recolor_hsla([0.33, 0.0, 0.5, 1.0], &grey).is_some());
    }
}
