//! Themes without code: a `.theme.json` file.
//!
//! Most themes are a table of `token -> value`, and compiling a plugin to
//! hold one is all overhead: a toolchain to install and a component to build
//! and audit, for what is data. A theme file is that table:
//!
//! ```json
//! {
//!   "rekindle-theme": 1,
//!   "id": "dev.you.midnight",
//!   "name": "Midnight",
//!   "version": "1.0.0",
//!   "tokens": { "background-base-low": "#0b0d12", "radius-md": 6, "density": "compact" },
//!   "tint": { "color": "#6d8cff", "amount": 0.06 },
//!   "surfaces": { "nitro-upsell": { "hidden": true } },
//!   "variants": [
//!     { "name": "midnight" },
//!     { "name": "abyss", "label": "Abyss (pure black)", "tokens": { "background-base-low": "#000000" } }
//!   ],
//!   "default-variant": "midnight"
//! }
//! ```
//!
//! - `tokens`: canonical token names or their aliases ([`crate::tokens`]).
//!   A color is `"#rrggbb"` or `"#rrggbbaa"`, a length a number of logical
//!   pixels, a font a family name the client ships, a choice one of the
//!   token's keywords.
//! - `tint`: one color mixed into every surface token (`amount`, 0-1) and
//!   optionally text (`text-amount`).
//! - `surfaces`: named layout surfaces ([`SURFACES`]) to hide or restyle
//!   (`hidden`, `opacity` 0-1, `radius` 0-32, `background`).
//! - `variants`: optional alternatives layered over the top-level values;
//!   the user picks one on the theme's settings page (a generated `variant`
//!   select), defaulting to `default-variant` or the first.
//! - `description`, `authors`, `updates` mean what they do in a plugin
//!   manifest.
//!
//! A bad value is an error; a token or surface this host doesn't know is a
//! warning (it's skipped), so a theme written for a newer client still loads.
//!
//! The host applies a theme through the same operations a theme plugin calls
//! (`set`, `set-tint`, `set-surface`), under the theme's id, so it layers,
//! resets and unloads like one. A theme holds only the `theming` capability.
//! [`crate::manifest::extract`] reads a theme file as its manifest, so
//! installing, the review screen, signing, trust, updates, the blocklist and
//! the settings page treat it like any plugin, and
//! [`crate::runtime::Runtime::load`] applies it instead of instantiating a
//! component.
//!
//! Whether a file is a theme is decided by its bytes (a component starts with
//! `\0asm`, a theme file with `{`), never by what a manifest says: a
//! component can't claim to be a theme.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value as Json;

use crate::backend::ClientBackend;
use crate::capability::Capability;
use crate::manifest::{self, Manifest, UpdateSource};
use crate::model::{Color, SURFACES, SurfaceStyle, ThemeValue, Tint, UiError, UiOp};
use crate::settings::{self, SettingSpec};
use crate::tokens::{self, ThemeError, TokenInfo, TokenKind};

/// The `rekindle-theme` format this host reads.
pub const FORMAT: u32 = 1;
/// What an installed theme is called (`plugins/<id>/theme.json`).
pub const FILE_NAME: &str = "theme.json";
/// Max size of a theme file.
pub const MAX_BYTES: usize = 256 * 1024;
/// Max tokens in one layer (the top level, or one variant).
pub const MAX_TOKENS: usize = 512;
/// Max variants.
pub const MAX_VARIANTS: usize = 16;
/// The generated setting that picks a variant.
pub const VARIANT_SETTING: &str = "variant";

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// Whether `bytes` are a theme file (rather than a component).
pub fn is_theme(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(BOM).unwrap_or(bytes);
    bytes.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{')
}

/// Values a theme sets, at one level (the top level or a variant).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layer {
    /// Canonical token name -> value.
    pub tokens: BTreeMap<String, ThemeValue>,
    pub tint: Option<Tint>,
    pub surfaces: BTreeMap<String, SurfaceStyle>,
}

impl Layer {
    /// `self` with `over` on top: its tokens and surfaces win, and its tint
    /// replaces this one's.
    pub fn with(&self, over: &Layer) -> Layer {
        let mut out = self.clone();
        out.tokens
            .extend(over.tokens.iter().map(|(k, v)| (k.clone(), v.clone())));
        if over.tint.is_some() {
            out.tint = over.tint;
        }
        out.surfaces
            .extend(over.surfaces.iter().map(|(k, v)| (k.clone(), *v)));
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub name: String,
    /// Shown on the settings page.
    pub label: String,
    pub layer: Layer,
}

/// A checked theme file.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub authors: Vec<String>,
    pub updates: Option<UpdateSource>,
    /// Always applied.
    pub base: Layer,
    /// Alternatives over `base`; the user picks one.
    pub variants: Vec<Variant>,
    /// Index into `variants` (0 when there are none).
    pub default_variant: usize,
    /// What doesn't stop it loading but is probably a mistake: a token or
    /// surface rekindle has no name for (it's skipped).
    pub warnings: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawTheme {
    #[serde(rename = "rekindle-theme")]
    _format: u32,
    id: String,
    name: String,
    version: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    tokens: BTreeMap<String, Json>,
    #[serde(default)]
    tint: Option<RawTint>,
    #[serde(default)]
    surfaces: BTreeMap<String, RawSurface>,
    #[serde(default)]
    variants: Vec<RawVariant>,
    #[serde(default)]
    default_variant: Option<String>,
    #[serde(default)]
    updates: Option<UpdateSource>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawVariant {
    name: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    tokens: BTreeMap<String, Json>,
    #[serde(default)]
    tint: Option<RawTint>,
    #[serde(default)]
    surfaces: BTreeMap<String, RawSurface>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawTint {
    color: String,
    #[serde(default)]
    amount: f32,
    #[serde(default)]
    text_amount: f32,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawSurface {
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    opacity: Option<f32>,
    #[serde(default)]
    radius: Option<f32>,
    #[serde(default)]
    background: Option<String>,
}

/// Read and check a theme file.
pub fn parse(bytes: &[u8]) -> Result<Theme> {
    if bytes.len() > MAX_BYTES {
        bail!(
            "theme file is {} bytes; the limit is {MAX_BYTES}",
            bytes.len()
        );
    }
    let bytes = bytes.strip_prefix(BOM).unwrap_or(bytes);
    let v: Json = serde_json::from_slice(bytes).context("theme file is not valid JSON")?;
    match v.get("rekindle-theme") {
        None => bail!("not a rekindle theme: it has no `\"rekindle-theme\": {FORMAT}`"),
        Some(Json::Number(n)) if n.as_u64() == Some(u64::from(FORMAT)) => {}
        Some(other) => bail!("unsupported theme format {other} (this rekindle reads {FORMAT})"),
    }
    let raw: RawTheme = serde_json::from_value(v).context("invalid theme file")?;

    let catalog: BTreeMap<String, TokenInfo> = tokens::default_tokens()
        .into_iter()
        .map(|(i, _)| (i.name.clone(), i))
        .collect();
    let mut warnings = Vec::new();
    let base = layer(
        "",
        &raw.tokens,
        raw.tint.as_ref(),
        &raw.surfaces,
        &catalog,
        &mut warnings,
    )?;

    if raw.variants.len() > MAX_VARIANTS {
        bail!(
            "{} variants; the limit is {MAX_VARIANTS}",
            raw.variants.len()
        );
    }
    let mut variants: Vec<Variant> = Vec::with_capacity(raw.variants.len());
    for v in &raw.variants {
        check_variant_name(&v.name)?;
        if variants.iter().any(|seen| seen.name == v.name) {
            bail!("two variants are called `{}`", v.name);
        }
        let label = v.label.clone().unwrap_or_else(|| v.name.clone());
        if label.trim().is_empty()
            || label.chars().count() > 64
            || label.chars().any(char::is_control)
        {
            bail!(
                "variant `{}`: `label` must be 1-64 printable characters",
                v.name
            );
        }
        if variants.iter().any(|seen| seen.label == label) {
            bail!("two variants are labelled `{label}`");
        }
        let at = format!("variants[{}].", v.name);
        let layer = layer(
            &at,
            &v.tokens,
            v.tint.as_ref(),
            &v.surfaces,
            &catalog,
            &mut warnings,
        )?;
        variants.push(Variant {
            name: v.name.clone(),
            label,
            layer,
        });
    }
    let default_variant = match &raw.default_variant {
        None => 0,
        Some(_) if variants.is_empty() => {
            bail!("`default-variant` is set but there are no `variants`")
        }
        Some(name) => variants
            .iter()
            .position(|v| &v.name == name)
            .with_context(|| {
                format!("`default-variant` is `{name}`, which isn't one of the variants")
            })?,
    };

    let theme = Theme {
        id: raw.id,
        name: raw.name,
        version: raw.version,
        description: raw.description,
        authors: raw.authors,
        updates: raw.updates,
        base,
        variants,
        default_variant,
        warnings,
    };
    theme.manifest().validate()?;
    Ok(theme)
}

fn check_variant_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !ok {
        bail!("variant name `{name}` must be 1-32 lowercase letters, digits or `-`");
    }
    Ok(())
}

fn layer(
    at: &str,
    raw_tokens: &BTreeMap<String, Json>,
    raw_tint: Option<&RawTint>,
    raw_surfaces: &BTreeMap<String, RawSurface>,
    catalog: &BTreeMap<String, TokenInfo>,
    warnings: &mut Vec<String>,
) -> Result<Layer> {
    if raw_tokens.len() > MAX_TOKENS {
        bail!(
            "{at}tokens: {} tokens; the limit is {MAX_TOKENS}",
            raw_tokens.len()
        );
    }
    let mut out = Layer::default();
    for (name, raw) in raw_tokens {
        let canonical = tokens::canonical(name);
        let Some(info) = catalog.get(canonical) else {
            warnings.push(format!(
                "{at}tokens: rekindle has no token `{name}`; it is skipped"
            ));
            continue;
        };
        let value = token_value(info, raw).with_context(|| format!("{at}tokens.{name}"))?;
        tokens::validate(info, &value)
            .map_err(|e| anyhow!("{at}tokens.{name}: {}", explain(e, info, &value)))?;
        if out.tokens.insert(canonical.to_string(), value).is_some() {
            bail!(
                "{at}tokens: `{name}` sets `{canonical}`, which another name in the same list sets too"
            );
        }
    }
    if let Some(t) = raw_tint {
        let unit = |x: f32, what: &str| -> Result<f32> {
            if (0.0..=1.0).contains(&x) {
                Ok(x)
            } else {
                bail!("{at}tint.{what} must be between 0 and 1, not {x}")
            }
        };
        out.tint = Some(Tint {
            color: color(&t.color).with_context(|| format!("{at}tint.color"))?,
            amount: unit(t.amount, "amount")?,
            text_amount: unit(t.text_amount, "text-amount")?,
        });
    }
    for (name, s) in raw_surfaces {
        if !SURFACES.contains(&name.as_str()) {
            warnings.push(format!(
                "{at}surfaces: rekindle has no surface `{name}`; it is skipped"
            ));
            continue;
        }
        let ranged = |x: Option<f32>, hi: f32, what: &str| -> Result<Option<f32>> {
            match x {
                Some(v) if !(0.0..=hi).contains(&v) => {
                    bail!("{at}surfaces.{name}.{what} must be between 0 and {hi}, not {v}")
                }
                x => Ok(x),
            }
        };
        let style = SurfaceStyle {
            hidden: s.hidden,
            opacity: ranged(s.opacity, 1.0, "opacity")?,
            radius: ranged(s.radius, 32.0, "radius")?,
            background: s
                .background
                .as_deref()
                .map(color)
                .transpose()
                .with_context(|| format!("{at}surfaces.{name}.background"))?,
        };
        out.surfaces.insert(name.clone(), style);
    }
    Ok(out)
}

fn color(s: &str) -> Result<Color> {
    Color::parse(s).with_context(|| format!("`{s}` is not a color (write #rrggbb or #rrggbbaa)"))
}

fn token_value(info: &TokenInfo, raw: &Json) -> Result<ThemeValue> {
    Ok(match (info.kind, raw) {
        (TokenKind::Color, Json::String(s)) => ThemeValue::Color(color(s)?),
        (TokenKind::Length, Json::Number(n)) => {
            ThemeValue::Length(n.as_f64().context("not a number")? as f32)
        }
        (TokenKind::Font, Json::String(s)) => ThemeValue::Font(s.clone()),
        (TokenKind::Choice, Json::String(s)) => ThemeValue::Choice(s.clone()),
        (kind, v) => {
            let want = match kind {
                TokenKind::Color => "a color like \"#0b0d12\"",
                TokenKind::Length => "a number (logical pixels)",
                TokenKind::Font => "a font family name",
                TokenKind::Choice => "a keyword",
            };
            bail!("expected {want}, not {v}")
        }
    })
}

fn explain(e: ThemeError, info: &TokenInfo, value: &ThemeValue) -> String {
    match (e, value) {
        (ThemeError::OutOfRange, ThemeValue::Length(x)) => match info.range {
            Some((lo, hi)) => format!("{x} is outside {lo}-{hi}"),
            None => format!("{x} is out of range"),
        },
        (ThemeError::OutOfRange, ThemeValue::Choice(k)) => {
            format!("`{k}` isn't one of {}", info.choices.join(", "))
        }
        (ThemeError::OutOfRange, ThemeValue::Font(_)) => {
            "a font family name must be 1-64 characters".into()
        }
        (e, _) => e.to_string(),
    }
}

impl Theme {
    /// The manifest the rest of rekindle sees: the `theming` capability
    /// only, and a `variant` select setting when there are variants.
    pub fn manifest(&self) -> Manifest {
        let settings = if self.variants.is_empty() {
            Vec::new()
        } else {
            vec![self.variant_setting()]
        };
        Manifest {
            schema: manifest::SCHEMA,
            id: self.id.clone(),
            name: self.name.clone(),
            version: self.version.clone(),
            description: self.description.clone(),
            authors: self.authors.clone(),
            capabilities: vec![Capability::Theming],
            network: Vec::new(),
            wit: manifest::rekindle_wit_version().to_string(),
            settings,
            strings: BTreeMap::new(),
            compose_uploads: false,
            updates: self.updates.clone(),
            theme: true,
        }
    }

    fn variant_setting(&self) -> SettingSpec {
        let options: Vec<&str> = self.variants.iter().map(|v| v.label.as_str()).collect();
        let raw = serde_json::json!([{
            "key": VARIANT_SETTING,
            "type": "select",
            "label": "Variant",
            "options": options,
            "default": self.default_variant,
        }]);
        settings::parse_schema(&raw)
            .expect("the generated variant setting is valid")
            .remove(0)
    }

    /// What to show with variant `selected` (an index, as the `variant`
    /// setting stores it; out of range or `None` means the default).
    pub fn resolved(&self, selected: Option<usize>) -> Layer {
        let pick = selected
            .filter(|i| *i < self.variants.len())
            .unwrap_or(self.default_variant);
        match self.variants.get(pick) {
            Some(v) => self.base.with(&v.layer),
            None => self.base.clone(),
        }
    }
}

/// What a theme has set in a backend, so the next application undoes what
/// it no longer sets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Applied {
    pub tokens: BTreeSet<String>,
    pub tint: bool,
    pub surfaces: BTreeSet<String>,
}

/// Make `backend` show `want` for `plugin`, resetting whatever of `prev` it
/// no longer sets. Returns what is now applied, and what this client build
/// didn't take (a token or surface it doesn't have).
pub fn apply(
    backend: &mut dyn ClientBackend,
    plugin: &str,
    want: &Layer,
    prev: &Applied,
) -> (Applied, Vec<String>) {
    let mut now = Applied::default();
    let mut missing = Vec::new();

    for t in prev.tokens.iter().filter(|t| !want.tokens.contains_key(*t)) {
        backend.reset_value(plugin, t);
    }
    for (t, v) in &want.tokens {
        match backend.set_value(plugin, t, v.clone()) {
            Ok(()) => {
                now.tokens.insert(t.clone());
            }
            Err(ThemeError::UnknownToken) => {
                missing.push(format!("this client has no token `{t}`"));
            }
            Err(e) => missing.push(format!("this client refused `{t}`: {e}")),
        }
    }

    match want.tint {
        Some(t) => {
            backend.set_tint(plugin, Some(t));
            now.tint = true;
        }
        None if prev.tint => backend.set_tint(plugin, None),
        None => {}
    }

    for s in prev
        .surfaces
        .iter()
        .filter(|s| !want.surfaces.contains_key(*s))
    {
        // A surface this build can't restyle was never set, so a failed
        // reset has nothing to undo.
        let _ = backend.ui_update(plugin, UiOp::ResetSurface { surface: s.clone() });
    }
    let mut no_surfaces = false;
    for (s, style) in &want.surfaces {
        match backend.set_surface(plugin, s, *style) {
            Ok(()) => {
                now.surfaces.insert(s.clone());
            }
            Err(UiError::Unavailable) => no_surfaces = true,
            Err(_) => missing.push(format!("this client can't restyle `{s}`")),
        }
    }
    if no_surfaces {
        missing.push("this client can't restyle surfaces".into());
    }
    (now, missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MockBackend;
    use crate::settings::SettingKind;

    fn file(extra: &str) -> Vec<u8> {
        format!(r#"{{"rekindle-theme": 1, "id": "dev.test.theme", "name": "Test", "version": "1.0.0"{extra}}}"#).into_bytes()
    }

    fn err(extra: &str) -> String {
        format!("{:#}", parse(&file(extra)).unwrap_err())
    }

    #[test]
    fn detects_by_bytes() {
        assert!(is_theme(b"  \n{\"rekindle-theme\": 1}"));
        assert!(is_theme(b"\xEF\xBB\xBF{}"));
        assert!(!is_theme(b"\0asm\x01\0\0\0"));
        assert!(!is_theme(b""));
        assert!(!is_theme(b"[1]"));
    }

    #[test]
    fn values_by_token_kind() {
        let t = parse(&file(
            r##", "tokens": {"background-base-low": "#0b0d12", "text-default": "#d6dce880", "radius-md": 6,
                 "font-primary": "Inter", "density": "compact", "accent": "#6d8cff"}"##,
        ))
        .unwrap();
        let tok = |n: &str| t.base.tokens.get(n).cloned();
        assert_eq!(
            tok("background-base-low"),
            Some(ThemeValue::Color(Color::rgb(0x0b, 0x0d, 0x12)))
        );
        assert_eq!(
            tok("text-default"),
            Some(ThemeValue::Color(Color {
                r: 0xd6,
                g: 0xdc,
                b: 0xe8,
                a: 0x80
            }))
        );
        assert_eq!(tok("radius-md"), Some(ThemeValue::Length(6.0)));
        assert_eq!(tok("font-primary"), Some(ThemeValue::Font("Inter".into())));
        assert_eq!(tok("density"), Some(ThemeValue::Choice("compact".into())));
        // An alias is stored under its canonical name.
        assert_eq!(
            tok(tokens::canonical("accent")),
            Some(ThemeValue::Color(Color::rgb(0x6d, 0x8c, 0xff)))
        );
        assert!(t.warnings.is_empty());
    }

    #[test]
    fn bad_values_are_errors() {
        assert!(err(r#", "tokens": {"background-base-low": "navy"}"#).contains("not a color"));
        assert!(err(r#", "tokens": {"background-base-low": 3}"#).contains("expected a color"));
        assert!(err(r#", "tokens": {"radius-md": 999}"#).contains("outside 0-64"));
        assert!(
            err(r#", "tokens": {"density": "roomy"}"#)
                .contains("isn't one of compact, cozy, default")
        );
        assert!(err(r#", "tokens": {"font-primary": ""}"#).contains("font family"));
        assert!(
            err(r##", "tint": {"color": "#000000", "amount": 2}"##).contains("between 0 and 1")
        );
        assert!(
            err(r#", "surfaces": {"member-list": {"opacity": 4}}"#).contains("between 0 and 1")
        );
        // Two names for the same token.
        let canon = tokens::canonical("accent");
        assert!(
            err(&format!(
                r##", "tokens": {{"accent": "#000000", "{canon}": "#111111"}}"##
            ))
            .contains("another name")
        );
        // Typos in the file's own keys are caught, not ignored.
        assert!(err(r#", "token": {}"#).contains("unknown field"));
        assert!(
            err(r##", "tint": {"color": "#000000", "amonut": 0.1}"##).contains("unknown field")
        );
    }

    #[test]
    fn unknown_names_are_warnings() {
        let t = parse(&file(r##", "tokens": {"background-sparkle": "#ffffff"}, "surfaces": {"sidebar-of-doom": {"hidden": true}}"##)).unwrap();
        assert!(t.base.tokens.is_empty() && t.base.surfaces.is_empty());
        assert_eq!(t.warnings.len(), 2, "{:?}", t.warnings);
        assert!(t.warnings[0].contains("background-sparkle"));
    }

    #[test]
    fn header_checks() {
        assert!(
            format!("{:#}", parse(b"{\"id\": \"x\"}").unwrap_err())
                .contains("not a rekindle theme")
        );
        assert!(
            format!("{:#}", parse(b"{\"rekindle-theme\": 2}").unwrap_err())
                .contains("unsupported theme format 2")
        );
        assert!(format!("{:#}", parse(b"{nope").unwrap_err()).contains("not valid JSON"));
        let bad_id = br#"{"rekindle-theme": 1, "id": "Not An Id", "name": "x", "version": "1"}"#;
        assert!(parse(bad_id).is_err());
        assert!(parse(&vec![b' '; MAX_BYTES + 1]).is_err());
    }

    #[test]
    fn variants_become_a_setting() {
        let t = parse(&file(
            r##", "tokens": {"text-default": "#eeeeee"},
                 "variants": [{"name": "midnight", "tokens": {"background-base-low": "#0b0d12"}},
                              {"name": "abyss", "label": "Abyss", "tokens": {"background-base-low": "#000000"}, "tint": {"color": "#9b8cff", "amount": 0.1}}],
                 "default-variant": "abyss""##,
        ))
        .unwrap();
        assert_eq!(t.default_variant, 1);
        let m = t.manifest();
        assert!(m.theme);
        assert_eq!(m.capabilities, vec![Capability::Theming]);
        assert_eq!(m.settings.len(), 1);
        let s = &m.settings[0];
        assert_eq!(
            (s.key.as_str(), s.kind),
            (VARIANT_SETTING, SettingKind::Select)
        );
        assert_eq!(s.options, vec!["midnight".to_string(), "Abyss".to_string()]);
        assert_eq!(
            settings::default_value(s),
            settings::SettingValue::Selected(1)
        );

        // The default (abyss) over the base; then midnight.
        let abyss = t.resolved(None);
        assert_eq!(
            abyss.tokens["background-base-low"],
            ThemeValue::Color(Color::rgb(0, 0, 0))
        );
        assert_eq!(
            abyss.tokens["text-default"],
            ThemeValue::Color(Color::rgb(0xee, 0xee, 0xee))
        );
        assert!(abyss.tint.is_some());
        let midnight = t.resolved(Some(0));
        assert_eq!(
            midnight.tokens["background-base-low"],
            ThemeValue::Color(Color::rgb(0x0b, 0x0d, 0x12))
        );
        assert!(midnight.tint.is_none());
        assert_eq!(
            t.resolved(Some(99)),
            abyss,
            "out of range falls back to the default"
        );
    }

    #[test]
    fn variant_checks() {
        assert!(err(r#", "variants": [{"name": "a"}, {"name": "a"}]"#).contains("two variants"));
        assert!(err(r#", "variants": [{"name": "Big Name"}]"#).contains("lowercase"));
        assert!(
            err(r#", "variants": [{"name": "a", "label": "X"}, {"name": "b", "label": "X"}]"#)
                .contains("labelled")
        );
        let label = |l: String| format!(r#", "variants": [{{"name": "a", "label": "{l}"}}]"#);
        assert!(parse(&file(&label("é".repeat(64)))).is_ok());
        assert!(err(&label("é".repeat(65))).contains("1-64 printable characters"));
        assert!(err(r#", "default-variant": "a""#).contains("no `variants`"));
        assert!(
            err(r#", "variants": [{"name": "a"}], "default-variant": "b""#)
                .contains("isn't one of")
        );
        let many: Vec<String> = (0..=MAX_VARIANTS)
            .map(|i| format!(r#"{{"name": "v{i}"}}"#))
            .collect();
        assert!(err(&format!(r#", "variants": [{}]"#, many.join(","))).contains("the limit"));
    }

    #[test]
    fn apply_sets_then_undoes_only_what_changed() {
        let mut b = MockBackend::new();
        let id = "dev.test.theme";
        let base = b.get_value("background-base-low").unwrap();
        let radius = b.get_value("radius-md").unwrap();

        let mut first = Layer::default();
        first.tokens.insert(
            "background-base-low".into(),
            ThemeValue::Color(Color::rgb(1, 2, 3)),
        );
        first
            .tokens
            .insert("radius-md".into(), ThemeValue::Length(0.0));
        first.surfaces.insert(
            "member-list".into(),
            SurfaceStyle {
                hidden: true,
                opacity: None,
                radius: None,
                background: None,
            },
        );
        let (applied, missing) = apply(&mut b, id, &first, &Applied::default());
        assert!(missing.is_empty(), "{missing:?}");
        assert_eq!(
            b.get_value("background-base-low"),
            Some(ThemeValue::Color(Color::rgb(1, 2, 3)))
        );
        assert_eq!(b.get_value("radius-md"), Some(ThemeValue::Length(0.0)));
        assert!(applied.surfaces.contains("member-list"));
        assert_eq!(
            b.surface_style("member-list").map(|(p, s)| (p, s.hidden)),
            Some((id, true))
        );

        // The next application keeps the background, drops the radius.
        let mut second = Layer::default();
        second.tokens.insert(
            "background-base-low".into(),
            ThemeValue::Color(Color::rgb(9, 9, 9)),
        );
        let (applied, _) = apply(&mut b, id, &second, &applied);
        assert_eq!(
            b.get_value("background-base-low"),
            Some(ThemeValue::Color(Color::rgb(9, 9, 9)))
        );
        assert_eq!(b.get_value("radius-md"), Some(radius));
        assert_eq!(applied.tokens.len(), 1);
        assert!(applied.surfaces.is_empty());
        assert!(b.surface_style("member-list").is_none());

        let (applied, _) = apply(&mut b, id, &Layer::default(), &applied);
        assert_eq!(b.get_value("background-base-low"), Some(base));
        assert_eq!(applied, Applied::default());
    }
}
