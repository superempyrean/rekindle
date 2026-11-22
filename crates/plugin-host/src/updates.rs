//! Automatic updates for first-party and community plugins, and the
//! known-bad blocklist, from the release channel ([`crate::channel`]).
//!
//! Two halves, so the network never runs where plugins do:
//!
//! - [`check`] (any thread; no runtime): fetch the channel and accept it if
//!   it is signed by a project key, not older than one already accepted, and
//!   not expired. Then download each update, check it is the listed file,
//!   and install it on disk, keeping the version it replaces as
//!   `previous.wasm`.
//! - [`apply`] (the runtime's thread): swap updated plugins in, roll back
//!   any that don't start (and never offer that file again), and stop any
//!   running plugin the channel has blocked.
//!
//! [`Updater`] runs both checks on a background thread at start and every
//! [`CHECK_INTERVAL`]. There is no updater service: updates happen while
//! rekindle runs, because the user started it.
//!
//! A community plugin updates from the channel's community index. A newer
//! version that requests no capability or host the installed one didn't
//! installs like a first-party update. One that requests more is
//! downloaded as `plugins/<id>/update.wasm` for the user to review.
//!
//! Third-party plugins are never updated automatically: a changed file
//! needs the user's review. With developer mode on, [`check_reviews`] looks
//! at each one's own update source (`manifest.updates`: a channel signed by
//! the author's key) and downloads a newer version as
//! `plugins/<id>/update.wasm` for the user to review; the channel and the
//! file must be signed by the key the reviewed version names. The blocklist
//! applies to third-party plugins all the same, and to every later start,
//! from the cached channel, offline or not.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::capability::Trust;
use crate::channel::{self, Channel, ChannelPlugin, CommunityPlugin, Location};
use crate::home::{Home, InstallRecord, InstallState, Installed, remove_if_present};
use crate::manifest::{self, Manifest, UpdateSource};
use crate::review;
use crate::runtime::Runtime;
use crate::signing::{self, PublicKey, Signature};

/// How often a running rekindle checks the channel.
pub const CHECK_INTERVAL: Duration = Duration::from_hours(6);

/// Unix seconds now.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// One installed plugin moving to a newer version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub id: String,
    pub name: String,
    pub from: String,
    pub to: String,
    /// sha256 (hex) of the new version's file.
    pub sha256: String,
}

impl Update {
    fn new(from: &str, to: &ChannelPlugin) -> Self {
        Self {
            id: to.id.clone(),
            name: to.name.clone(),
            from: from.into(),
            to: to.version.clone(),
            sha256: to.sha256.clone(),
        }
    }
}

/// What [`check`] found and did.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Serial of the channel accepted.
    pub serial: u64,
    /// Installed on disk; [`apply`] swaps the running ones in.
    pub updated: Vec<Update>,
    /// Newer versions not installed because automatic updates are off.
    pub available: Vec<Update>,
    /// (id, why) for updates that couldn't be downloaded or installed.
    pub failed: Vec<(String, String)>,
    /// (id, reason) for installed plugins the channel blocks.
    pub blocked: Vec<(String, String)>,
    /// Updates downloaded for review: community ones that ask for more, and
    /// third-party ones ([`check_reviews`]).
    pub reviews: Vec<Update>,
    /// Checks that didn't run at all (offline, a bad channel).
    pub errors: Vec<String>,
}

impl Home {
    /// The channel to check: `$REKINDLE_CHANNEL`, else `config.json`'s
    /// `channel`, else the official one.
    pub fn channel_location(&self) -> Result<Option<Location>> {
        rekindle_release::channel_location(self.config()?.channel.as_deref())
    }

    /// Use another release channel than the official one (development and
    /// tests; `None` goes back to the official one).
    pub fn set_channel(&self, location: Option<String>) -> Result<()> {
        self.update_config(|c| c.channel = location)
    }

    /// The "Update automatically" setting (on unless turned off).
    pub fn set_manual_updates(&self, manual: bool) -> Result<()> {
        self.update_config(|c| c.manual_updates = manual)
    }

    fn channel_path(&self) -> PathBuf {
        self.root().join("channel").join("channel.json")
    }

    /// Where [`accept_channel`](Self::accept_channel) writes a channel
    /// before renaming it into place.
    fn channel_tmp_path(&self) -> PathBuf {
        self.channel_path().with_extension("json.tmp")
    }

    /// The last channel accepted, its signature checked again. Its blocklist
    /// applies even when it has expired or there is no network.
    pub fn cached_channel(&self) -> Option<Channel> {
        let path = self.channel_path();
        let sig = Signature::load(&signing::sig_path_for(&path)).ok()?;
        let read = |p: &Path| -> Option<Result<Channel>> {
            let bytes = std::fs::read(p).ok()?;
            Some(channel::verify(&bytes, &sig, self.first_party_keys()).map(|(c, _)| c))
        };
        let cached = read(&path);
        if let Some(Ok(c)) = cached {
            return Some(c);
        }
        // `accept_channel` saves the new signature before it renames the new
        // channel into place. After a crash between the two, the signature
        // belongs to the temporary file.
        let tmp = self.channel_tmp_path();
        if let Some(Ok(c)) = read(&tmp) {
            if let Err(e) = std::fs::rename(&tmp, &path) {
                tracing::warn!("moving {} into place: {e}", tmp.display());
            }
            return Some(c);
        }
        if let Some(Err(e)) = cached {
            tracing::warn!(
                "ignoring the cached release channel {}: {e:#}",
                path.display()
            );
        }
        None
    }

    /// Accept a fetched channel: signed by a project key, not older than one
    /// already accepted, not expired. It becomes the cached channel.
    pub fn accept_channel(&self, bytes: &[u8], sig: &Signature, now: u64) -> Result<Channel> {
        let (channel, _) = channel::verify(bytes, sig, self.first_party_keys())?;
        if channel.is_expired(now) {
            bail!(
                "the release channel expired (serial {}, expired at unix time {})",
                channel.serial,
                channel.expires
            );
        }
        let accepted = self.config()?.channel_serial;
        if channel.serial < accepted {
            bail!(
                "the release channel has serial {}, older than one already accepted ({accepted}); refusing to roll back",
                channel.serial
            );
        }
        let path = self.channel_path();
        if channel.serial == accepted
            && let Ok(cached) = std::fs::read(&path)
        {
            if cached == bytes {
                return Ok(channel);
            }
            bail!(
                "two different release channels have serial {}; keeping the first",
                channel.serial
            );
        }
        let dir = path.parent().expect("the channel path is in a directory");
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let tmp = self.channel_tmp_path();
        std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
        sig.save(&signing::sig_path_for(&path))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
        self.update_config(|c| c.channel_serial = channel.serial)?;
        Ok(channel)
    }

    fn review_path(&self, id: &str) -> PathBuf {
        self.plugin_dir(id).join("update.wasm")
    }

    /// Whether the file with this sha256 is already downloaded for review.
    fn review_waiting(&self, id: &str, sha256: &str) -> bool {
        std::fs::read(self.review_path(id)).is_ok_and(|b| signing::sha256_hex(&b) == sha256)
    }

    /// A downloaded update waiting for review, if it is still newer than the
    /// installed version. A community update must still be listed in the
    /// cached channel's index; a third-party one is checked again against
    /// the author's key. One that fails either check is discarded.
    pub fn pending_review(&self, id: &str) -> Result<Option<Waiting>> {
        let Ok(wasm) = std::fs::read(self.review_path(id)) else {
            return Ok(None);
        };
        let checked = self.check_review(id, wasm);
        if !matches!(checked, Ok(Some(_))) {
            self.discard_review(id);
        }
        checked.with_context(|| format!("the downloaded update of {id}"))
    }

    fn check_review(&self, id: &str, wasm: Vec<u8>) -> Result<Option<Waiting>> {
        let installed = self
            .get(id)?
            .context("not installed")?
            .manifest
            .context("the installed plugin has no manifest")?;
        let sig_path = signing::sig_path_for(&self.review_path(id));
        let sig = sig_path
            .exists()
            .then(|| Signature::load(&sig_path))
            .transpose()?;
        let listed = self
            .cached_channel()
            .is_some_and(|c| c.community_entry(&signing::sha256_hex(&wasm)).is_some());
        if !listed {
            let source = installed
                .updates
                .as_ref()
                .context("the installed version has no update source")?;
            let sig = sig.as_ref().context("the update has no signature")?;
            author_key(source, &installed.name)?
                .verify_plugin(&wasm, sig)
                .context("not signed by the author's key")?;
        }
        let manifest = manifest::extract(&wasm)?.context("not a rekindle plugin")?;
        let newer = manifest.id == id && is_newer(&manifest.version, &installed.version);
        Ok(newer.then_some(Waiting {
            wasm,
            sig,
            manifest,
        }))
    }

    /// Keep a downloaded update as `update.wasm` for the user to review.
    fn keep_for_review(&self, id: &str, wasm: &[u8], sig: Option<&Signature>) -> Result<()> {
        let path = self.review_path(id);
        let tmp = path.with_extension("wasm.tmp");
        std::fs::write(&tmp, wasm).with_context(|| format!("writing {}", tmp.display()))?;
        let sig_path = signing::sig_path_for(&path);
        match sig {
            Some(s) => s.save(&sig_path)?,
            None => remove_if_present(&sig_path)?,
        }
        std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))
    }

    /// Drop a downloaded update (installed, declined, or stale).
    pub fn discard_review(&self, id: &str) {
        let path = self.review_path(id);
        // A file left behind is checked again before it is offered
        // (`pending_review`), so a failed removal does no harm.
        let _ = std::fs::remove_file(signing::sig_path_for(&path));
        let _ = std::fs::remove_file(path);
    }

    fn previous_path(&self, id: &str) -> PathBuf {
        self.plugin_dir(id).join("previous.wasm")
    }

    /// Install an update over an installed plugin without a review, keeping
    /// the current file as `previous.wasm` for [`Home::roll_back`]. The
    /// update must be first-party, or a community plugin that asks for
    /// nothing the installed version didn't ([`review::asks_for_more`]).
    pub fn install_update(&self, wasm: &[u8], sig: Option<&Signature>) -> Result<InstallRecord> {
        let m = manifest::extract(wasm)?.context("not a rekindle plugin (no manifest)")?;
        match self.trust_of(wasm, sig)?.trust {
            Trust::FirstParty => {}
            Trust::Community => {
                let rec = self
                    .record(&m.id)?
                    .with_context(|| format!("{} is not installed", m.id))?;
                if review::asks_for_more(&rec, &m) {
                    bail!(
                        "the update for {} asks for more than the installed version; it needs a review",
                        m.id
                    );
                }
            }
            Trust::ThirdParty => bail!(
                "the update for {} is neither signed by a rekindle project key nor in the community index",
                m.id
            ),
        }
        let (current, previous) = (self.artifact_path(&m.id), self.previous_path(&m.id));
        if current.exists() {
            std::fs::copy(&current, &previous)
                .with_context(|| format!("keeping {}", previous.display()))?;
            let (current_sig, previous_sig) = (
                signing::sig_path_for(&current),
                signing::sig_path_for(&previous),
            );
            if current_sig.exists() {
                std::fs::copy(&current_sig, &previous_sig)
                    .with_context(|| format!("keeping {}", previous_sig.display()))?;
            } else {
                remove_if_present(&previous_sig)?;
            }
        }
        Ok(self.install(wasm, sig)?.1)
    }

    /// Put back the version an update replaced, after the update failed to
    /// start. The failed file is remembered and not offered again.
    pub fn roll_back(&self, id: &str) -> Result<()> {
        let inst = self
            .get(id)?
            .with_context(|| format!("{id} is not installed"))?;
        let failed = signing::sha256_hex(&inst.read_wasm()?);
        let previous = self.previous_path(id);
        let wasm = std::fs::read(&previous)
            .with_context(|| format!("{id} has no previous version to go back to"))?;
        let previous_sig = signing::sig_path_for(&previous);
        let sig = previous_sig
            .exists()
            .then(|| Signature::load(&previous_sig))
            .transpose()?;
        let (_, mut record) = self.install(&wasm, sig.as_ref())?;
        record.bad_updates.insert(failed);
        self.write_record(&record)?;
        // The previous version is installed again; a copy left behind is
        // replaced by the next update.
        let _ = std::fs::remove_file(&previous);
        let _ = std::fs::remove_file(&previous_sig);
        Ok(())
    }
}

/// A downloaded update waiting for review ([`Home::pending_review`]).
pub struct Waiting {
    pub wasm: Vec<u8>,
    /// The author's signature, for a third-party update.
    pub sig: Option<Signature>,
    pub manifest: Manifest,
}

/// Whether `candidate` is a newer version than `installed` (an installed
/// version that isn't semver counts as older).
fn is_newer(candidate: &str, installed: &str) -> bool {
    match (
        semver::Version::parse(candidate),
        semver::Version::parse(installed),
    ) {
        (Ok(c), Ok(i)) => c > i,
        (Ok(_), Err(_)) => true,
        (Err(_), _) => false,
    }
}

/// A newer version of an installed plugin, from the channel.
pub enum Listed<'c> {
    FirstParty(&'c ChannelPlugin),
    Community(&'c CommunityPlugin),
}

impl Listed<'_> {
    /// The channel's entry for the file.
    pub fn plugin(&self) -> &ChannelPlugin {
        match self {
            Listed::FirstParty(p) => p,
            Listed::Community(c) => &c.plugin,
        }
    }
}

/// Installed first-party and community plugins the channel has a newer
/// version of.
pub fn pending<'c>(home: &Home, channel: &'c Channel) -> Result<Vec<(Installed, Listed<'c>)>> {
    let host_wit = manifest::rekindle_wit_version();
    let mut out = Vec::new();
    for inst in home.installed()? {
        let (Some(t), Some(m)) = (&inst.trust, &inst.manifest) else {
            continue;
        };
        if matches!(
            inst.state,
            InstallState::Refused(_) | InstallState::Blocked(_) | InstallState::NeedsReview(_)
        ) {
            continue;
        }
        let listed = match t.trust {
            Trust::FirstParty => channel.latest(&inst.id, host_wit).map(Listed::FirstParty),
            Trust::Community => channel
                .latest_community(&inst.id, host_wit)
                .map(Listed::Community),
            Trust::ThirdParty => None,
        };
        let Some(listed) = listed else { continue };
        let entry = listed.plugin();
        let bad = inst
            .record
            .as_ref()
            .is_some_and(|r| r.bad_updates.contains(&entry.sha256));
        if entry.sha256 != t.sha256 && !bad && is_newer(&entry.version, &m.version) {
            out.push((inst, listed));
        }
    }
    Ok(out)
}

/// The manifest of a downloaded file, checked to be the id and version the
/// channel lists for it.
fn listed_manifest(wasm: &[u8], entry: &ChannelPlugin) -> Result<Manifest> {
    let m = manifest::extract(wasm)?.context("the download is not a rekindle plugin")?;
    m.validate()?;
    if (m.id.as_str(), m.version.as_str()) != (entry.id.as_str(), entry.version.as_str()) {
        bail!(
            "the download is {} {}, but the channel lists {} {}",
            m.id,
            m.version,
            entry.id,
            entry.version
        );
    }
    Ok(m)
}

/// What [`download`] did with an update.
enum Downloaded {
    Installed,
    /// A community update that asks for more: waiting for review.
    ForReview,
}

/// Download a listed update and install it on disk, or keep it for review.
fn download(home: &Home, location: &Location, listed: &Listed) -> Result<Downloaded> {
    let (wasm, sig) = match listed {
        Listed::FirstParty(p) => channel::fetch_plugin(location, p).map(|(w, s)| (w, Some(s)))?,
        Listed::Community(c) => (channel::fetch_community(location, c)?, None),
    };
    let m = listed_manifest(&wasm, listed.plugin())?;
    if let Listed::Community(_) = listed {
        let rec = home
            .record(&m.id)?
            .with_context(|| format!("{} is not installed", m.id))?;
        if review::asks_for_more(&rec, &m) {
            home.keep_for_review(&m.id, &wasm, None)?;
            return Ok(Downloaded::ForReview);
        }
    }
    home.install_update(&wasm, sig.as_ref())?;
    Ok(Downloaded::Installed)
}

/// Check the channel at `location` and install first-party and community
/// updates on disk (unless automatic updates are off). Touches no runtime:
/// run it anywhere.
pub fn check(home: &Home, location: &Location, now: u64) -> Result<Report> {
    let (bytes, sig) = channel::fetch(location)?;
    let channel = home
        .accept_channel(&bytes, &sig, now)
        .with_context(|| format!("release channel {location}"))?;
    let manual = home.config()?.manual_updates;
    let mut report = Report {
        serial: channel.serial,
        ..Default::default()
    };
    for (inst, listed) in pending(home, &channel)? {
        let entry = listed.plugin();
        let from = inst.manifest.as_ref().map_or("", |m| m.version.as_str());
        let update = Update::new(from, entry);
        if home.review_waiting(&inst.id, &entry.sha256) {
            report.reviews.push(update);
            continue;
        }
        if manual {
            report.available.push(update);
            continue;
        }
        match download(home, location, &listed) {
            Ok(Downloaded::Installed) => report.updated.push(update),
            Ok(Downloaded::ForReview) => report.reviews.push(update),
            Err(e) => report.failed.push((update.id, format!("{e:#}"))),
        }
    }
    for inst in home.installed()? {
        if let InstallState::Blocked(why) = inst.state {
            report.blocked.push((inst.id, why));
        }
    }
    Ok(report)
}

fn author_key(source: &UpdateSource, name: &str) -> Result<PublicKey> {
    PublicKey::from_base64(&source.key, name).context("the plugin's update key is invalid")
}

/// With developer mode on, check each third-party plugin's own update
/// source and download newer versions for review (never installing them).
/// Contacts only the hosts the plugins' install screens named.
pub fn check_reviews(home: &Home, now: u64) -> Result<Reviews> {
    check_reviews_with(home, now, Location::parse)
}

/// [`check_reviews`], with the update URL mapped to a location by `locate`
/// (tests serve a local directory).
pub fn check_reviews_with(
    home: &Home,
    now: u64,
    locate: impl Fn(&str) -> Result<Location>,
) -> Result<Reviews> {
    let mut out = Reviews::default();
    if !home.developer_mode()? {
        return Ok(out);
    }
    let blocklist = home.cached_channel();
    for inst in home.installed()? {
        let (Some(t), Some(m), Some(rec)) = (&inst.trust, &inst.manifest, &inst.record) else {
            continue;
        };
        let reviewable = t.trust == Trust::ThirdParty
            && matches!(inst.state, InstallState::Ready | InstallState::Off);
        let Some(source) = m.updates.as_ref().filter(|_| reviewable) else {
            continue;
        };
        let checked = locate(&source.url)
            .and_then(|loc| review_update(home, &inst.id, m, rec, &loc, blocklist.as_ref(), now));
        match checked {
            Ok(Some(u)) => out.ready.push(u),
            Ok(None) => home.discard_review(&inst.id),
            Err(e) => out.failed.push((inst.id.clone(), format!("{e:#}"))),
        }
    }
    Ok(out)
}

/// Check the author channel at `loc` of the installed plugin `id` (manifest
/// `m`, which names the channel's key) for a newer version, and keep it for
/// review unless it is waiting already. `None`: nothing newer that isn't
/// blocked or known bad.
fn review_update(
    home: &Home,
    id: &str,
    m: &Manifest,
    rec: &InstallRecord,
    loc: &Location,
    blocklist: Option<&Channel>,
    now: u64,
) -> Result<Option<Update>> {
    let source = m
        .updates
        .as_ref()
        .context("the plugin names no update source")?;
    let key = author_key(source, &m.name)?;
    let ch = author_channel(home, rec, loc, &key, now)
        .with_context(|| format!("{}'s update channel", m.name))?;
    let Some(entry) = ch.latest(id, manifest::rekindle_wit_version()) else {
        return Ok(None);
    };
    let blocked = blocklist.is_some_and(|c| c.blocked_reason(&entry.sha256).is_some());
    if !is_newer(&entry.version, &m.version) || rec.bad_updates.contains(&entry.sha256) || blocked {
        return Ok(None);
    }
    if !home.review_waiting(id, &entry.sha256) {
        let (wasm, sig) = channel::fetch_plugin(loc, entry)?;
        key.verify_plugin(&wasm, &sig)
            .with_context(|| format!("{} is not signed by the author's key", entry.url))?;
        listed_manifest(&wasm, entry)?;
        home.keep_for_review(id, &wasm, Some(&sig))?;
    }
    Ok(Some(Update::new(&m.version, entry)))
}

/// Fetch a third-party plugin's update channel from `loc`: signed by `key`
/// (the key the reviewed version names), not expired, and not older than
/// one seen before. Its serial is recorded in the plugin's install record.
fn author_channel(
    home: &Home,
    rec: &InstallRecord,
    loc: &Location,
    key: &PublicKey,
    now: u64,
) -> Result<Channel> {
    let (bytes, sig) = channel::fetch(loc)?;
    let ch = channel::verify_with(&bytes, &sig, key)
        .context("it isn't signed by the key its install screen named")?;
    if ch.is_expired(now) {
        bail!("it expired");
    }
    if ch.serial < rec.update_serial {
        bail!(
            "it has serial {}, older than one already seen ({}); refusing to roll back",
            ch.serial,
            rec.update_serial
        );
    }
    if ch.serial > rec.update_serial {
        let mut r = rec.clone();
        r.update_serial = ch.serial;
        home.write_record(&r)?;
    }
    Ok(ch)
}

/// What [`check_reviews`] found.
#[derive(Debug, Clone, Default)]
pub struct Reviews {
    /// Downloaded and waiting for the user's review.
    pub ready: Vec<Update>,
    /// (id, why) for update sources that couldn't be checked.
    pub failed: Vec<(String, String)>,
}

/// What [`apply`] did to one plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Running the new version now (or will, the next time it's turned on).
    Updated { id: String, to: String },
    /// The new version didn't start; the previous one is back, on disk and
    /// (if it was running) in the runtime.
    RolledBack { id: String, to: String, why: String },
    /// Blocked by the channel and stopped.
    Stopped { id: String, why: String },
}

/// Bring a running runtime in line with a [`check`]: swap updated plugins
/// in (rolling back any that don't start) and stop blocked ones.
pub fn apply(home: &Home, rt: &mut Runtime, report: &Report) -> Vec<Outcome> {
    let mut out = Vec::new();
    for u in &report.updated {
        let updated = Outcome::Updated {
            id: u.id.clone(),
            to: u.to.clone(),
        };
        if rt.plugin(&u.id).is_none() {
            out.push(updated);
            continue;
        }
        let started = home
            .get(&u.id)
            .and_then(|i| i.context("it is no longer installed"))
            .and_then(|i| {
                let (wasm, trust) = i.read_verified()?;
                rt.reload(&wasm, trust)
            });
        match started {
            Ok(_) => out.push(updated),
            Err(e) => {
                let mut why = format!("{e:#}");
                if let Err(e) = home.roll_back(&u.id) {
                    why = format!("{why}; and going back to the previous version failed: {e:#}");
                }
                out.push(Outcome::RolledBack {
                    id: u.id.clone(),
                    to: u.to.clone(),
                    why,
                });
            }
        }
    }
    for (id, why) in &report.blocked {
        // `unload` fails only for a plugin that isn't loaded: nothing to stop.
        if rt.unload(id).is_ok() {
            out.push(Outcome::Stopped {
                id: id.clone(),
                why: why.clone(),
            });
        }
    }
    out
}

/// One round of checks: the release channel at `location` (if any), then
/// third-party update sources (if developer mode is on). A check that
/// can't run is in [`Report::errors`]; the other still runs.
pub fn check_all(home: &Home, location: Option<&Location>, now: u64) -> Report {
    let mut report = match location.map(|l| check(home, l, now)) {
        Some(Ok(r)) => r,
        Some(Err(e)) => Report {
            errors: vec![format!("{e:#}")],
            ..Default::default()
        },
        None => Report::default(),
    };
    match check_reviews(home, now) {
        Ok(r) => {
            report.reviews.extend(r.ready);
            report.failed.extend(r.failed);
        }
        Err(e) => report.errors.push(format!("{e:#}")),
    }
    report
}

/// Runs [`check_all`] on a background thread: now, then every `interval`,
/// and on [`Updater::check_now`]. Hand each result to [`apply`] on the
/// runtime's thread. Dropping it stops the checks (a download in progress
/// finishes first). With no release channel and developer mode off, a
/// round contacts nothing.
pub struct Updater {
    results: mpsc::Receiver<Report>,
    wake: mpsc::Sender<()>,
}

impl Updater {
    /// Start checking on a thread of its own, now and then every `interval`.
    /// Fails only if the thread can't be started.
    pub fn start(
        home: Home,
        location: Option<Location>,
        interval: Duration,
    ) -> std::io::Result<Self> {
        let (tx, results) = mpsc::channel();
        let (wake, woken) = mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("rekindle-updates".into())
            .spawn(move || {
                while tx.send(check_all(&home, location.as_ref(), now())).is_ok() {
                    if let Err(mpsc::RecvTimeoutError::Disconnected) = woken.recv_timeout(interval)
                    {
                        return;
                    }
                }
            })?;
        Ok(Self { results, wake })
    }

    /// Check again now (the user asked).
    pub fn check_now(&self) {
        // The thread only stops once `self` is dropped, or if a check
        // panicked; there is nothing to wake then.
        let _ = self.wake.send(());
    }

    /// A finished check, if there is one.
    pub fn try_recv(&self) -> Option<Report> {
        self.results.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_is_semver() {
        assert!(is_newer("1.10.0", "1.9.0"));
        assert!(!is_newer("1.9.0", "1.10.0"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(is_newer("1.0.0", "not-semver"));
        assert!(!is_newer("not-semver", "1.0.0"));
    }
}
