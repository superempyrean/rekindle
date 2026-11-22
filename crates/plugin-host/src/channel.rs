//! The release channel: one signed file listing the current first-party
//! plugins, the community index and the known-bad plugin files.
//!
//! `channel.json` is signed as a whole (detached `channel.json.sig`, domain
//! `rekindle-channel-sig-v1`) over its exact bytes, by a project key
//! ([`signing::first_party_keys`]). Every plugin it lists also carries its
//! own project-key signature, so an installed update is first-party on its
//! own, without the channel.
//!
//! Two fields keep an old or replayed channel from doing harm:
//!
//! - `serial` goes up with every publish. A host remembers the highest it
//!   has accepted and refuses anything lower, so a stale mirror or an
//!   attacker can't roll users back to a version that was replaced.
//! - `expires` (unix seconds). Past it the channel offers no updates, so
//!   nobody can freeze users on an old channel indefinitely. Its blocklist
//!   still applies: blocking is never the unsafe direction.
//!
//! The community index (`community`) lists plugins written outside the
//! project. Their files carry no project signature: the channel's signature
//! over each listed sha256 is what makes them community plugins
//! (`home::TrustContext`). The index never drops a version it has listed, so
//! an installed one stays a community plugin. Taking one down is a block.
//!
//! `rekindle channel build` / `verify` make and check one; [`crate::updates`]
//! uses it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::capability::Capability;
use crate::manifest::{self, Manifest};
use crate::signing::{self, Keypair, PublicKey, Signature};
pub use rekindle_release::framework::{Framework, LAUNCHER_FILE};
pub use rekindle_release::location::Location;

/// Max size of a fetched channel.
const MAX_CHANNEL_BYTES: u64 = 16 * 1024 * 1024;
/// Max size of a fetched plugin.
const MAX_PLUGIN_BYTES: u64 = 64 * 1024 * 1024;
/// Max size of a signature file.
const MAX_SIG_BYTES: u64 = 64 * 1024;

/// The id namespace of first-party plugins. No community plugin may use it.
pub const FIRST_PARTY_NAMESPACE: &str = "dev.rekindle";

/// `channel.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    /// [`signing::FORMAT`].
    pub format: u32,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Increases with every publish (`rekindle channel build` uses the
    /// current unix time).
    pub serial: u64,
    /// Unix seconds after which the channel offers no updates.
    pub expires: u64,
    /// First-party plugins: every version listed, each signed by a project
    /// key.
    #[serde(default)]
    pub plugins: Vec<ChannelPlugin>,
    /// Plugin files that must not be installed or loaded, whoever made them
    /// and whether or not developer mode is on.
    #[serde(default)]
    pub blocked: Vec<Blocked>,
    /// The community index: plugins from outside the project, reviewed
    /// against its policy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub community: Vec<CommunityPlugin>,
    /// rekindle's own files, for the launcher (`rekindle_release::framework`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub framework: Option<Framework>,
    /// The launcher (`rekindle.exe`), one file listed the same way, for
    /// the installed launcher to replace itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<Framework>,
}

/// One version of a plugin, and where its file is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelPlugin {
    pub id: String,
    pub name: String,
    /// Semver.
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// The plugin API version it was built against (its manifest's `wit`);
    /// a host only takes versions it can load ([`wit_compatible`]).
    pub wit: String,
    /// sha256 (hex) of the exact file bytes.
    pub sha256: String,
    /// Where to download it: absolute, or relative to the channel's location
    /// (`plugins/<sha256>.wasm` from [`build`]). A first-party plugin's
    /// project-key signature is at `<url>.sig`.
    pub url: String,
}

/// A plugin in the community index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommunityPlugin {
    #[serde(flatten)]
    pub plugin: ChannelPlugin,
    /// The account that submitted it, which owns the id's namespace.
    pub author: String,
    /// What its manifest requests, for the store before it is downloaded.
    /// Installing shows the downloaded file's own manifest.
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    #[serde(default)]
    pub network: Vec<String>,
    /// Where the index built it from.
    pub source: Source,
}

/// A plugin's source: a git repository at one commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// An https URL.
    pub repo: String,
    /// The full commit hash.
    pub commit: String,
    /// The plugin's directory in the repository, if it isn't the root.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
}

impl Source {
    /// Check the repository is https, the commit a full hash, and the path
    /// inside the repository.
    pub fn validate(&self) -> Result<()> {
        let u = url::Url::parse(&self.repo)
            .with_context(|| format!("source repository `{}` is not a URL", self.repo))?;
        if u.scheme() != "https" {
            bail!("source repository `{}` must be https", self.repo);
        }
        let hex = self
            .commit
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c));
        if !(hex && (self.commit.len() == 40 || self.commit.len() == 64)) {
            bail!("source commit `{}` is not a full commit hash", self.commit);
        }
        // A drive prefix (`C:`) would make the path absolute on Windows.
        let escapes = self
            .path
            .split(['/', '\\'])
            .any(|seg| seg == ".." || seg.contains(':'));
        if self.path.starts_with(['/', '\\']) || escapes {
            bail!(
                "source path `{}` must stay inside the repository",
                self.path
            );
        }
        Ok(())
    }
}

/// `<file>.listing.json`, next to each community plugin given to [`build`]:
/// who submitted it and what it was built from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    pub author: String,
    pub source: Source,
}

/// Where a community plugin's [`Listing`] is.
pub fn listing_path_for(file: &Path) -> PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(".listing.json");
    PathBuf::from(s)
}

/// A plugin file that must not be installed or loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocked {
    /// sha256 (hex) of the plugin file.
    pub sha256: String,
    /// Shown to the user: why rekindle turned it off.
    pub reason: String,
    /// For people reading the channel; matching is by sha256 only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// A plugin's namespace: its id without the last segment
/// (`dev.alice.notes` is in `dev.alice`). The index gives each namespace to
/// one author.
pub fn namespace(id: &str) -> &str {
    id.rsplit_once('.').map_or(id, |(ns, _)| ns)
}

/// The GitHub login a namespace belongs to: `alice` for `io.github.alice`
/// and every namespace under it.
pub(crate) fn github_login(ns: &str) -> Option<&str> {
    let rest = ns.strip_prefix("io.github.")?;
    Some(rest.split_once('.').map_or(rest, |(login, _)| login))
}

/// Whether `id` is in the first-party namespace.
pub fn is_first_party_id(id: &str) -> bool {
    id.strip_prefix(FIRST_PARTY_NAMESPACE)
        .is_some_and(|rest| rest.starts_with('.'))
}

impl Channel {
    /// An empty channel.
    pub fn new(name: impl Into<String>, serial: u64, expires: u64) -> Self {
        Self {
            format: signing::FORMAT,
            name: name.into(),
            serial,
            expires,
            plugins: vec![],
            blocked: vec![],
            community: vec![],
            framework: None,
            launcher: None,
        }
    }

    /// Structural checks: format, digests, versions, no duplicates, and the
    /// community index's namespaces and sources.
    pub fn validate(&self) -> Result<()> {
        if self.format != signing::FORMAT {
            bail!(
                "unsupported channel format {} (this host reads {})",
                self.format,
                signing::FORMAT
            );
        }
        let mut seen = BTreeSet::new();
        for p in self
            .plugins
            .iter()
            .chain(self.community.iter().map(|c| &c.plugin))
        {
            if !signing::is_sha256_hex(&p.sha256) {
                bail!("channel entry {} has a malformed sha256", p.id);
            }
            semver::Version::parse(&p.version).with_context(|| {
                format!(
                    "channel entry {}: version `{}` is not semver",
                    p.id, p.version
                )
            })?;
            if !seen.insert((p.id.as_str(), p.version.as_str())) {
                bail!("channel lists {} {} twice", p.id, p.version);
            }
            if p.url.is_empty() {
                bail!("channel entry {} has no url", p.id);
            }
        }
        let first_party: BTreeSet<&str> = self.plugins.iter().map(|p| p.id.as_str()).collect();
        let mut owners = BTreeMap::new();
        for c in &self.community {
            let id = c.plugin.id.as_str();
            manifest::validate_id(id)?;
            if is_first_party_id(id) || first_party.contains(id) {
                bail!("community entry {id} uses the first-party namespace");
            }
            if c.author.trim().is_empty() {
                bail!("community entry {id} has no author");
            }
            let ns = namespace(id);
            if let Some(login) = github_login(ns)
                && !login.eq_ignore_ascii_case(&c.author)
            {
                bail!(
                    "{id} is in the GitHub account {login}'s namespace but is listed for {}",
                    c.author
                );
            }
            if let Some(owner) = owners.insert(ns, c.author.as_str())
                && !owner.eq_ignore_ascii_case(&c.author)
            {
                bail!(
                    "namespace {ns} belongs to {owner}, but {id} is listed for {}",
                    c.author
                );
            }
            c.source
                .validate()
                .with_context(|| format!("community entry {id}"))?;
        }
        for b in &self.blocked {
            if !signing::is_sha256_hex(&b.sha256) {
                bail!("channel blocklist has a malformed sha256: {}", b.sha256);
            }
        }
        if let Some(f) = &self.framework {
            f.validate()?;
        }
        if let Some(l) = &self.launcher {
            l.validate_launcher()?;
        }
        Ok(())
    }

    /// Whether the channel is past `expires` at unix time `now`.
    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.expires
    }

    /// Why `sha256` is blocked, if it is.
    pub fn blocked_reason(&self, sha256: &str) -> Option<&str> {
        self.blocked
            .iter()
            .find(|b| b.sha256 == sha256)
            .map(|b| b.reason.as_str())
    }

    /// The newest listed first-party version of `id` that a host
    /// implementing `host_wit` can load and that isn't blocked.
    pub fn latest(&self, id: &str, host_wit: &str) -> Option<&ChannelPlugin> {
        self.newest(self.plugins.iter(), id, host_wit, |p| p)
    }

    /// [`Self::latest`] in the community index.
    pub fn latest_community(&self, id: &str, host_wit: &str) -> Option<&CommunityPlugin> {
        self.newest(self.community.iter(), id, host_wit, |c| &c.plugin)
    }

    fn newest<'a, T>(
        &self,
        items: impl Iterator<Item = &'a T>,
        id: &str,
        host_wit: &str,
        plugin: impl Fn(&T) -> &ChannelPlugin,
    ) -> Option<&'a T> {
        items
            .filter(|t| {
                let p = plugin(t);
                p.id == id
                    && wit_compatible(&p.wit, host_wit)
                    && self.blocked_reason(&p.sha256).is_none()
            })
            .filter_map(|t| {
                semver::Version::parse(&plugin(t).version)
                    .ok()
                    .map(|v| (v, t))
            })
            .max_by(|(a, _), (b, _)| a.cmp(b))
            .map(|(_, t)| t)
    }

    /// The community entry for the file with this sha256.
    pub fn community_entry(&self, sha256: &str) -> Option<&CommunityPlugin> {
        self.community.iter().find(|c| c.plugin.sha256 == sha256)
    }

    /// The newest loadable version of each first-party plugin, in id order.
    pub fn first_party(&self, host_wit: &str) -> Vec<&ChannelPlugin> {
        let ids: BTreeSet<&str> = self.plugins.iter().map(|p| p.id.as_str()).collect();
        ids.into_iter()
            .filter_map(|id| self.latest(id, host_wit))
            .collect()
    }

    /// The newest loadable version of each community plugin, in id order:
    /// what the store lists.
    pub fn store(&self, host_wit: &str) -> Vec<&CommunityPlugin> {
        let ids: BTreeSet<&str> = self
            .community
            .iter()
            .map(|c| c.plugin.id.as_str())
            .collect();
        ids.into_iter()
            .filter_map(|id| self.latest_community(id, host_wit))
            .collect()
    }

    /// The bytes of `channel.json` and their signature by `key`.
    pub fn sign(&self, key: &Keypair) -> (Vec<u8>, Signature) {
        let mut bytes =
            serde_json::to_vec_pretty(self).expect("a channel has only string-keyed maps");
        bytes.push(b'\n');
        let sig = key.sign_channel(&bytes);
        (bytes, sig)
    }

    /// Write `out` and `out.sig`. Returns the signature's path.
    pub fn write_signed(&self, out: &Path, key: &Keypair) -> Result<PathBuf> {
        let (bytes, sig) = self.sign(key);
        std::fs::write(out, bytes).with_context(|| format!("writing {}", out.display()))?;
        let sig_path = signing::sig_path_for(out);
        sig.save(&sig_path)?;
        Ok(sig_path)
    }
}

/// Whether a plugin built against plugin API `plugin` loads in a host that
/// implements `host`. Below 1.0 a minor version is a breaking change, so the
/// minor must match; from 1.0 the major must. Either way the plugin can't be
/// newer than the host.
pub fn wit_compatible(plugin: &str, host: &str) -> bool {
    let (Ok(p), Ok(h)) = (semver::Version::parse(plugin), semver::Version::parse(host)) else {
        return false;
    };
    if h.major == 0 {
        p.major == 0 && p.minor == h.minor && p.patch <= h.patch
    } else {
        p.major == h.major && (p.minor, p.patch) <= (h.minor, h.patch)
    }
}

/// Inputs to [`build`].
pub struct BuildOptions<'a> {
    /// `*.wasm` and `*.theme.json` files, each with a `<file>.sig`.
    pub dir: &'a Path,
    /// Signs the channel.
    pub key: &'a Keypair,
    /// [`Channel::name`].
    pub name: String,
    /// [`Channel::serial`].
    pub serial: u64,
    /// [`Channel::expires`].
    pub expires: u64,
    /// Keys a listed plugin may be signed with (project keys).
    pub plugin_keys: Vec<PublicKey>,
    pub blocked: Vec<Blocked>,
    /// Community plugins to add: `*.wasm` and `*.theme.json` files, each
    /// with a `<file>.listing.json` ([`Listing`]).
    pub community: Option<&'a Path>,
    /// Community entries already published. They are kept as they are,
    /// because the index never drops a version.
    pub keep_community: Vec<CommunityPlugin>,
    /// rekindle's own files to list, already at `framework/<version>/` in
    /// `dir`.
    pub framework: Option<Framework>,
    /// The launcher to list, already at `launcher/<version>/` in `dir`.
    pub launcher: Option<Framework>,
}

/// Build a channel from every `*.wasm` and `*.theme.json` in `dir`
/// (non-recursive). Each plugin's manifest is read from the file, and its
/// `.sig` must verify against one of `plugin_keys`, or the build fails: the
/// channel lists only first-party plugins.
///
/// Each file and its signature are copied to `dir/plugins/<sha256>.<ext>`,
/// which is its `url`. A hosted file never changes, so it can be cached for
/// good, and a channel cached from before an update still points at the
/// bytes it lists.
pub fn build(opts: &BuildOptions) -> Result<Channel> {
    let mut channel = Channel::new(opts.name.clone(), opts.serial, opts.expires);
    for path in plugin_files(opts.dir)? {
        let (bytes, m) = read_plugin(&path)?;
        let sig_path = signing::sig_path_for(&path);
        let sig = Signature::load(&sig_path).with_context(|| {
            format!(
                "{} has no signature; sign it with a project key (`rekindle sign`)",
                path.display()
            )
        })?;
        let Some(key) = opts.plugin_keys.iter().find(|k| k.key_id() == sig.key_id) else {
            bail!(
                "{} is signed by key {}, which is not a project key; the channel lists only first-party plugins",
                path.display(),
                sig.key_id
            );
        };
        key.verify_plugin(&bytes, &sig)
            .with_context(|| format!("{}", sig_path.display()))?;
        channel.plugins.push(host_file(opts.dir, &path, &bytes, m)?);
    }
    channel.community.clone_from(&opts.keep_community);
    if let Some(dir) = opts.community {
        for path in plugin_files(dir)? {
            let (bytes, m) = read_plugin(&path)?;
            let listing_path = listing_path_for(&path);
            let listing = std::fs::read(&listing_path)
                .with_context(|| format!("{} has no listing", path.display()))?;
            let listing: Listing = serde_json::from_slice(&listing)
                .with_context(|| format!("parsing {}", listing_path.display()))?;
            if channel
                .community_entry(&signing::sha256_hex(&bytes))
                .is_some()
            {
                continue;
            }
            let (capabilities, network) = (m.capabilities.clone(), m.network.clone());
            channel.community.push(CommunityPlugin {
                plugin: host_file(opts.dir, &path, &bytes, m)?,
                author: listing.author,
                capabilities,
                network,
                source: listing.source,
            });
        }
    }
    channel.blocked.clone_from(&opts.blocked);
    if let Some(f) = &opts.framework {
        f.check_dir(&opts.dir.join(&f.base))
            .context("the framework files in the channel directory")?;
        channel.framework = Some(f.clone());
    }
    if let Some(l) = &opts.launcher {
        l.check_dir(&opts.dir.join(&l.base))
            .context("the launcher in the channel directory")?;
        channel.launcher = Some(l.clone());
    }
    channel.validate()?;
    Ok(channel)
}

/// Whether `path` names a theme file (`*.theme.json`).
pub(crate) fn is_theme_path(path: &Path) -> bool {
    path.file_name().is_some_and(|n| {
        n.to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".theme.json")
    })
}

/// The `*.wasm` and `*.theme.json` files in `dir`, sorted.
fn plugin_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry
            .with_context(|| format!("reading {}", dir.display()))?
            .path();
        let wasm = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("wasm"));
        if (wasm || is_theme_path(&path)) && path.is_file() {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// A plugin file's bytes and its checked manifest.
fn read_plugin(path: &Path) -> Result<(Vec<u8>, Manifest)> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let m = manifest::extract(&bytes)?.with_context(|| {
        format!(
            "{} has no rekindle manifest (build it with `rekindle build`)",
            path.display()
        )
    })?;
    m.validate()
        .with_context(|| format!("{}", path.display()))?;
    Ok((bytes, m))
}

/// Copy a listed file (and its `.sig`, if it has one) to
/// `dir/plugins/<sha256>.<ext>`, and return its entry.
fn host_file(dir: &Path, path: &Path, bytes: &[u8], m: Manifest) -> Result<ChannelPlugin> {
    let sha256 = signing::sha256_hex(bytes);
    let ext = if is_theme_path(path) {
        "theme.json"
    } else {
        "wasm"
    };
    let url = format!("plugins/{sha256}.{ext}");
    let hosted = dir.join(&url);
    std::fs::create_dir_all(dir.join("plugins")).context("creating plugins/")?;
    std::fs::write(&hosted, bytes).with_context(|| format!("writing {}", hosted.display()))?;
    let sig_path = signing::sig_path_for(path);
    if sig_path.exists() {
        std::fs::copy(&sig_path, signing::sig_path_for(&hosted))
            .with_context(|| format!("copying {}", sig_path.display()))?;
    }
    Ok(ChannelPlugin {
        id: m.id,
        name: m.name,
        version: m.version,
        description: m.description,
        wit: m.wit,
        sha256,
        url,
    })
}

/// Give each plugin version the previous channel at `previous` published
/// the file it published, fetched into `dir/plugins/` with its signature.
/// A hosted file never changes under a version, and TypeScript builds
/// differ by a few bytes each time, so a change ships only with a new
/// version: installs update by version. The new build's hosted file and its
/// signature are removed, since the channel no longer lists them. Returns
/// `id version` of each plugin whose new build differed from the published
/// file.
pub fn keep_published(
    dir: &Path,
    previous: &Location,
    published: &[ChannelPlugin],
    plugins: &mut [ChannelPlugin],
) -> Result<Vec<String>> {
    let mut kept = Vec::new();
    for p in plugins.iter_mut() {
        let Some(old) = published
            .iter()
            .find(|o| o.id == p.id && o.version == p.version)
        else {
            continue;
        };
        if old.sha256 == p.sha256 {
            continue;
        }
        if p.url != old.url {
            let unlisted = dir.join(&p.url);
            crate::home::remove_if_present(&signing::sig_path_for(&unlisted))?;
            crate::home::remove_if_present(&unlisted)?;
        }
        let to = dir.join(&old.url);
        if !to.exists() {
            std::fs::create_dir_all(dir.join("plugins")).context("creating plugins/")?;
            let from = previous.join(&old.url)?;
            std::fs::write(&to, from.read(MAX_PLUGIN_BYTES)?)
                .with_context(|| format!("copying {from}"))?;
        }
        let bytes = std::fs::read(&to).with_context(|| format!("reading {}", to.display()))?;
        if signing::sha256_hex(&bytes) != old.sha256 {
            bail!(
                "{} doesn't match the previous channel's listing",
                to.display()
            );
        }
        let sig_to = signing::sig_path_for(&to);
        if !sig_to.exists() {
            let from = previous.join(&old.url)?.with_suffix(".sig");
            std::fs::write(&sig_to, from.read(MAX_SIG_BYTES)?)
                .with_context(|| format!("copying {from}"))?;
        }
        kept.push(format!("{} {}", p.id, p.version));
        *p = old.clone();
    }
    Ok(kept)
}

/// Verify `bytes` against `sig` with a specific key, and parse the channel.
pub fn verify_with(bytes: &[u8], sig: &Signature, key: &PublicKey) -> Result<Channel> {
    key.verify_channel(bytes, sig)?;
    let channel: Channel =
        serde_json::from_slice(bytes).context("channel.json is not a valid channel")?;
    channel.validate()?;
    Ok(channel)
}

/// Verify against one of `keys` (normally [`signing::first_party_keys`]).
/// Returns the channel and the key that signed it.
pub fn verify(bytes: &[u8], sig: &Signature, keys: &[PublicKey]) -> Result<(Channel, PublicKey)> {
    sig.check_bytes(signing::SigKind::Channel, bytes)?;
    let Some(key) = keys.iter().find(|k| k.key_id() == sig.key_id) else {
        bail!(
            "channel is signed by key {}, which is not a project key",
            sig.key_id
        );
    };
    let channel = verify_with(bytes, sig, key)?;
    Ok((channel, key.clone()))
}

/// Read `channel.json` and `channel.json.sig` from a location.
pub fn fetch(location: &Location) -> Result<(Vec<u8>, Signature)> {
    let bytes = location.read(MAX_CHANNEL_BYTES)?;
    let sig_loc = location.with_suffix(".sig");
    let sig_bytes = sig_loc
        .read(MAX_SIG_BYTES)
        .with_context(|| format!("channel signature {sig_loc} is missing"))?;
    let sig = Signature::from_json(&sig_bytes).with_context(|| format!("parsing {sig_loc}"))?;
    Ok((bytes, sig))
}

/// Download a listed plugin and its signature from next to the channel at
/// `channel`, and check the bytes are the ones listed. Whether the
/// signature makes it first-party is the caller's check (`home`).
pub fn fetch_plugin(channel: &Location, entry: &ChannelPlugin) -> Result<(Vec<u8>, Signature)> {
    let (loc, bytes) = fetch_listed(channel, entry)?;
    let sig_loc = loc.with_suffix(".sig");
    let sig = Signature::from_json(&sig_loc.read(MAX_SIG_BYTES)?)
        .with_context(|| format!("parsing {sig_loc}"))?;
    Ok((bytes, sig))
}

/// Download a community plugin listed in the channel at `channel`, and check
/// the bytes are the ones listed. Its listing is what makes it a community
/// plugin, so there is no signature to fetch.
pub fn fetch_community(channel: &Location, entry: &CommunityPlugin) -> Result<Vec<u8>> {
    Ok(fetch_listed(channel, &entry.plugin)?.1)
}

/// Download `entry`'s file and check its sha256. Returns where it was
/// fetched from and its bytes.
fn fetch_listed(channel: &Location, entry: &ChannelPlugin) -> Result<(Location, Vec<u8>)> {
    let loc = channel.join(&entry.url)?;
    let bytes = loc.read(MAX_PLUGIN_BYTES)?;
    let actual = signing::sha256_hex(&bytes);
    if actual != entry.sha256 {
        bail!(
            "{loc} does not match the channel (sha256 {actual}, the channel lists {})",
            entry.sha256
        );
    }
    Ok((loc, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, version: &str, wit: &str, body: &[u8]) -> ChannelPlugin {
        ChannelPlugin {
            id: id.into(),
            name: id.into(),
            version: version.into(),
            description: String::new(),
            wit: wit.into(),
            sha256: signing::sha256_hex(body),
            url: format!("{id}-{version}.wasm"),
        }
    }

    #[test]
    fn channel_sig_roundtrip_and_tamper() {
        let ck = Keypair::from_seed([7; 32], "Channel");
        let other = Keypair::from_seed([8; 32], "Other");
        let mut channel = Channel::new("test", 5, 100);
        channel
            .plugins
            .push(entry("dev.example.a", "1.0.0", "0.8.0", b"a"));
        let (bytes, sig) = channel.sign(&ck);

        assert!(
            verify(&bytes, &sig, &[other.public()]).is_err(),
            "not signed by a listed key"
        );
        let (got, signer) = verify(&bytes, &sig, &[other.public(), ck.public()]).unwrap();
        assert_eq!((got, signer.key_id()), (channel, ck.key_id()));

        let mut tampered = bytes.clone();
        let pos = tampered.windows(5).position(|w| w == b"1.0.0").unwrap();
        tampered[pos] = b'9';
        assert!(verify(&tampered, &sig, &[ck.public()]).is_err());

        // A plugin signature over the same bytes is not a channel signature.
        assert!(verify(&bytes, &ck.sign_plugin(&bytes), &[ck.public()]).is_err());
    }

    #[test]
    fn a_published_version_keeps_its_published_file() {
        let root =
            std::env::temp_dir().join(format!("rekindle-keep-published-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (prev, dir) = (root.join("prev"), root.join("new"));
        std::fs::create_dir_all(&prev).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        let old = entry("dev.example.a", "1.0.0", "0.8.0", b"old build");
        std::fs::write(prev.join(&old.url), b"old build").unwrap();
        std::fs::write(prev.join(format!("{}.sig", old.url)), b"its signature").unwrap();
        let previous = Location::Path(prev.join("channel.json"));

        let mut new_build = entry("dev.example.a", "1.0.0", "0.8.0", b"new build");
        new_build.url = "new-build.wasm".into();
        std::fs::write(dir.join(&new_build.url), b"new build").unwrap();
        std::fs::write(dir.join("new-build.wasm.sig"), b"its signature").unwrap();
        let mut plugins = vec![new_build, entry("dev.example.b", "1.0.0", "0.8.0", b"b")];
        let kept =
            keep_published(&dir, &previous, std::slice::from_ref(&old), &mut plugins).unwrap();
        assert_eq!(kept, ["dev.example.a 1.0.0"]);
        assert!(!dir.join("new-build.wasm").exists() && !dir.join("new-build.wasm.sig").exists());
        assert_eq!(plugins[0], old);
        assert_eq!(std::fs::read(dir.join(&old.url)).unwrap(), b"old build");
        assert_eq!(
            std::fs::read(dir.join(format!("{}.sig", old.url))).unwrap(),
            b"its signature"
        );
        assert_eq!(
            plugins[1].sha256,
            signing::sha256_hex(b"b"),
            "not published before"
        );

        let mut bumped = vec![entry("dev.example.a", "1.0.1", "0.8.0", b"new build")];
        assert!(
            keep_published(&dir, &previous, std::slice::from_ref(&old), &mut bumped)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            bumped[0].sha256,
            signing::sha256_hex(b"new build"),
            "a new version ships the new build"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn latest_skips_incompatible_and_blocked_versions() {
        let mut c = Channel::new("test", 1, 100);
        c.plugins
            .push(entry("dev.example.a", "1.0.0", "0.8.0", b"1"));
        c.plugins
            .push(entry("dev.example.a", "1.10.0", "0.8.0", b"2"));
        c.plugins
            .push(entry("dev.example.a", "1.2.0", "0.8.0", b"3"));
        c.plugins
            .push(entry("dev.example.a", "2.0.0", "0.9.0", b"4"));
        c.validate().unwrap();
        // Semver order, not string order; 2.0.0 needs a newer host.
        assert_eq!(
            c.latest("dev.example.a", "0.8.0").unwrap().version,
            "1.10.0"
        );
        assert_eq!(c.latest("dev.example.a", "0.9.0").unwrap().version, "2.0.0");
        assert!(c.latest("dev.example.b", "0.8.0").is_none());

        c.blocked.push(Blocked {
            sha256: signing::sha256_hex(b"2"),
            reason: "broken".into(),
            id: None,
        });
        assert_eq!(c.latest("dev.example.a", "0.8.0").unwrap().version, "1.2.0");
        assert_eq!(c.blocked_reason(&signing::sha256_hex(b"2")), Some("broken"));
        assert!(c.is_expired(100) && !c.is_expired(99));

        c.plugins
            .push(entry("dev.example.a", "1.0.0", "0.8.0", b"5"));
        assert!(c.validate().is_err(), "duplicate version");
    }

    fn community(id: &str, version: &str, author: &str, body: &[u8]) -> CommunityPlugin {
        CommunityPlugin {
            plugin: entry(id, version, "0.8.0", body),
            author: author.into(),
            capabilities: vec![],
            network: vec![],
            source: Source {
                repo: "https://example.com/a.git".into(),
                commit: "a".repeat(40),
                path: String::new(),
            },
        }
    }

    #[test]
    fn the_community_index_lists_newest_versions_and_keeps_namespaces_apart() {
        let mut c = Channel::new("test", 1, 100);
        c.community
            .push(community("dev.alice.a", "1.0.0", "alice", b"1"));
        c.community
            .push(community("dev.alice.a", "1.1.0", "alice", b"2"));
        c.community
            .push(community("dev.alice.b", "0.1.0", "alice", b"3"));
        c.validate().unwrap();
        let store: Vec<(&str, &str)> = c
            .store("0.8.0")
            .iter()
            .map(|e| (e.plugin.id.as_str(), e.plugin.version.as_str()))
            .collect();
        assert_eq!(
            store,
            vec![("dev.alice.a", "1.1.0"), ("dev.alice.b", "0.1.0")]
        );
        assert_eq!(
            c.community_entry(&signing::sha256_hex(b"1"))
                .unwrap()
                .plugin
                .version,
            "1.0.0"
        );

        // A blocked version drops out of the store but stays listed.
        c.blocked.push(Blocked {
            sha256: signing::sha256_hex(b"2"),
            reason: "broken".into(),
            id: None,
        });
        assert_eq!(
            c.latest_community("dev.alice.a", "0.8.0")
                .unwrap()
                .plugin
                .version,
            "1.0.0"
        );
        assert!(c.community_entry(&signing::sha256_hex(b"2")).is_some());

        let mut bad = c.clone();
        bad.community
            .push(community("dev.alice.c", "1.0.0", "mallory", b"4"));
        assert!(bad.validate().is_err(), "dev.alice belongs to alice");
        let mut bad = c.clone();
        bad.community
            .push(community("dev.rekindle.x", "1.0.0", "mallory", b"5"));
        assert!(bad.validate().is_err(), "the first-party namespace");
        let mut bad = c.clone();
        bad.community
            .push(community("dev.alice.a", "1.0.0", "alice", b"6"));
        assert!(bad.validate().is_err(), "a version listed twice");
        let mut bad = c.clone();
        bad.community
            .push(community("io.github.bob.tools.x", "1.0.0", "mallory", b"7"));
        assert!(bad.validate().is_err(), "bob's GitHub namespace");
        assert!(
            is_first_party_id("dev.rekindle.kaomoji")
                && !is_first_party_id("dev.rekindler.kaomoji")
        );
    }

    #[test]
    fn wit_compatibility() {
        assert!(wit_compatible("0.8.0", "0.8.0"));
        assert!(wit_compatible("0.8.0", "0.8.3"));
        assert!(!wit_compatible("0.8.3", "0.8.0"), "newer than the host");
        assert!(!wit_compatible("0.7.0", "0.8.0"), "0.x minor is breaking");
        assert!(wit_compatible("1.2.0", "1.4.0"));
        assert!(!wit_compatible("1.5.0", "1.4.0"));
        assert!(!wit_compatible("2.0.0", "1.4.0"));
        assert!(!wit_compatible("nonsense", "0.8.0"));
    }
}
