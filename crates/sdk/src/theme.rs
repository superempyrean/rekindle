//! Theme tokens (requires `theming`).
//!
//! Token names follow Discord's current design-token vocabulary:
//! `background-base-{lowest,lower,low}`, `background-surface-{high,higher,highest}`,
//! `background-mod-*`, `background-brand`, `text-{default,strong,subtle,muted,link}`,
//! `icon-*`, `border-*`, `interactive-*`, `control-*`, `input-*`, chat and
//! status tokens, and the tokens that aren't colours: `radius-{xs,sm,md,lg,xl}`,
//! `font-{primary,headline,code}`, `density` and `chat-avatar-size`.
//! Older names (`background-primary`, `text-normal`, `accent`, ...) work as
//! aliases. [`tokens`] lists what the client exposes.
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! # use rekindle_sdk::theme::Density;
//! # fn f(inter_ttf: &[u8]) -> Result<(), String> {
//! theme::set("background-base-low", Color::rgb(0x0b, 0x0d, 0x12));
//! theme::tint(Color::rgb(0xeb, 0x45, 0x9e), 0.12);
//! theme::register_font("Inter", inter_ttf)?;
//! for done in [
//!     theme::set_radius("radius-md", 0.0),
//!     theme::set_density(Density::Compact),
//!     theme::set_font("font-primary", "Inter"),
//! ] {
//!     done.map_err(|e| format!("{e:?}"))?;
//! }
//! # Ok(())
//! # }
//! ```

use crate::Color;
use crate::raw::rekindle::plugin::theming;
pub use theming::{
    Background, StringError, SurfaceStyle, ThemeError, ThemeValue, Tint, TokenInfo, TokenKind,
};

/// The client's spacing density, the `density` token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    /// Tighter spacing than the default.
    Compact,
    /// Looser spacing than the default.
    Cozy,
    /// The client's default spacing.
    Default,
}

/// Every token the client exposes, by canonical name, with its kind, group
/// and aliases, sorted.
pub fn tokens() -> Vec<TokenInfo> {
    theming::tokens()
}

/// The names of the tokens in `group` (`"background"`, `"control"`, ...).
pub fn group(group: &str) -> Vec<String> {
    tokens()
        .into_iter()
        .filter(|t| t.group == group)
        .map(|t| t.name)
        .collect()
}

/// The effective colour of a colour token.
pub fn get(token: &str) -> Option<Color> {
    theming::get_color(token)
}

/// The effective value of any token.
pub fn value(token: &str) -> Option<ThemeValue> {
    theming::get(token)
}

/// Overrides a colour token. Returns `false` if the client has no such
/// colour token.
pub fn set(token: &str, color: Color) -> bool {
    theming::set_color(token, color)
}

/// Overrides any token with a value of its kind.
#[expect(
    clippy::needless_pass_by_value,
    reason = "callers build the value inline: `set_value(t, ThemeValue::Length(4.0))`"
)]
pub fn set_value(token: &str, value: ThemeValue) -> Result<(), ThemeError> {
    theming::set(token, &value)
}

/// Overrides several colour tokens, and returns the tokens the client
/// doesn't have.
pub fn set_many<'a>(pairs: impl IntoIterator<Item = (&'a str, Color)>) -> Vec<&'a str> {
    pairs
        .into_iter()
        .filter(|(t, c)| !set(t, *c))
        .map(|(t, _)| t)
        .collect()
}

/// Sets every colour token in `group` to `color`, and returns how many it
/// set.
pub fn set_group(group_name: &str, color: Color) -> usize {
    group(group_name).iter().filter(|t| set(t, color)).count()
}

/// Sets a length token (`radius-*`, `chat-avatar-size`), in logical pixels.
pub fn set_length(token: &str, px: f32) -> Result<(), ThemeError> {
    theming::set(token, &ThemeValue::Length(px))
}

/// Sets a `radius-*` token, in logical pixels. The same as [`set_length`].
pub fn set_radius(token: &str, px: f32) -> Result<(), ThemeError> {
    set_length(token, px)
}

/// Sets a font token (`font-primary`, `font-headline`, `font-code`) to a
/// family the client ships or the plugin registered with [`register_font`].
pub fn set_font(token: &str, family: &str) -> Result<(), ThemeError> {
    theming::set(token, &ThemeValue::Font(family.into()))
}

/// Sets the client's spacing density.
pub fn set_density(d: Density) -> Result<(), ThemeError> {
    let choice = match d {
        Density::Compact => "compact",
        Density::Cozy => "cozy",
        Density::Default => "default",
    };
    theming::set("density", &ThemeValue::Choice(choice.into()))
}

/// Removes the plugin's override of a token.
pub fn reset(token: &str) {
    theming::reset(token);
}

/// Mixes `color` into every surface token, in `OKLab`, by `amount` (0 to 1),
/// as a custom client theme does. Explicit overrides still win.
pub fn tint(color: Color, amount: f32) {
    tint_with_text(color, amount, 0.0);
}

/// [`tint`], also mixing `color` into the text tokens by `text_amount`.
pub fn tint_with_text(color: Color, amount: f32, text_amount: f32) {
    theming::set_tint(Some(Tint {
        color,
        amount,
        text_amount,
    }));
}

/// Removes the plugin's tint.
pub fn clear_tint() {
    theming::set_tint(None);
}

/// Registers a font the plugin bundles (TTF or OTF, at most 8 MiB), for
/// [`set_font`]. Fonts are never fetched from the network.
pub fn register_font(family: &str, data: &[u8]) -> Result<(), String> {
    theming::register_font(family, data)
}

/// The named layout surfaces this build can restyle (`guild-list`,
/// `chat.gift-button`, `title-bar.help`, ...).
pub fn surfaces() -> Vec<String> {
    theming::surfaces()
}

/// Hides a named surface, such as the gift button or the help icon.
pub fn hide_surface(surface: &str) -> Result<(), ThemeError> {
    theming::set_surface(
        surface,
        SurfaceStyle {
            hidden: true,
            opacity: None,
            radius: None,
            background: None,
        },
    )
}

/// Restyles a named surface within bounds; it can't be moved or redrawn.
pub fn set_surface(surface: &str, style: SurfaceStyle) -> Result<(), ThemeError> {
    theming::set_surface(surface, style)
}

/// Removes the plugin's style for a surface.
pub fn reset_surface(surface: &str) {
    theming::reset_surface(surface);
}

/// The keys of the client strings this build lets plugins replace.
pub fn strings() -> Vec<String> {
    theming::strings()
}

/// Shows `text` in place of one of the client's strings, by a key from
/// [`strings`]; `None` restores it.
pub fn set_string(key: &str, text: Option<&str>) -> Result<(), StringError> {
    theming::set_string(key, text)
}

/// Shows one of the plugin's registered images
/// ([`ui::register_image`](crate::ui::register_image)) behind the chat, at
/// `opacity` (0 to 1) with `blur`.
pub fn set_background(image: &str, opacity: f32, blur: f32) {
    theming::set_background(Some(&Background {
        image: image.into(),
        opacity,
        blur,
    }));
}

/// Removes the plugin's chat background.
pub fn clear_background() {
    theming::set_background(None);
}
