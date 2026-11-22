//! Midnight: a theme plugin.
//!
//! Applies one of three palettes over the client's semantic tokens, remembers
//! the choice in storage, and adds a palette command to cycle variants.

use rekindle_sdk::prelude::*;

struct Variant {
    name: &'static str,
    tokens: &'static [(&'static str, Color)],
}

const VARIANTS: &[Variant] = &[
    Variant {
        name: "midnight",
        tokens: &[
            ("background-base-low", Color::rgb(0x0b, 0x0d, 0x12)),
            ("background-base-lower", Color::rgb(0x08, 0x0a, 0x0e)),
            ("background-base-lowest", Color::rgb(0x05, 0x06, 0x09)),
            ("background-surface-higher", Color::rgb(0x05, 0x06, 0x09)),
            ("input-background-default", Color::rgb(0x13, 0x16, 0x1d)),
            ("background-brand", Color::rgb(0x6d, 0x8c, 0xff)),
            ("text-default", Color::rgb(0xd6, 0xdc, 0xe8)),
        ],
    },
    Variant {
        name: "abyss",
        tokens: &[
            ("background-base-low", Color::rgb(0x00, 0x00, 0x00)),
            ("background-base-lower", Color::rgb(0x00, 0x00, 0x00)),
            ("background-base-lowest", Color::rgb(0x00, 0x00, 0x00)),
            ("background-surface-higher", Color::rgb(0x0a, 0x0a, 0x0a)),
            ("input-background-default", Color::rgb(0x11, 0x11, 0x11)),
            ("background-brand", Color::rgb(0x9b, 0x8c, 0xff)),
            ("text-default", Color::rgb(0xe6, 0xe6, 0xe6)),
        ],
    },
    Variant {
        name: "dusk",
        tokens: &[
            ("background-base-low", Color::rgb(0x1d, 0x18, 0x26)),
            ("background-base-lower", Color::rgb(0x17, 0x13, 0x1f)),
            ("background-base-lowest", Color::rgb(0x11, 0x0e, 0x17)),
            ("background-surface-higher", Color::rgb(0x11, 0x0e, 0x17)),
            ("input-background-default", Color::rgb(0x26, 0x20, 0x31)),
            ("background-brand", Color::rgb(0xff, 0x8f, 0x6b)),
            ("text-default", Color::rgb(0xe8, 0xdf, 0xf0)),
        ],
    },
];

/// Foregrounds and chrome for a dark surface, applied with every variant. The
/// variants only darken backgrounds; without these, a client in a light base
/// theme keeps its dark text and light hover/selection fills, and ends up
/// dark-on-dark. With them, Midnight is a complete dark theme on any base.
const DARK_SURFACE: &[(&str, Color)] = &[
    ("text-strong", Color::rgb(0xf2, 0xf3, 0xf5)),
    ("text-subtle", Color::rgb(0xb5, 0xba, 0xc1)),
    ("text-muted", Color::rgb(0x94, 0x9b, 0xa4)),
    ("interactive-text-default", Color::rgb(0xb5, 0xba, 0xc1)),
    ("interactive-text-hover", Color::rgb(0xdb, 0xde, 0xe1)),
    ("interactive-text-active", Color::rgb(0xff, 0xff, 0xff)),
    ("icon-muted", Color::rgb(0x80, 0x84, 0x8e)),
    ("icon-subtle", Color::rgb(0x4e, 0x50, 0x58)),
    ("background-surface-high", Color::rgb(0x0d, 0x10, 0x16)),
    ("background-mod-subtle", Color::rgb(0x1a, 0x1d, 0x24)),
    ("background-mod-normal", Color::rgb(0x23, 0x27, 0x33)),
    ("background-mod-strong", Color::rgb(0x2c, 0x31, 0x40)),
    ("border-subtle", Color::rgb(0x1f, 0x23, 0x2c)),
    ("channeltextarea-background", Color::rgb(0x13, 0x16, 0x1d)),
    ("scrollbar-auto-thumb", Color::rgb(0x1a, 0x1b, 0x1e)),
];

struct Midnight {
    variant: usize,
}

impl Midnight {
    fn apply(&self) {
        let v = &VARIANTS[self.variant];
        let unknown = theme::set_many(DARK_SURFACE.iter().chain(v.tokens).copied());
        if !unknown.is_empty() {
            warn!("this client has no tokens: {}", unknown.join(", "));
        }
        info!("applied variant `{}`", v.name);
    }
}

impl Plugin for Midnight {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let saved = storage::get("variant").unwrap_or_default();
        let variant = VARIANTS.iter().position(|v| v.name == saved).unwrap_or(0);
        let plugin = Midnight { variant };
        plugin.apply();
        ui::register_command("next-variant", "Midnight: next variant");
        Ok(plugin)
    }

    fn on_command(&mut self, id: &str) {
        if id == "next-variant" {
            self.variant = (self.variant + 1) % VARIANTS.len();
            if let Err(e) = storage::set("variant", VARIANTS[self.variant].name) {
                error!("could not save variant: {e:?}");
            }
            self.apply();
        }
    }
}

rekindle_sdk::plugin!(Midnight);
