//! The user's rekindle home: developer mode, installed plugins (and whether
//! each is turned on), and plugin storage.
//!
//! Location: `$REKINDLE_HOME`, else `%USERPROFILE%\.rekindle` / `$HOME/.rekindle`.
//!
//! ```text
//! config.json                  Config: developer mode, updates
//! plugins/<id>/plugin.wasm
//! plugins/<id>/plugin.wasm.sig (optional)
//! plugins/<id>/theme.json      a theme file instead of plugin.wasm (+ .sig; `theme`)
//! plugins/<id>/previous.wasm   the version an update replaced, for rollback (+ .sig)
//! plugins/<id>/update.wasm     a downloaded update waiting for review (+ .sig)
//! plugins/<id>/install.json    InstallRecord: on/off, and what the user saw at install
//! channel/channel.json         the last release channel accepted (+ .sig; `updates`)
//! storage/<id>@<account>.json  plugin storage, per account (runtime::StorageLocation::Dir)
//! storage/<id>.json            plugin storage from before an account was known
//! storage/<id>.settings.json   plugin settings
//! plugins/<id>/ledger.json     what the plugin did, a day at a time (`runtime::ledger`)
//! ```
//!
//! The trust model (owner decisions 2026-11-07 and 2026-11-14) has three
//! kinds of plugin:
//!
//! - **First-party**: signed with a project key. Nothing to approve; it
//!   starts off and the user turns it on.
//! - **Community**: its sha256 is listed in the community index of the
//!   cached release channel, which a project key signs. Installing shows
//!   the review screen (`review`) and needs no developer mode.
//! - **Third-party**: anything else. Installing or loading one needs
//!   developer mode, a deliberate switch with a blunt warning.
//!
//! The user's yes to a community or third-party plugin is bound to that
//! file's sha256, so a changed file is not loaded until it is reviewed
//! again. A community update that asks for nothing new installs without a
//! review (`updates`).
//!
//! Every plugin, of any kind, gets the same sandbox and the capabilities its
//! manifest requests. A theme file is a plugin here in every way (trust,
//! review, updates, on/off); it is only stored and loaded differently.
//! Trust is resolved again every time plugins are listed, so a file swapped
//! on disk is caught at the next start. A file on the release channel's
//! blocklist (`updates`) is never loaded.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::capability::{Capability, Trust};
use crate::channel::Channel;
use crate::manifest::{self, Manifest};
use crate::runtime::{Runtime, RuntimeConfig, StorageLocation};
use crate::signing::{self, PublicKey, SigKind, Signature};
use crate::theme;

/// What an installed component is called (`plugins/<id>/plugin.wasm`).
const WASM_FILE: &str = "plugin.wasm";

/// Shown before developer mode is turned on.
pub const DEVELOPER_MODE_WARNING: &str = "\
Developer mode lets you install plugins that are not part of rekindle and
not in its community index. Nobody has reviewed them.

Plugins from outside rekindle can read your messages and send them anywhere,
change what you send, and act as you when you click their buttons. Only
install plugins from people you trust. Nobody from rekindle or Discord will
ever ask you to turn this on; if someone does, it is a scam.";

/// Where a plugin's trust level came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TrustSource {
    /// Signed with a project key.
    ProjectKey { key_id: String, name: String },
    /// Listed in the community index of the release channel with this
    /// serial.
    Community { author: String, serial: u64 },
    /// No signature.
    Unsigned,
    /// Signed, but not with a project key. The signature only proves the
    /// file wasn't changed since whoever made the key signed it.
    OtherKey { key_id: String },
}

/// The result of resolving a plugin's trust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustResolution {
    pub trust: Trust,
    /// sha256 (hex) of the file resolved.
    pub sha256: String,
    pub source: TrustSource,
}

impl TrustResolution {
    /// One line for humans.
    pub fn describe(&self) -> String {
        match &self.source {
            TrustSource::ProjectKey { key_id, name } => {
                format!("first-party (signed by {name}, key {key_id})")
            }
            TrustSource::Community { author, .. } => {
                format!("community (by {author}, in rekindle's community index)")
            }
            TrustSource::Unsigned => "third-party (unsigned)".into(),
            TrustSource::OtherKey { key_id } => {
                format!("third-party (signed by key {key_id}, not a rekindle key)")
            }
        }
    }
}

/// The keys that make a plugin first-party.
#[derive(Debug, Clone)]
pub struct TrustContext {
    pub first_party_keys: Vec<PublicKey>,
}

impl Default for TrustContext {
    /// The project keys compiled into this build.
    fn default() -> Self {
        Self {
            first_party_keys: signing::first_party_keys(),
        }
    }
}

impl TrustContext {
    /// A context with specific first-party keys (tests, and nothing else).
    pub fn with_keys(first_party_keys: Vec<PublicKey>) -> Self {
        Self { first_party_keys }
    }

    /// Resolve a plugin's trust from its signature alone. A signature that
    /// doesn't match the file, or that names a project key but doesn't
    /// verify, is an error: never a silent downgrade to third-party.
    pub fn resolve(&self, wasm: &[u8], sig: Option<&Signature>) -> Result<TrustResolution> {
        let sha256 = signing::sha256_hex(wasm);
        let Some(sig) = sig else {
            return Ok(TrustResolution {
                trust: Trust::ThirdParty,
                sha256,
                source: TrustSource::Unsigned,
            });
        };
        sig.check_bytes(SigKind::Plugin, wasm)
            .context("plugin signature is invalid")?;
        let Some(key) = self
            .first_party_keys
            .iter()
            .find(|k| k.key_id() == sig.key_id)
        else {
            return Ok(TrustResolution {
                trust: Trust::ThirdParty,
                sha256,
                source: TrustSource::OtherKey {
                    key_id: sig.key_id.clone(),
                },
            });
        };
        key.verify_plugin(wasm, sig)
            .context("plugin signature is invalid")?;
        Ok(TrustResolution {
            trust: Trust::FirstParty,
            sha256,
            source: TrustSource::ProjectKey {
                key_id: key.key_id(),
                name: key.name.clone(),
            },
        })
    }

    /// [`Self::resolve`], then a plugin that isn't first-party is community
    /// if `channel` (already verified against a project key) lists its
    /// sha256 in the community index.
    fn resolve_in(
        &self,
        wasm: &[u8],
        sig: Option<&Signature>,
        channel: Option<&Channel>,
    ) -> Result<TrustResolution> {
        let mut r = self.resolve(wasm, sig)?;
        if r.trust == Trust::ThirdParty
            && let Some(c) = channel
            && let Some(entry) = c.community_entry(&r.sha256)
        {
            r.trust = Trust::Community;
            r.source = TrustSource::Community {
                author: entry.author.clone(),
                serial: c.serial,
            };
        }
        Ok(r)
    }
}

/// `config.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub format: u32,
    /// Third-party plugins may be installed and loaded.
    pub developer_mode: bool,
    /// Don't install updates automatically (they are still checked for, and
    /// the blocklist still applies).
    pub manual_updates: bool,
    /// Release channel override (development; normally the built-in one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// Highest channel serial accepted: a lower one is a rollback.
    pub channel_serial: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            format: signing::FORMAT,
            developer_mode: false,
            manual_updates: false,
            channel: None,
            channel_serial: 0,
        }
    }
}

/// `plugins/<id>/install.json`: the user's choices for an installed plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallRecord {
    pub format: u32,
    pub id: String,
    pub version: String,
    /// sha256 of the file installed. For a community or third-party plugin
    /// this is the file the user reviewed; any other file needs a new
    /// review.
    pub sha256: String,
    /// Trust when it was installed.
    pub trust: Trust,
    /// Turned on by the user.
    pub enabled: bool,
    /// What the manifest requested when it was installed, so a review of an
    /// update can say what is new.
    #[serde(default)]
    pub capabilities: BTreeSet<Capability>,
    #[serde(default)]
    pub hosts: BTreeSet<String>,
    /// Updates (sha256) that failed to load and were rolled back; not
    /// offered again.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub bad_updates: BTreeSet<String>,
    /// Third-party: the highest serial of the author's update channel seen
    /// (a lower one is a rollback).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub update_serial: u64,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl InstallRecord {
    /// The record of a fresh install of `manifest`, resolved as `resolution`.
    pub fn new(manifest: &Manifest, resolution: &TrustResolution, enabled: bool) -> Self {
        Self {
            format: signing::FORMAT,
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            sha256: resolution.sha256.clone(),
            trust: resolution.trust,
            enabled,
            capabilities: manifest.capabilities.iter().copied().collect(),
            hosts: manifest
                .network
                .iter()
                .map(|h| h.to_ascii_lowercase())
                .collect(),
            bad_updates: BTreeSet::new(),
            update_serial: 0,
        }
    }
}

/// Whether an installed plugin loads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "reason", rename_all = "kebab-case")]
pub enum InstallState {
    Ready,
    /// Not turned on (`rekindle enable`).
    Off,
    /// Third-party, and developer mode is off.
    DeveloperModeOff,
    /// Not first-party, and the file isn't the one the user reviewed:
    /// review it again (`rekindle install`).
    NeedsReview(String),
    /// Must not be loaded (tampered, unreadable, malformed).
    Refused(String),
    /// On the release channel's blocklist: rekindle turned it off.
    Blocked(String),
}

impl InstallState {
    /// One line for humans.
    pub fn describe(&self) -> String {
        match self {
            InstallState::Ready => "on".into(),
            InstallState::Off => "off".into(),
            InstallState::DeveloperModeOff => "off: third-party, and developer mode is off".into(),
            InstallState::NeedsReview(why) => {
                format!("needs review: {why}; run `rekindle install` again")
            }
            InstallState::Refused(why) => format!("refused: {why}"),
            InstallState::Blocked(why) => format!("turned off by rekindle: {why}"),
        }
    }
}

/// One installed plugin.
#[derive(Debug, Clone)]
pub struct Installed {
    /// The directory name; equals `manifest.id` for a healthy install.
    pub id: String,
    /// `None` only when the file couldn't be read (state is `Refused`).
    pub manifest: Option<Manifest>,
    /// `plugin.wasm`, or `theme.json` for a theme.
    pub file: PathBuf,
    pub sig: Option<PathBuf>,
    pub record: Option<InstallRecord>,
    pub trust: Option<TrustResolution>,
    pub state: InstallState,
}

impl Installed {
    pub fn is_ready(&self) -> bool {
        self.state == InstallState::Ready
    }

    /// The installed file's bytes, unchecked. Load with
    /// [`Self::read_verified`].
    pub fn read_wasm(&self) -> Result<Vec<u8>> {
        std::fs::read(&self.file).with_context(|| format!("reading {}", self.file.display()))
    }

    /// Read the file and check it is byte-for-byte the one whose trust was
    /// resolved, since it could be replaced between resolution and loading.
    /// Returns the bytes and that trust, for [`Runtime::load`].
    pub fn read_verified(&self) -> Result<(Vec<u8>, Trust)> {
        let t = self
            .trust
            .as_ref()
            .context("plugin has no trust resolution")?;
        let bytes = self.read_wasm()?;
        let actual = signing::sha256_hex(&bytes);
        if actual != t.sha256 {
            bail!(
                "{} changed on disk after it was verified (sha256 {actual}, expected {})",
                self.file.display(),
                t.sha256
            );
        }
        Ok((bytes, t.trust))
    }
}

/// The outcome of loading one installed plugin.
#[derive(Debug)]
pub struct LoadReport {
    pub id: String,
    pub trust: Option<Trust>,
    /// `Ok` if loaded; otherwise why not (including "off").
    pub result: Result<(), String>,
}

/// The user's rekindle home directory.
///
/// Plugin ids are directory names here, so every method that takes an id
/// checks it first (`manifest::validate_id`).
#[derive(Debug, Clone)]
pub struct Home {
    root: PathBuf,
    trust: TrustContext,
}

impl Home {
    /// A home at an explicit directory (created lazily on first write).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            trust: TrustContext::default(),
        }
    }

    /// Use other first-party keys than the compiled-in ones (tests).
    pub fn with_trust(mut self, trust: TrustContext) -> Self {
        self.trust = trust;
        self
    }

    /// `$REKINDLE_HOME`, else `~/.rekindle`.
    pub fn from_env() -> Result<Self> {
        Ok(Self::new(rekindle_release::plugin_home()?))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn plugins_dir(&self) -> PathBuf {
        self.root.join("plugins")
    }

    pub fn plugin_dir(&self, id: &str) -> PathBuf {
        self.plugins_dir().join(id)
    }

    pub fn storage_dir(&self) -> PathBuf {
        self.root.join("storage")
    }

    fn config_path(&self) -> PathBuf {
        self.root.join("config.json")
    }

    fn record_path(&self, id: &str) -> PathBuf {
        self.plugin_dir(id).join("install.json")
    }

    /// The installed file: `theme.json` for a theme, else `plugin.wasm`.
    pub(crate) fn artifact_path(&self, id: &str) -> PathBuf {
        let dir = self.plugin_dir(id);
        let theme = dir.join(theme::FILE_NAME);
        if theme.exists() {
            theme
        } else {
            dir.join(WASM_FILE)
        }
    }

    /// A runtime config whose plugin storage lives in this home.
    pub fn runtime_config(&self) -> RuntimeConfig {
        RuntimeConfig {
            storage: StorageLocation::Dir(self.storage_dir()),
            compile_cache: Some(self.compile_cache_dir()),
            ledger: Some(self.plugins_dir()),
            ..Default::default()
        }
    }

    /// What plugin `id` did on each of the last days, as a runtime last
    /// wrote it ([`Runtime::flush_activity`]).
    pub fn activity(&self, id: &str) -> crate::runtime::ledger::Days {
        if manifest::validate_id(id).is_err() {
            return crate::runtime::ledger::Days::default();
        }
        crate::runtime::ledger::Days::load(
            &self.plugin_dir(id).join(crate::runtime::ledger::FILE_NAME),
        )
    }

    /// Compiled plugins, kept between launches (`RuntimeConfig::compile_cache`).
    pub fn compile_cache_dir(&self) -> PathBuf {
        self.root.join("cache").join("compiled")
    }

    // ---- trust and developer mode ----

    /// Resolve a plugin's trust, against the cached release channel's
    /// community index.
    pub fn trust_of(&self, wasm: &[u8], sig: Option<&Signature>) -> Result<TrustResolution> {
        self.trust
            .resolve_in(wasm, sig, self.cached_channel().as_ref())
    }

    /// The keys that make a plugin (and the release channel) first-party.
    pub fn first_party_keys(&self) -> &[PublicKey] {
        &self.trust.first_party_keys
    }

    /// `config.json`, or the defaults if there is none.
    pub fn config(&self) -> Result<Config> {
        Ok(read_json(&self.config_path())?.unwrap_or_default())
    }

    /// Change `config.json`.
    pub(crate) fn update_config(&self, edit: impl FnOnce(&mut Config)) -> Result<()> {
        let mut c = self.config()?;
        c.format = signing::FORMAT;
        edit(&mut c);
        write_json(&self.config_path(), &c)
    }

    pub fn developer_mode(&self) -> Result<bool> {
        Ok(self.config()?.developer_mode)
    }

    /// Turn developer mode on or off. The caller shows
    /// [`DEVELOPER_MODE_WARNING`] and gets a deliberate yes before turning it
    /// on; nothing a plugin can call reaches this.
    pub fn set_developer_mode(&self, on: bool) -> Result<()> {
        self.update_config(|c| c.developer_mode = on)
    }

    // ---- installed plugins ----

    /// The install record of an installed plugin.
    pub fn record(&self, id: &str) -> Result<Option<InstallRecord>> {
        manifest::validate_id(id)?;
        read_json(&self.record_path(id))
    }

    pub(crate) fn write_record(&self, record: &InstallRecord) -> Result<()> {
        write_json(&self.record_path(&record.id), record)
    }

    /// Copy a plugin (a component, or a theme file) in. For a community or
    /// third-party plugin, the caller must have shown the user its review
    /// screen ([`crate::review`]) and got a yes for these exact bytes. A
    /// third-party plugin also needs developer mode.
    ///
    /// A first-party plugin starts off; the others start on (the user just
    /// said yes). Reinstalling keeps the on/off choice. Returns the
    /// installed file and its record.
    pub fn install(
        &self,
        wasm: &[u8],
        sig: Option<&Signature>,
    ) -> Result<(PathBuf, InstallRecord)> {
        let m = manifest::extract(wasm)?.context("not a rekindle plugin (no manifest)")?;
        m.validate()?;
        let channel = self.cached_channel();
        let resolution = self.trust.resolve_in(wasm, sig, channel.as_ref())?;
        if let Some(why) = channel
            .as_ref()
            .and_then(|c| c.blocked_reason(&resolution.sha256))
        {
            bail!("rekindle has turned off this version of {}: {why}", m.id);
        }
        if resolution.trust == Trust::ThirdParty && !self.developer_mode()? {
            bail!(
                "{} is a third-party plugin ({}); installing it needs developer mode (`rekindle developer-mode on`)",
                m.id,
                resolution.describe()
            );
        }
        let prev = self.record(&m.id)?;
        let enabled = match &prev {
            Some(prev) => prev.enabled,
            None => resolution.trust != Trust::FirstParty,
        };
        let mut record = InstallRecord::new(&m, &resolution, enabled);
        if let Some(prev) = prev {
            record.bad_updates = prev.bad_updates;
            record.update_serial = prev.update_serial;
        }

        let dir = self.plugin_dir(&m.id);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let (name, other) = if theme::is_theme(wasm) {
            (theme::FILE_NAME, WASM_FILE)
        } else {
            (WASM_FILE, theme::FILE_NAME)
        };
        let path = dir.join(name);
        let sig_path = signing::sig_path_for(&path);
        // Drop the old record first, so a crash mid-install never leaves a
        // record next to bytes the user didn't review.
        remove_if_present(&self.record_path(&m.id))?;
        // A plugin that became a theme (or the other way round) leaves no
        // stale file of the other kind behind.
        let other = dir.join(other);
        remove_if_present(&signing::sig_path_for(&other))?;
        remove_if_present(&other)?;
        std::fs::write(&path, wasm).with_context(|| format!("writing {}", path.display()))?;
        match sig {
            Some(s) => s.save(&sig_path)?,
            None => remove_if_present(&sig_path)?,
        }
        self.write_record(&record)?;
        Ok((path, record))
    }

    /// Turn an installed plugin on or off.
    pub fn set_enabled(&self, id: &str, on: bool) -> Result<()> {
        let inst = self
            .get(id)?
            .with_context(|| format!("{id} is not installed"))?;
        let mut record = match (inst.record, &inst.trust, &inst.manifest) {
            (Some(r), _, _) => r,
            // A first-party file put in place by hand: nothing to review.
            (None, Some(t), Some(m)) if t.trust == Trust::FirstParty => {
                InstallRecord::new(m, t, false)
            }
            _ => bail!("{id}: {}", inst.state.describe()),
        };
        if on
            && matches!(
                inst.state,
                InstallState::NeedsReview(_) | InstallState::Refused(_) | InstallState::Blocked(_)
            )
        {
            bail!("{id}: {}", inst.state.describe());
        }
        record.enabled = on;
        self.write_record(&record)
    }

    /// Remove an installed plugin. Its storage and settings are kept unless
    /// `purge_storage`. Returns whether it was installed.
    pub fn uninstall(&self, id: &str, purge_storage: bool) -> Result<bool> {
        manifest::validate_id(id)?;
        let dir = self.plugin_dir(id);
        let existed = dir.exists();
        if existed {
            std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
        }
        if purge_storage {
            self.purge_storage(id)?;
        }
        Ok(existed)
    }

    /// Delete a plugin's files in [`Self::storage_dir`]: its storage for each
    /// account and from before one was known (`PluginStorage::open`), and
    /// its settings (`SettingsStore::open`).
    fn purge_storage(&self, id: &str) -> Result<()> {
        let dir = self.storage_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
        };
        let (account, settings) = (format!("{id}@"), format!("{id}.settings"));
        for entry in entries {
            let path = entry
                .with_context(|| format!("reading {}", dir.display()))?
                .path();
            let Some(stem) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".json"))
            else {
                continue;
            };
            if stem == id || stem == settings || stem.starts_with(&account) {
                remove_if_present(&path)?;
            }
        }
        Ok(())
    }

    /// Every installed plugin and whether it loads, resolving trust again.
    pub fn installed(&self) -> Result<Vec<Installed>> {
        let developer_mode = self.developer_mode()?;
        let channel = self.cached_channel();
        let dir = self.plugins_dir();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
        };
        let mut ids: Vec<String> = entries
            .filter_map(std::result::Result::ok)
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|id| manifest::validate_id(id).is_ok())
            .collect();
        ids.sort();
        Ok(ids
            .iter()
            .map(|id| self.inspect_installed(id, developer_mode, channel.as_ref()))
            .collect())
    }

    /// One installed plugin by id.
    pub fn get(&self, id: &str) -> Result<Option<Installed>> {
        manifest::validate_id(id)?;
        if !self.plugin_dir(id).is_dir() {
            return Ok(None);
        }
        let developer_mode = self.developer_mode()?;
        let channel = self.cached_channel();
        Ok(Some(self.inspect_installed(
            id,
            developer_mode,
            channel.as_ref(),
        )))
    }

    fn inspect_installed(
        &self,
        id: &str,
        developer_mode: bool,
        channel: Option<&Channel>,
    ) -> Installed {
        let path = self.artifact_path(id);
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let sig_path = signing::sig_path_for(&path);
        let mut inst = Installed {
            id: id.to_string(),
            manifest: None,
            file: path.clone(),
            sig: sig_path.exists().then_some(sig_path),
            record: None,
            trust: None,
            state: InstallState::Ready,
        };
        let refuse = |mut inst: Installed, why: String| {
            inst.state = InstallState::Refused(why);
            inst
        };

        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => return refuse(inst, format!("cannot read {}: {e}", path.display())),
        };
        match manifest::extract(&bytes) {
            Ok(Some(m)) if m.id == id => inst.manifest = Some(m),
            Ok(Some(m)) => {
                return refuse(
                    inst,
                    format!("{file} is `{}`, but is installed as `{id}`", m.id),
                );
            }
            Ok(None) => return refuse(inst, format!("{file} has no manifest")),
            Err(e) => return refuse(inst, format!("{file} is malformed: {e:#}")),
        }
        let sig = match inst.sig.as_deref().map(Signature::load).transpose() {
            Ok(s) => s,
            Err(e) => return refuse(inst, format!("{e:#}")),
        };
        let resolution = match self.trust.resolve_in(&bytes, sig.as_ref(), channel) {
            Ok(r) => r,
            Err(e) => return refuse(inst, format!("{e:#}")),
        };
        inst.trust = Some(resolution.clone());
        inst.record = match self.record(id) {
            Ok(r) => r,
            Err(e) => return refuse(inst, format!("{e:#}")),
        };

        if let Some(why) = channel.and_then(|c| c.blocked_reason(&resolution.sha256)) {
            inst.state = InstallState::Blocked(why.to_string());
            return inst;
        }
        let reviewed = resolution.trust != Trust::FirstParty;
        inst.state = match (&inst.record, resolution.trust) {
            (None, Trust::FirstParty) => InstallState::Off,
            (None, _) => {
                InstallState::NeedsReview("it wasn't installed with `rekindle install`".into())
            }
            (Some(r), _) if reviewed && r.trust == Trust::FirstParty => {
                InstallState::NeedsReview(format!(
                    "it was first-party when installed, and now is {}",
                    resolution.describe()
                ))
            }
            (Some(r), _) if reviewed && r.sha256 != resolution.sha256 => {
                InstallState::NeedsReview(format!("{file} changed since you reviewed it"))
            }
            (Some(r), _) if !r.enabled => InstallState::Off,
            (Some(_), Trust::ThirdParty) if !developer_mode => InstallState::DeveloperModeOff,
            (Some(_), _) => InstallState::Ready,
        };
        inst
    }

    /// Load every installed plugin that is on into `rt`, and report the
    /// rest. Construct `rt` with [`Home::runtime_config`] so storage persists
    /// in this home.
    pub fn load_into(&self, rt: &mut Runtime) -> Result<Vec<LoadReport>> {
        let installed = self.installed()?;
        // Each install's verified bytes, or why it isn't loaded.
        let read: Vec<Result<(Vec<u8>, Trust), String>> = installed
            .iter()
            .map(|inst| match inst.state {
                InstallState::Ready => inst.read_verified().map_err(|e| format!("{e:#}")),
                ref state => Err(state.describe()),
            })
            .collect();
        let ready: Vec<(&[u8], Trust)> = read
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .map(|(b, t)| (b.as_slice(), *t))
            .collect();
        let mut loaded = rt.load_many(&ready).into_iter();
        Ok(installed
            .into_iter()
            .zip(&read)
            .map(|(inst, r)| {
                let result = match r {
                    Ok(_) => loaded
                        .next()
                        .expect("load_many returns one result per plugin")
                        .map(|_| ())
                        .map_err(|e| format!("{e:#}")),
                    Err(why) => Err(why.clone()),
                };
                LoadReport {
                    trust: inst.trust.as_ref().map(|t| t.trust),
                    id: inst.id,
                    result,
                }
            })
            .collect())
    }
}

/// Read and parse a JSON file, or `None` if there is none.
fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>> {
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b)
            .map(Some)
            .with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Write a JSON file through a temporary file, so a crash leaves the old
/// file or the new one.
pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(value)? + "\n")
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
}

/// Remove a file. One that isn't there is already removed.
pub(crate) fn remove_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("removing {}", path.display()))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signing::Keypair;

    #[test]
    fn resolution_rules() {
        let wasm = b"plugin".as_slice();
        let project = Keypair::from_seed([1; 32], "Project");
        let other = Keypair::from_seed([2; 32], "Other");
        let ctx = TrustContext::with_keys(vec![project.public()]);

        let r = ctx.resolve(wasm, None).unwrap();
        assert_eq!(
            (r.trust, &r.source),
            (Trust::ThirdParty, &TrustSource::Unsigned)
        );

        let r = ctx.resolve(wasm, Some(&project.sign_plugin(wasm))).unwrap();
        assert_eq!(r.trust, Trust::FirstParty);
        assert!(matches!(r.source, TrustSource::ProjectKey { .. }));

        // Signed by someone else: third-party, not an error.
        let r = ctx.resolve(wasm, Some(&other.sign_plugin(wasm))).unwrap();
        assert_eq!(
            r.source,
            TrustSource::OtherKey {
                key_id: other.key_id()
            }
        );

        // Tampered bytes: an error, whoever signed.
        assert!(
            ctx.resolve(b"plugiN", Some(&project.sign_plugin(wasm)))
                .is_err()
        );
        assert!(
            ctx.resolve(b"plugiN", Some(&other.sign_plugin(wasm)))
                .is_err()
        );

        // A signature claiming a project key id but made by another key.
        let mut forged = other.sign_plugin(wasm);
        forged.key_id = project.key_id();
        assert!(ctx.resolve(wasm, Some(&forged)).is_err());

        // The compiled-in keys: an unsigned file is third-party.
        assert_eq!(
            TrustContext::default().resolve(wasm, None).unwrap().trust,
            Trust::ThirdParty
        );
    }

    #[test]
    fn a_new_config_has_the_current_format() {
        let root =
            std::env::temp_dir().join(format!("rekindle-home-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = Home::new(&root);
        assert_eq!(home.config().unwrap(), Config::default());
        home.update_config(|c| c.channel_serial = 7).unwrap();
        let c = home.config().unwrap();
        assert_eq!((c.format, c.channel_serial), (signing::FORMAT, 7));
        // Fields a file leaves out take their defaults.
        std::fs::write(home.config_path(), br#"{"developer_mode": true}"#).unwrap();
        let c = home.config().unwrap();
        assert_eq!((c.format, c.developer_mode), (signing::FORMAT, true));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn purging_removes_settings_and_every_accounts_storage() {
        let root = std::env::temp_dir().join(format!("rekindle-home-purge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = Home::new(&root);
        let storage = home.storage_dir();
        std::fs::create_dir_all(&storage).unwrap();
        let files = [
            "dev.a.notes.json",
            "dev.a.notes@123.json",
            "dev.a.notes@456.json",
            "dev.a.notes.settings.json",
            "dev.a.notes-2.json",
            "dev.a.notes-2@123.json",
            "dev.a.other.settings.json",
        ];
        for f in files {
            std::fs::write(storage.join(f), b"{}").unwrap();
        }

        home.uninstall("dev.a.notes", false).unwrap();
        assert!(files.iter().all(|f| storage.join(f).exists()), "kept");
        home.uninstall("dev.a.notes", true).unwrap();
        let left: Vec<&str> = files
            .into_iter()
            .filter(|f| storage.join(f).exists())
            .collect();
        assert_eq!(
            left,
            [
                "dev.a.notes-2.json",
                "dev.a.notes-2@123.json",
                "dev.a.other.settings.json"
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
