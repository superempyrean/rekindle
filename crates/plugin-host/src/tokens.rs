//! The canonical theme token set.
//!
//! Names follow Discord's *current* semantic CSS variables (the
//! `background-base-*` / `background-surface-*` / `text-*` system that
//! replaced `--background-primary` & co.), because the native client is
//! stated to look identical to today's and most likely carries the same
//! design system.
//!
//! A backend exposes these names, or a subset of them, and maps each onto a
//! field of its own theme. The default values are a neutral dark theme, not
//! Discord's exact palette.
//!
//! Older rekindle names (`background-primary`, `text-normal`, `accent`, …) are
//! accepted everywhere as [aliases](ALIASES).

use serde::{Deserialize, Serialize};

use crate::model::{Color, ThemeValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TokenKind {
    Color,
    /// Logical pixels.
    Length,
    /// A font family name.
    Font,
    /// One of a fixed set of keywords (see `choices`).
    Choice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenInfo {
    pub name: String,
    pub kind: TokenKind,
    /// Family, e.g. `background`, `text`, `control`, `radius`.
    pub group: String,
    /// Older names that resolve to this token.
    pub aliases: Vec<String>,
    /// Allowed keywords for `Choice` tokens.
    pub choices: Vec<String>,
    /// Inclusive range for `Length` tokens.
    pub range: Option<(f32, f32)>,
    /// Whether a theme tint applies to this token (and at which strength:
    /// surface tokens get the tint amount, text tokens the text amount).
    pub tint: TintRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TintRole {
    None,
    Surface,
    Text,
}

const fn c(hex: u32) -> ThemeValue {
    ThemeValue::Color(Color::hex(hex))
}

const fn ca(hex: u32, a: u8) -> ThemeValue {
    let Color { r, g, b, .. } = Color::hex(hex);
    ThemeValue::Color(Color { r, g, b, a })
}

/// (name, group, default, tint role)
type Row = (&'static str, &'static str, ThemeValue, TintRole);

use TintRole::{None as N, Surface as S, Text as T};

#[rustfmt::skip]
const COLORS: &[Row] = &[
    // Surfaces: the base ladder (app frame → sidebars → main), raised surfaces, overlays.
    ("background-base-lowest",            "background", c(0x121214), S),
    ("background-base-lower",             "background", c(0x18181b), S),
    ("background-base-low",               "background", c(0x1d1d21), S),
    ("background-surface-high",           "background", c(0x232328), S),
    ("background-surface-higher",         "background", c(0x28282d), S),
    ("background-surface-highest",        "background", c(0x2e2e34), S),
    ("background-mod-subtle",             "background", ca(0xffffff, 0x1f), N),
    ("background-mod-normal",             "background", ca(0xffffff, 0x29), N),
    ("background-mod-strong",             "background", ca(0xffffff, 0x33), N),
    ("background-mod-muted",              "background", ca(0xffffff, 0x14), N),
    ("background-brand",                  "background", c(0x5865f2), N),
    ("background-accent",                 "background", c(0x4e5058), S),
    ("background-scrim",                  "background", ca(0x000000, 0xb3), N),
    ("background-code",                   "background", c(0x232328), S),
    ("background-feedback-critical",      "background", ca(0xf23f43, 0x26), N),
    ("background-feedback-positive",      "background", ca(0x23a55a, 0x26), N),
    ("background-feedback-warning",       "background", ca(0xf0b232, 0x26), N),
    ("background-feedback-info",          "background", ca(0x00a8fc, 0x26), N),
    // Text.
    ("text-default",                      "text", c(0xdbdee1), T),
    ("text-strong",                       "text", c(0xf2f3f5), T),
    ("text-subtle",                       "text", c(0xb5bac1), T),
    ("text-muted",                        "text", c(0x949ba4), T),
    ("text-link",                         "text", c(0x00a8fc), N),
    ("text-brand",                        "text", c(0x949cf7), N),
    ("text-invert",                       "text", c(0x060607), N),
    ("text-code",                         "text", c(0xdbdee1), T),
    ("text-feedback-critical",            "text", c(0xfa777c), N),
    ("text-feedback-positive",            "text", c(0x46c46e), N),
    ("text-feedback-warning",             "text", c(0xf5c55a), N),
    ("text-feedback-info",                "text", c(0x4fb6ff), N),
    ("text-status-online",                "status", c(0x23a55a), N),
    ("text-status-idle",                  "status", c(0xf0b232), N),
    ("text-status-dnd",                   "status", c(0xf23f43), N),
    ("text-status-offline",               "status", c(0x80848e), N),
    // Icons.
    ("icon-default",                      "icon", c(0xdbdee1), T),
    ("icon-strong",                       "icon", c(0xf2f3f5), T),
    ("icon-subtle",                       "icon", c(0xb5bac1), T),
    ("icon-muted",                        "icon", c(0x80848e), T),
    ("icon-brand",                        "icon", c(0x949cf7), N),
    ("icon-link",                         "icon", c(0x00a8fc), N),
    // Borders.
    ("border-subtle",                     "border", ca(0xffffff, 0x14), N),
    ("border-normal",                     "border", ca(0xffffff, 0x1f), N),
    ("border-strong",                     "border", ca(0xffffff, 0x33), N),
    ("border-muted",                      "border", ca(0xffffff, 0x0d), N),
    ("border-focus",                      "border", c(0x00a8fc), N),
    // Interactive elements (sidebar rows, menu items, tabs).
    ("interactive-background-default",    "interactive", ca(0x000000, 0x00), N),
    ("interactive-background-hover",      "interactive", ca(0xffffff, 0x14), N),
    ("interactive-background-active",     "interactive", ca(0xffffff, 0x1f), N),
    ("interactive-background-selected",   "interactive", ca(0xffffff, 0x29), N),
    ("interactive-text-default",          "interactive", c(0xb5bac1), T),
    ("interactive-text-hover",            "interactive", c(0xdbdee1), T),
    ("interactive-text-active",           "interactive", c(0xffffff), T),
    ("interactive-icon-default",          "interactive", c(0xb5bac1), T),
    ("interactive-icon-hover",            "interactive", c(0xdbdee1), T),
    ("interactive-icon-active",           "interactive", c(0xffffff), T),
    ("interactive-muted",                 "interactive", c(0x4e5058), N),
    ("channels-default",                  "interactive", c(0x949ba4), T),
    // Controls (buttons). Primary/secondary/critical × background/text × default/hover/active.
    ("control-primary-background-default",  "control", c(0x5865f2), N),
    ("control-primary-background-hover",    "control", c(0x4752c4), N),
    ("control-primary-background-active",   "control", c(0x3c45a5), N),
    ("control-primary-text-default",        "control", c(0xffffff), N),
    ("control-secondary-background-default","control", c(0x4e5058), S),
    ("control-secondary-background-hover",  "control", c(0x6d6f78), S),
    ("control-secondary-background-active", "control", c(0x80848e), S),
    ("control-secondary-text-default",      "control", c(0xffffff), N),
    ("control-critical-primary-background-default", "control", c(0xda373c), N),
    ("control-critical-primary-background-hover",   "control", c(0xa12828), N),
    ("control-critical-primary-text-default",       "control", c(0xffffff), N),
    // Inputs and small widgets.
    ("input-background-default",          "input", c(0x1e1f22), S),
    ("input-border-default",              "input", ca(0xffffff, 0x1f), N),
    ("input-text-default",                "input", c(0xdbdee1), T),
    ("input-placeholder-text-default",    "input", c(0x6d6f78), T),
    ("switch-background-default",         "input", c(0x80848e), N),
    ("switch-background-active",          "input", c(0x23a55a), N),
    ("slider-thumb-default",              "input", c(0xffffff), N),
    // Chat.
    ("chat-background",                   "chat", c(0x1d1d21), S),
    ("chat-border",                       "chat", ca(0xffffff, 0x0d), N),
    ("channeltextarea-background",        "chat", c(0x28282d), S),
    ("message-background-hover",          "chat", ca(0x000000, 0x1a), N),
    ("message-highlight-background-default","chat", ca(0x5865f2, 0x14), N),
    ("message-mentioned-background-default","chat", ca(0xf0b232, 0x14), N),
    ("mention-background",                "chat", ca(0x5865f2, 0x4d), N),
    ("mention-foreground",                "chat", c(0xc9cdfb), N),
    ("embed-background",                  "chat", c(0x232328), S),
    // Containers.
    ("modal-background",                  "container", c(0x232328), S),
    ("card-background-default",           "container", ca(0xffffff, 0x0a), N),
    ("app-frame-background",              "container", c(0x121214), S),
    ("toast-background",                  "container", c(0x2e2e34), S),
    // Status and scrollbars.
    ("status-online",                     "status", c(0x23a55a), N),
    ("status-danger",                     "status", c(0xf23f43), N),
    ("status-positive",                   "status", c(0x23a55a), N),
    ("status-warning",                    "status", c(0xf0b232), N),
    ("status-speaking",                   "status", c(0x23a55a), N),
    ("scrollbar-auto-thumb",              "scrollbar", c(0x2b2d31), S),
    ("scrollbar-auto-track",              "scrollbar", ca(0x000000, 0x00), N),
    ("scrollbar-thin-thumb",              "scrollbar", c(0x2b2d31), S),
];

/// Older rekindle names → canonical. (They were Discord's previous CSS names.)
pub const ALIASES: &[(&str, &str)] = &[
    ("accent", "background-brand"),
    ("background-floating", "background-surface-higher"),
    ("background-modifier-hover", "background-mod-subtle"),
    ("background-modifier-selected", "background-mod-normal"),
    ("background-primary", "background-base-low"),
    ("background-secondary", "background-base-lower"),
    ("background-tertiary", "background-base-lowest"),
    ("border", "border-subtle"),
    ("channel-icon", "channels-default"),
    ("input-background", "input-background-default"),
    ("interactive-active", "interactive-text-active"),
    ("interactive-hover", "interactive-text-hover"),
    ("interactive-normal", "interactive-text-default"),
    ("scrollbar", "scrollbar-auto-thumb"),
    ("status-idle", "text-status-idle"),
    ("text-normal", "text-default"),
];

/// Contract tokens that Discord's clients draw under tokens of their own,
/// with those tokens. A colour a plugin sets for the first applies to each
/// of the others that no plugin set, and reads of those return it. Read
/// off the desktop client (build 2208) with every one of its tokens given
/// a colour of its own.
pub const DRAWN_AS: &[(&str, &[&str])] = &[
    // The composer. The client carries the contract's token but draws
    // nothing with it.
    ("channeltextarea-background", &["chat-background-default"]),
    // The window behind the title bar, the server rail and the channel
    // list.
    ("app-frame-background", &["background-base-lowest"]),
    // A modal's card, which also draws popouts and the message hover bar.
    ("modal-background", &["background-surface-high"]),
];

/// Contract tokens that rekindle's own surfaces draw with on every client:
/// its toasts' fill. A plugin can set them whether or not the client has a
/// token of its own for them.
pub const OWN: &[&str] = &["toast-background"];

const DENSITY_CHOICES: &[&str] = &["compact", "cozy", "default"];

/// Resolve an alias to its canonical name (canonical names pass through).
pub fn canonical(name: &str) -> &str {
    ALIASES
        .iter()
        .find(|(a, _)| *a == name)
        .map_or(name, |(_, c)| *c)
}

/// The full canonical token table with default values.
pub fn default_tokens() -> Vec<(TokenInfo, ThemeValue)> {
    let aliases_of = |name: &str| -> Vec<String> {
        ALIASES
            .iter()
            .filter(|(_, c)| *c == name)
            .map(|(a, _)| a.to_string())
            .collect()
    };
    let mut out: Vec<(TokenInfo, ThemeValue)> = COLORS
        .iter()
        .map(|(name, group, v, tint)| {
            let info = TokenInfo {
                name: name.to_string(),
                kind: TokenKind::Color,
                group: group.to_string(),
                aliases: aliases_of(name),
                choices: vec![],
                range: None,
                tint: *tint,
            };
            (info, v.clone())
        })
        .collect();

    let length = |name: &str, group: &str, v: f32, range: (f32, f32)| {
        (
            TokenInfo {
                name: name.into(),
                kind: TokenKind::Length,
                group: group.into(),
                aliases: vec![],
                choices: vec![],
                range: Some(range),
                tint: TintRole::None,
            },
            ThemeValue::Length(v),
        )
    };
    for (name, v) in [
        ("radius-xs", 2.0),
        ("radius-sm", 4.0),
        ("radius-md", 8.0),
        ("radius-lg", 12.0),
        ("radius-xl", 16.0),
    ] {
        out.push(length(name, "radius", v, (0.0, 64.0)));
    }
    out.push(length("chat-avatar-size", "size", 40.0, (16.0, 80.0)));

    let font = |name: &str, v: &str| {
        (
            TokenInfo {
                name: name.into(),
                kind: TokenKind::Font,
                group: "font".into(),
                aliases: vec![],
                choices: vec![],
                range: None,
                tint: TintRole::None,
            },
            ThemeValue::Font(v.into()),
        )
    };
    out.push(font("font-primary", "gg sans"));
    out.push(font("font-headline", "gg sans"));
    out.push(font("font-code", "gg mono"));

    out.push((
        TokenInfo {
            name: "density".into(),
            kind: TokenKind::Choice,
            group: "density".into(),
            aliases: vec![],
            choices: DENSITY_CHOICES
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
            range: None,
            tint: TintRole::None,
        },
        ThemeValue::Choice("default".into()),
    ));
    out.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    out
}

/// Check a value against a token's kind and constraints.
pub fn validate(info: &TokenInfo, v: &ThemeValue) -> Result<(), ThemeError> {
    let in_range = match (info.kind, v) {
        (TokenKind::Color, ThemeValue::Color(_)) => true,
        (TokenKind::Length, ThemeValue::Length(x)) => {
            x.is_finite() && info.range.is_none_or(|(lo, hi)| (lo..=hi).contains(x))
        }
        (TokenKind::Font, ThemeValue::Font(f)) => !f.trim().is_empty() && f.len() <= 64,
        (TokenKind::Choice, ThemeValue::Choice(k)) => info.choices.iter().any(|c| c == k),
        _ => return Err(ThemeError::WrongKind),
    };
    if in_range {
        Ok(())
    } else {
        Err(ThemeError::OutOfRange)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeError {
    #[error("unknown token")]
    UnknownToken,
    #[error("value has the wrong kind for this token")]
    WrongKind,
    #[error("value out of range")]
    OutOfRange,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn names_unique_and_aliases_resolve() {
        let t = default_tokens();
        let names: BTreeSet<_> = t.iter().map(|(i, _)| i.name.as_str()).collect();
        assert_eq!(names.len(), t.len(), "duplicate token names");
        for (alias, canon) in ALIASES {
            assert!(names.contains(canon), "alias {alias} -> unknown {canon}");
            assert!(
                !names.contains(alias),
                "alias {alias} shadows a canonical token"
            );
        }
        assert_eq!(canonical("background-primary"), "background-base-low");
        assert_eq!(canonical("text-default"), "text-default");
        assert!(t.len() >= 80, "{} tokens", t.len());
    }

    #[test]
    fn defaults_validate() {
        for (info, v) in default_tokens() {
            validate(&info, &v).unwrap_or_else(|e| panic!("{}: {e}", info.name));
        }
    }

    #[test]
    fn validation() {
        let t = default_tokens();
        let get = |n: &str| t.iter().find(|(i, _)| i.name == n).unwrap().0.clone();
        assert_eq!(
            validate(&get("radius-md"), &ThemeValue::Length(100.0)),
            Err(ThemeError::OutOfRange)
        );
        assert_eq!(
            validate(&get("radius-md"), &ThemeValue::Color(Color::hex(0))),
            Err(ThemeError::WrongKind)
        );
        assert_eq!(
            validate(&get("density"), &ThemeValue::Choice("huge".into())),
            Err(ThemeError::OutOfRange)
        );
        assert!(validate(&get("density"), &ThemeValue::Choice("compact".into())).is_ok());
    }
}
