use rekindle_sdk::prelude::*;

/// Token overrides. Run `theme::tokens()` (or look at `rekindle dev`) for the
/// full list the client exposes.
const PALETTE: &[(&str, Color)] = &[
    ("background-base-low", Color::rgb(0x1e, 0x1e, 0x2e)),
    ("background-base-lower", Color::rgb(0x18, 0x18, 0x25)),
    ("background-base-lowest", Color::rgb(0x11, 0x11, 0x1b)),
    ("background-brand", Color::rgb(0xcb, 0xa6, 0xf7)),
    ("text-default", Color::rgb(0xcd, 0xd6, 0xf4)),
];

struct Theme;

impl Plugin for Theme {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let unknown = theme::set_many(PALETTE.iter().copied());
        if !unknown.is_empty() {
            warn!("unknown tokens: {unknown:?}");
        }
        info!("{{TITLE}} applied");
        Ok(Theme)
    }
}

rekindle_sdk::plugin!(Theme);
