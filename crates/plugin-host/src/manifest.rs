//! Static plugin metadata.
//!
//! Authors declare it in `Cargo.toml` (`[package.metadata.rekindle]`);
//! `rekindle build` validates it and embeds it in the component as the
//! `rekindle:manifest` custom section. Because the manifest is inside the
//! `.wasm`, it is covered by the plugin's signature, and the host can show it
//! on the install screen without running any plugin code.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::capability::Capability;
use crate::settings::{self, SettingSpec};

/// Name of the custom section that carries the manifest.
pub const SECTION: &str = "rekindle:manifest";
/// Bumped if the embedded JSON format changes incompatibly.
pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    /// Reverse-DNS-style id, e.g. `dev.example.midnight`.
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub authors: Vec<String>,
    pub capabilities: Vec<Capability>,
    /// Hosts the plugin wants to reach (requires `network`). Exact hostnames.
    #[serde(default)]
    pub network: Vec<String>,
    /// WIT package version the plugin was built against.
    pub wit: String,
    /// The settings schema (design doc §10); the host renders the page.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<SettingSpec>,
    /// Bundled strings for `host.tr`: locale -> key -> text, from the
    /// project's `i18n/<locale>.toml` files.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub strings: BTreeMap<String, BTreeMap<String, String>>,
    /// Opt in to `compose-handler.on-upload` (needs `compose`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub compose_uploads: bool,
    /// Where the author publishes updates (third-party plugins).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updates: Option<UpdateSource>,
    /// Read from a theme file ([`crate::theme`]): it runs no code. Only
    /// [`extract`] sets this, from the file's bytes; a component's embedded
    /// manifest saying so is refused.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub theme: bool,
}

/// A third-party plugin's update source: a release channel (`channel.json`,
/// the same format as rekindle's) signed by the author's key. With developer
/// mode on, rekindle checks it and offers a newer version for review; it is
/// never installed without one. The channel and each listed plugin must be
/// signed by `key`, the key named by the version the user reviewed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSource {
    /// `https://` URL of the author's `channel.json`.
    pub url: String,
    /// The author's Ed25519 public key, base64 (`public_key` in their
    /// `rekindle keygen` `.public.json`).
    pub key: String,
}

impl UpdateSource {
    pub fn validate(&self) -> Result<()> {
        let u = url::Url::parse(&self.url)
            .with_context(|| format!("invalid `updates.url` `{}`", self.url))?;
        if u.scheme() != "https" || u.host_str().is_none() {
            bail!("`updates.url` must be an https:// URL, not `{}`", self.url);
        }
        crate::signing::PublicKey::from_base64(&self.key, "author")
            .context("invalid `updates.key`")?;
        Ok(())
    }

    /// The host checked for updates, for the install screen.
    pub fn host(&self) -> String {
        url::Url::parse(&self.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| self.url.clone())
    }
}

/// Max bundled strings, in bytes across all locales.
const MAX_STRINGS_BYTES: usize = 256 * 1024;

impl Manifest {
    /// Check everything that can be checked statically.
    pub fn validate(&self) -> Result<()> {
        if self.schema != SCHEMA {
            bail!(
                "unsupported manifest schema {} (this host reads {SCHEMA})",
                self.schema
            );
        }
        validate_id(&self.id)?;
        if self.name.trim().is_empty() {
            bail!("manifest `name` is empty");
        }
        if self.version.trim().is_empty() {
            bail!("manifest `version` is empty");
        }
        let has_net = self.capabilities.contains(&Capability::Network);
        if has_net && self.network.is_empty() {
            bail!(
                "`network` capability requested but no hosts declared (add `network = [\"api.example.com\"]`)"
            );
        }
        if !has_net && !self.network.is_empty() {
            bail!("network hosts declared but the `network` capability is not requested");
        }
        for h in &self.network {
            validate_host(h)?;
        }
        settings::validate_schema(&self.settings)?;
        if let Some(u) = &self.updates {
            u.validate()?;
        }
        let bytes: usize = self
            .strings
            .values()
            .flat_map(|t| t.iter())
            .map(|(k, v)| k.len() + v.len())
            .sum();
        if bytes > MAX_STRINGS_BYTES {
            bail!("bundled strings are {bytes} bytes; the limit is {MAX_STRINGS_BYTES}");
        }
        for loc in self.strings.keys() {
            if loc.is_empty()
                || loc.len() > 16
                || !loc.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            {
                bail!("invalid locale `{loc}` in i18n/ (use BCP 47 like `en` or `pt-BR`)");
            }
        }
        if self.compose_uploads && !self.requests(Capability::Compose) {
            bail!("`compose-uploads` needs the `compose` capability");
        }
        if self.theme && self.capabilities != [Capability::Theming] {
            bail!("a theme holds only the `theming` capability");
        }
        Ok(())
    }

    /// Look up a bundled string: the exact locale, then its language
    /// (`pt-BR` -> `pt`), then `en`, then any locale, then `None`.
    pub fn tr(&self, locale: &str, key: &str) -> Option<&str> {
        let lang = locale.split('-').next().unwrap_or(locale);
        [locale, lang, "en"]
            .into_iter()
            .find_map(|l| self.strings.get(l).and_then(|t| t.get(key)))
            .or_else(|| self.strings.values().find_map(|t| t.get(key)))
            .map(String::as_str)
    }

    /// Requested directly, or implied by a requested capability
    /// (`read-messages` covers `read-users`, `act` covers `react`).
    pub fn requests(&self, cap: Capability) -> bool {
        self.capabilities.contains(&cap)
            || cap
                .implied_by()
                .iter()
                .any(|c| self.capabilities.contains(c))
    }
}

/// Check a plugin id: lowercase reverse-DNS, like `dev.yourname.plugin`.
pub fn validate_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 128
        && id.split('.').count() >= 2
        && id.split('.').all(|seg| {
            !seg.is_empty()
                && seg
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                && !seg.starts_with('-')
        });
    if !ok {
        bail!("invalid plugin id `{id}`: use lowercase reverse-DNS like `dev.yourname.plugin`");
    }
    // Plugin `x`'s settings are saved as `x.settings.json`, beside storage
    // files named after plugin ids.
    if id.ends_with(".settings") {
        bail!("invalid plugin id `{id}`: an id can't end in `.settings`");
    }
    Ok(())
}

/// Hosts must be plain DNS names: no scheme, port, path, or wildcard. Exact
/// names keep the install screen honest about where data can go.
pub fn validate_host(h: &str) -> Result<()> {
    let ok = !h.is_empty()
        && h.len() <= 253
        && h.contains('.')
        && h.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        });
    if !ok {
        bail!("invalid network host `{h}`: use a bare hostname like `api.example.com`");
    }
    Ok(())
}

/// Is this one of Discord's own hosts? Allowed (unauthenticated: the host
/// never attaches credentials), but called out on the install screen.
pub fn is_discord_host(h: &str) -> bool {
    const ROOTS: [&str; 8] = [
        "discord.com",
        "discordapp.com",
        "discord.gg",
        "discordapp.net",
        "discord.media",
        "discord.new",
        "discord.gift",
        "discordcdn.com",
    ];
    let h = h.to_ascii_lowercase();
    ROOTS
        .iter()
        .any(|r| h == *r || h.ends_with(&format!(".{r}")))
}

/// The `[package.metadata.rekindle]` table as authors write it.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct CargoMetadata {
    pub id: String,
    pub name: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub network: Vec<String>,
    /// The settings schema, as an array of tables (or a table by key).
    #[serde(default)]
    pub settings: Option<serde_json::Value>,
    #[serde(default)]
    pub compose_uploads: bool,
    /// `updates = { url = "https://…/channel.json", key = "<public key>" }`.
    #[serde(default)]
    pub updates: Option<UpdateSource>,
}

impl CargoMetadata {
    /// Combine with package fields into a full manifest.
    pub fn into_manifest(
        self,
        pkg_name: &str,
        version: &str,
        description: &str,
        authors: &[String],
    ) -> Result<Manifest> {
        let capabilities = self
            .capabilities
            .iter()
            .map(|c| {
                Capability::parse(c).with_context(|| {
                    let all: Vec<_> = Capability::ALL.iter().map(|c| c.as_str()).collect();
                    format!(
                        "unknown capability `{c}` (expected one of: {})",
                        all.join(", ")
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let m = Manifest {
            schema: SCHEMA,
            id: self.id,
            name: self.name.unwrap_or_else(|| pkg_name.to_string()),
            version: version.to_string(),
            description: description.to_string(),
            authors: authors.to_vec(),
            capabilities,
            network: self
                .network
                .into_iter()
                .map(|h| h.to_ascii_lowercase())
                .collect(),
            wit: rekindle_wit_version().to_string(),
            settings: match &self.settings {
                Some(raw) => settings::parse_schema(raw)?,
                None => Vec::new(),
            },
            strings: BTreeMap::new(),
            compose_uploads: self.compose_uploads,
            updates: self.updates,
            theme: false,
        };
        m.validate()?;
        Ok(m)
    }
}

/// Read a project's bundled strings: `i18n/<locale>.toml`, each a table of
/// `key = "text"` (nested tables flatten to dotted keys).
pub fn load_strings(
    project_dir: &std::path::Path,
) -> Result<BTreeMap<String, BTreeMap<String, String>>> {
    let dir = project_dir.join("i18n");
    let mut out = BTreeMap::new();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(out);
    };
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("toml") {
            continue;
        }
        let locale = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let table: toml::Table =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let mut flat = BTreeMap::new();
        flatten_strings("", &toml::Value::Table(table), &mut flat)
            .with_context(|| format!("{}: values must be strings", path.display()))?;
        out.insert(locale, flat);
    }
    Ok(out)
}

fn flatten_strings(
    prefix: &str,
    v: &toml::Value,
    out: &mut BTreeMap<String, String>,
) -> Result<()> {
    match v {
        toml::Value::Table(t) => {
            for (k, v) in t {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_strings(&key, v, out)?;
            }
        }
        toml::Value::String(s) => {
            out.insert(prefix.to_string(), s.clone());
        }
        other => bail!("`{prefix}` is {other}"),
    }
    Ok(())
}

/// The WIT package version this host implements.
pub fn rekindle_wit_version() -> &'static str {
    "0.8.0"
}

/// Append the manifest to a component as a top-level custom section.
pub fn embed(component: &[u8], manifest: &Manifest) -> Result<Vec<u8>> {
    if extract(component)?.is_some() {
        bail!("component already carries a {SECTION} section");
    }
    let json = serde_json::to_vec(manifest)?;
    let mut section = Vec::new();
    leb128(&mut section, SECTION.len() as u64);
    section.extend_from_slice(SECTION.as_bytes());
    section.extend_from_slice(&json);

    let mut out = component.to_vec();
    out.push(0); // custom section id
    leb128(&mut out, section.len() as u64);
    out.extend_from_slice(&section);
    Ok(out)
}

/// Swap a component's manifest for another (tests and tooling: a plugin
/// whose manifest says less than its code imports must be refused).
pub fn replace(component: &[u8], manifest: &Manifest) -> Result<Vec<u8>> {
    let mut out = component.to_vec();
    // From the last, so the earlier ranges stay where they were.
    for (range, _) in manifest_sections(component)?.into_iter().rev() {
        // The section's id byte and its size come before `range`.
        let mut header = vec![0u8];
        leb128(&mut header, range.len() as u64);
        out.drain(range.start - header.len()..range.end);
    }
    embed(&out, manifest)
}

/// Read a plugin's manifest: from a component's top-level custom section, or
/// a theme file's own fields ([`crate::theme`]).
///
/// In a component, sections inside nested modules/components are ignored,
/// so a plugin can't smuggle a second manifest in its core module. Exactly
/// one is allowed, and it can't say it is a theme: that comes only from the
/// bytes being a theme file.
pub fn extract(artifact: &[u8]) -> Result<Option<Manifest>> {
    if crate::theme::is_theme(artifact) {
        return crate::theme::parse(artifact).map(|t| Some(t.manifest()));
    }
    let sections = manifest_sections(artifact)?;
    let [(_, data)] = sections.as_slice() else {
        if sections.is_empty() {
            return Ok(None);
        }
        bail!("multiple {SECTION} sections");
    };
    let m: Manifest = serde_json::from_slice(data).context("manifest section is not valid JSON")?;
    if m.theme {
        bail!("a component's manifest can't say it is a theme (a theme is a .theme.json file)");
    }
    Ok(Some(m))
}

/// The component's top-level manifest sections: each one's range (its name
/// and data) and its data.
fn manifest_sections(component: &[u8]) -> Result<Vec<(std::ops::Range<usize>, &[u8])>> {
    use wasmparser::{Parser, Payload};
    let mut depth = 0usize;
    let mut found = Vec::new();
    for payload in Parser::new(0).parse_all(component) {
        match payload.context("malformed wasm")? {
            Payload::Version { .. } => depth += 1,
            Payload::End(_) => depth = depth.saturating_sub(1),
            Payload::CustomSection(c) if depth == 1 && c.name() == SECTION => {
                let r = c.range();
                found.push((r.start as usize..r.end as usize, c.data()));
            }
            _ => {}
        }
    }
    Ok(found)
}

fn leb128(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Manifest {
        Manifest {
            schema: SCHEMA,
            id: "dev.example.test".into(),
            name: "Test".into(),
            version: "0.1.0".into(),
            description: String::new(),
            authors: vec![],
            capabilities: vec![Capability::Theming, Capability::Network],
            network: vec!["api.example.com".into()],
            wit: "0.2.0".into(),
            settings: vec![],
            strings: BTreeMap::new(),
            compose_uploads: false,
            updates: None,
            theme: false,
        }
    }

    #[test]
    fn validates() {
        sample().validate().unwrap();
        let mut m = sample();
        m.network.clear();
        assert!(m.validate().is_err());
        let mut m = sample();
        m.network = vec!["https://api.example.com".into()];
        assert!(m.validate().is_err());
        let mut m = sample();
        m.id = "Bad Id".into();
        assert!(m.validate().is_err());
    }

    #[test]
    fn an_id_cant_take_another_plugins_settings_file_name() {
        validate_id("dev.alice.settings-sync").unwrap();
        assert!(validate_id("dev.alice.notes.settings").is_err());
    }

    #[test]
    fn tr_falls_back_by_language_then_english() {
        let mut m = sample();
        m.strings.insert(
            "en".into(),
            BTreeMap::from([("hi".into(), "Hello".into()), ("bye".into(), "Bye".into())]),
        );
        m.strings
            .insert("pt".into(), BTreeMap::from([("hi".into(), "Ola".into())]));
        assert_eq!(m.tr("pt-BR", "hi"), Some("Ola"));
        assert_eq!(m.tr("pt-BR", "bye"), Some("Bye"));
        assert_eq!(m.tr("de", "hi"), Some("Hello"));
        assert_eq!(m.tr("de", "nope"), None);
        m.validate().unwrap();
        m.strings.insert("bad locale".into(), BTreeMap::new());
        assert!(m.validate().is_err());
    }

    #[test]
    fn discord_hosts() {
        assert!(is_discord_host("discord.com"));
        assert!(is_discord_host("CDN.discordapp.com"));
        assert!(!is_discord_host("notdiscord.com"));
        assert!(!is_discord_host("discord.com.evil.example"));
    }

    #[test]
    fn embed_extract_roundtrip() {
        // Smallest valid component: magic + component version/layer.
        let empty_component = [0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];
        assert_eq!(extract(&empty_component).unwrap(), None);
        let with = embed(&empty_component, &sample()).unwrap();
        assert_eq!(extract(&with).unwrap(), Some(sample()));
        assert!(embed(&with, &sample()).is_err());
    }

    #[test]
    fn update_sources_must_be_https_with_a_valid_key() {
        let key = crate::signing::Keypair::generate("Author")
            .unwrap()
            .public()
            .to_base64();
        let ok = UpdateSource {
            url: "https://example.com/c.json".into(),
            key: key.clone(),
        };
        assert!(ok.validate().is_ok());
        assert!(
            UpdateSource {
                url: "http://example.com/c.json".into(),
                key: key.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            UpdateSource {
                url: "C:/plugins/c.json".into(),
                key
            }
            .validate()
            .is_err()
        );
        assert!(
            UpdateSource {
                url: "https://example.com/c.json".into(),
                key: "nope".into()
            }
            .validate()
            .is_err()
        );
    }
}
