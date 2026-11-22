//! The install: where rekindle lives on disk, and which of its versions to
//! start. The launcher, the shim and the host all read and write it.
//!
//! ```text
//! %LOCALAPPDATA%\rekindle\        ($REKINDLE_INSTALL_ROOT overrides)
//!   rekindle.exe                  this program
//!   rekindle.exe.old              the program it replaced, until the next run deletes it
//!   launcher-update\              a newer program while it downloads and is checked
//!   state.json                    State
//!   versions\<version>\           rekindle's own files (the host library, recovered data)
//!   launcher.log                  what the last launches did
//!   launcher.log.old              the log before it, once it passed 1 MB
//! ```
//!
//! The user's plugins and their settings are not here: they live in the
//! plugin home (`~/.rekindle`), which uninstalling keeps unless asked.
//!
//! Versions: a new one is downloaded while rekindle runs and becomes
//! `pending`; the next launch tries it first. A version that starts becomes
//! `current` (the one it replaced stays as `previous`). A version that has
//! never started and reports that it can't is marked bad and never tried
//! again; the launch falls back to the one before it. A check with no report
//! at all (the client ended first, or timed out) can be the machine at that
//! moment, so such a version is marked bad only after [`NO_REPORT_LIMIT`]
//! checks in a row. A version that has started before and fails now is kept
//! (the client most likely changed, which an older rekindle won't fix), and
//! the launch goes on to the next candidate.
//!
//! On a launch of the client, the shim picks the version ([`State::launch`])
//! and records it as `trying`; the host records whether it came up
//! ([`State::reported`]). A version still `trying` at the next launch never
//! reported, because the client crashed or hung while it started, so it
//! counts as a version that can't start.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Environment variable overriding the install directory.
pub const ROOT_ENV: &str = "REKINDLE_INSTALL_ROOT";
/// The installed launcher's file name.
pub const EXE_NAME: &str = "rekindle.exe";
/// Checks in a row with no report before a version that never started is
/// marked bad.
pub const NO_REPORT_LIMIT: u32 = 3;
/// The host library in a version's folder.
pub const HOST_DLL: &str = "rekindle_host.dll";

/// The environment variables the launcher, the shim and the host pass each
/// other. Each is read by name in more than one of them.
pub mod env {
    /// Set by the shim for the host: the version it loaded from the
    /// install, by [`State::launch`](crate::State::launch). The host reports
    /// to the install only when it is set.
    pub const HOST_VERSION: &str = "REKINDLE_HOST_VERSION";
    /// `1` on a real launch: the host loads the user's installed plugins
    /// and stays attached. The shim sets it unless the self-test runs.
    pub const INSTALLED: &str = "REKINDLE_HOST_INSTALLED";
    /// The host's `layout.json`. The shim points this and the two below at
    /// the files in the version's folder, unless they are set already.
    pub const LAYOUT: &str = "REKINDLE_HOST_LAYOUT";
    /// The host's `anchors.json`.
    pub const ANCHORS: &str = "REKINDLE_HOST_ANCHORS";
    /// The host's `palette.json`.
    pub const PALETTE: &str = "REKINDLE_HOST_PALETTE";
    /// The host's `wire.json`, for a build whose UI crosses a wire: where
    /// its guest puts the slots plugins add to. Optional, so the shim sets
    /// it only when the version's folder holds one.
    pub const WIRE_MAP: &str = "REKINDLE_HOST_WIRE_MAP";
    /// The file the host writes its one-line report to.
    pub const HANDSHAKE: &str = "REKINDLE_ATTACH_HANDSHAKE";
    /// The folder the shim loads the host and its data from, in place of the
    /// one `state.json` names.
    pub const HOST_DIR: &str = "REKINDLE_PROXY_DIR";
    /// Turns a launch into the launcher's check: the shim keeps the client
    /// off the desktop, copies the host's report to this file, and ends the
    /// client.
    pub const CHECK: &str = "REKINDLE_PROXY_DRIVE_PROVE";
    /// The file the shim appends each step it takes to, so a check without a
    /// report can say how far the shim got.
    pub const TRACE: &str = "REKINDLE_PROXY_TRACE";
    /// The self-test's drive mode: the shim keeps the client off the desktop
    /// and the host drives rekindle's UI from inside.
    pub const DRIVE: &str = "REKINDLE_HOST_DRIVE";
    /// The file the host writes what its first update check did to. A check
    /// waits for it.
    pub const UPDATED: &str = "REKINDLE_HOST_UPDATED";
}

/// `state.json`: the versions on disk, which one to start, and what the
/// install has learned about each.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default = "one")]
    pub format: u32,
    /// The version to start.
    pub current: Option<String>,
    /// The version `current` replaced, kept to fall back to.
    pub previous: Option<String>,
    /// Downloaded, not tried yet: the next launch tries it first.
    pub pending: Option<String>,
    /// Versions that have started at least once.
    #[serde(default)]
    pub healthy: BTreeSet<String>,
    /// Versions that never started: never tried or downloaded again.
    #[serde(default)]
    pub bad: BTreeSet<String>,
    /// Checks in a row that ended with no report, for versions that never
    /// started.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub no_report: BTreeMap<String, u32>,
    /// Highest release channel serial accepted (a lower one is a rollback).
    #[serde(default)]
    pub channel_serial: u64,
    /// The client program rekindle starts.
    pub client: Option<PathBuf>,
    /// The version of the installed `rekindle.exe`, recorded when the setup
    /// or an update writes it. Setups before 0.2.0 didn't record it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launcher: Option<String>,
    /// Launcher versions that didn't start when checked: never downloaded
    /// again.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub bad_launchers: BTreeSet<String>,
    /// The version the shim loaded on the last launch of the client, until
    /// the host reports whether it came up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trying: Option<String>,
}

fn one() -> u32 {
    1
}

impl State {
    /// The versions to try this launch, best first.
    pub fn candidates(&self) -> Vec<String> {
        let mut out = Vec::new();
        for v in [&self.pending, &self.current, &self.previous]
            .into_iter()
            .flatten()
        {
            if !self.bad.contains(v) && !out.contains(v) {
                out.push(v.clone());
            }
        }
        out
    }

    /// `v` started.
    pub fn started(&mut self, v: &str) {
        self.healthy.insert(v.to_string());
        self.no_report.remove(v);
        if self.pending.as_deref() == Some(v) {
            self.pending = None;
        }
        if self.current.as_deref() != Some(v) {
            self.previous = self.current.take();
            self.current = Some(v.to_string());
        }
    }

    /// `v` reported that it can't start. Returns whether it was marked bad
    /// (it had never started before).
    pub fn failed(&mut self, v: &str) -> bool {
        if self.healthy.contains(v) {
            return false;
        }
        self.no_report.remove(v);
        self.bad.insert(v.to_string());
        if self.pending.as_deref() == Some(v) {
            self.pending = None;
        }
        if self.current.as_deref() == Some(v) {
            self.current = self.previous.take();
        }
        if self.previous.as_deref() == Some(v) {
            self.previous = None;
        }
        true
    }

    /// `v`'s check ended with no report. It stays where it is, to be tried
    /// again next time, until [`NO_REPORT_LIMIT`] such checks in a row mark
    /// it bad. Returns whether it was marked bad.
    pub fn unreported(&mut self, v: &str) -> bool {
        if self.healthy.contains(v) {
            return false;
        }
        let n = self.no_report.entry(v.to_string()).or_insert(0);
        *n += 1;
        if *n < NO_REPORT_LIMIT {
            return false;
        }
        self.failed(v)
    }

    /// The version a launch of the client loads: the best candidate that
    /// `present` says is on disk. A version the last launch loaded that never
    /// reported is [`State::failed`] first. Records the choice as `trying`;
    /// the caller saves the state.
    pub fn launch(&mut self, present: impl Fn(&str) -> bool) -> Option<String> {
        if let Some(v) = self.trying.take() {
            self.failed(&v);
        }
        let v = self.candidates().into_iter().find(|v| present(v))?;
        self.trying = Some(v.clone());
        Some(v)
    }

    /// The host of `v` reported: it came up (`ok`) or it couldn't.
    pub fn reported(&mut self, v: &str, ok: bool) {
        if self.trying.as_deref() == Some(v) {
            self.trying = None;
        }
        if ok {
            self.started(v);
        } else {
            self.failed(v);
        }
    }

    /// A version is offered by the channel: should it be downloaded?
    pub fn wants(&self, v: &str) -> bool {
        if self.bad.contains(v) || self.pending.as_deref() == Some(v) {
            return false;
        }
        let newest = [&self.current, &self.pending]
            .into_iter()
            .flatten()
            .filter_map(|c| semver::Version::parse(c).ok())
            .max();
        match (semver::Version::parse(v), newest) {
            (Ok(v), Some(n)) => v > n,
            (Ok(_), None) => true,
            (Err(_), _) => false,
        }
    }

    /// Whether the installed launcher is newer than `v`. An unrecorded one is
    /// older than any.
    pub fn launcher_newer_than(&self, v: &str) -> bool {
        match (
            self.launcher.as_deref().map(semver::Version::parse),
            semver::Version::parse(v),
        ) {
            (Some(Ok(installed)), Ok(v)) => installed > v,
            _ => false,
        }
    }

    /// The channel lists launcher `v`: should it replace the installed one?
    pub fn wants_launcher(&self, v: &str) -> bool {
        let Ok(parsed) = semver::Version::parse(v) else {
            return false;
        };
        !self.bad_launchers.contains(v)
            && self
                .launcher
                .as_deref()
                .and_then(|l| semver::Version::parse(l).ok())
                .is_none_or(|l| parsed > l)
    }

    /// The versions to keep on disk.
    pub fn keep(&self) -> BTreeSet<String> {
        [&self.current, &self.previous, &self.pending, &self.trying]
            .into_iter()
            .flatten()
            .cloned()
            .collect()
    }

    /// Whether `client` is the client this state records, so its shim loads
    /// rekindle from this install. Paths compare case-insensitively, as
    /// given or canonicalized.
    pub fn records_client(&self, client: &Path) -> bool {
        let Some(recorded) = &self.client else {
            return false;
        };
        let same = |a: &Path, b: &Path| {
            a.to_string_lossy()
                .eq_ignore_ascii_case(&b.to_string_lossy())
        };
        same(recorded, client)
            || matches!((std::fs::canonicalize(recorded), std::fs::canonicalize(client)), (Ok(a), Ok(b)) if same(&a, &b))
    }
}

/// `Ok` if a host report says rekindle came up, else why it didn't.
pub fn healthy(report: &str) -> Result<()> {
    let report = report.trim();
    if report.is_empty() {
        anyhow::bail!("rekindle didn't report from inside the client");
    }
    // The host ends its report with `error=` and the whole message, spaces
    // included. `surfaces=ERR(..)` is one token.
    if let Some(i) = report.find(" error=") {
        anyhow::bail!(
            "rekindle couldn't start inside the client: {}",
            &report[i + 1..]
        );
    }
    if let Some(e) = report
        .split_whitespace()
        .find(|t| t.starts_with("surfaces=ERR"))
    {
        anyhow::bail!("rekindle couldn't start inside the client: {e}");
    }
    Ok(())
}

/// An install directory.
#[derive(Debug, Clone)]
pub struct Install {
    root: PathBuf,
}

impl Install {
    /// The install at `root`, which need not exist yet.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `$REKINDLE_INSTALL_ROOT`, else `%LOCALAPPDATA%\rekindle`, else (where
    /// there is no `LOCALAPPDATA`) `$HOME/.local/share/rekindle`.
    pub fn default_root() -> Result<PathBuf> {
        if let Some(r) = std::env::var_os(ROOT_ENV).filter(|r| !r.is_empty()) {
            return Ok(PathBuf::from(r));
        }
        let base = std::env::var_os("LOCALAPPDATA")
            .filter(|b| !b.is_empty())
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|h| !h.is_empty())
                    .map(|h| PathBuf::from(h).join(".local/share").into_os_string())
            })
            .context("cannot find your local app data folder; set REKINDLE_INSTALL_ROOT")?;
        Ok(PathBuf::from(base).join("rekindle"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The installed launcher, `rekindle.exe`.
    pub fn exe(&self) -> PathBuf {
        self.root.join(EXE_NAME)
    }

    /// The program `exe` replaced, kept until it isn't running.
    pub fn old_exe(&self) -> PathBuf {
        self.root.join(format!("{EXE_NAME}.old"))
    }

    /// Where a newer program downloads and is checked.
    pub fn launcher_update_dir(&self) -> PathBuf {
        self.root.join("launcher-update")
    }

    /// The folder holding one folder per version.
    pub fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    /// Version `v`'s files.
    pub fn version_dir(&self, v: &str) -> PathBuf {
        self.versions_dir().join(v)
    }

    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    pub fn log_path(&self) -> PathBuf {
        self.root.join("launcher.log")
    }

    /// The log [`Install::log`] moved aside when it passed 1 MB.
    pub fn old_log_path(&self) -> PathBuf {
        self.root.join("launcher.log.old")
    }

    /// Whether rekindle is installed here.
    pub fn is_installed(&self) -> bool {
        self.state_path().exists()
    }

    /// The install's state, or a fresh one if there is no `state.json`.
    pub fn state(&self) -> Result<State> {
        let path = self.state_path();
        match std::fs::read(&path) {
            Ok(b) => {
                serde_json::from_slice(&b).with_context(|| format!("parsing {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State {
                format: 1,
                ..Default::default()
            }),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Write `state.json`, through a temporary file so a crash leaves the
    /// old one whole.
    pub fn save(&self, state: &State) -> Result<()> {
        std::fs::create_dir_all(&self.root)
            .with_context(|| format!("creating {}", self.root.display()))?;
        let path = self.state_path();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(state)? + "\n")
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))
    }

    /// Remove versions no longer kept, leftovers of interrupted downloads,
    /// and the program an update replaced once it isn't running.
    pub fn gc(&self, state: &State) {
        // Best effort throughout: what can't be removed now (a program
        // still running, a file in use) is tried again on the next run.
        let _ = std::fs::remove_file(self.old_exe());
        let _ = std::fs::remove_dir_all(self.launcher_update_dir());
        let keep = state.keep();
        let Ok(rd) = std::fs::read_dir(self.versions_dir()) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !keep.contains(&name) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }

    /// Append a line to the launcher log, moving it to
    /// [`Install::old_log_path`] once it passes 1 MB. The log is
    /// diagnostics, so a failure to write it is ignored.
    pub fn log(&self, line: &str) {
        let path = self.log_path();
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
            let _ = std::fs::rename(&path, self.old_log_path());
        }
        let _ = std::fs::create_dir_all(&self.root);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write as _;
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let _ = writeln!(f, "{secs} {line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(v: &str) -> State {
        State {
            format: 1,
            current: Some(v.into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_pending_version_is_tried_first_and_replaces_current_when_it_starts() {
        let mut s = installed("1.0.0");
        s.started("1.0.0");
        assert!(s.wants("1.1.0") && !s.wants("1.0.0") && !s.wants("0.9.0"));
        s.pending = Some("1.1.0".into());
        assert!(!s.wants("1.1.0"), "already downloaded");
        assert_eq!(s.candidates(), ["1.1.0", "1.0.0"]);
        s.started("1.1.0");
        assert_eq!(
            (
                s.current.as_deref(),
                s.previous.as_deref(),
                s.pending.as_deref()
            ),
            (Some("1.1.0"), Some("1.0.0"), None)
        );
        assert_eq!(s.keep().into_iter().collect::<Vec<_>>(), ["1.0.0", "1.1.0"]);
    }

    #[test]
    fn a_new_version_that_fails_is_bad_and_the_launch_falls_back() {
        let mut s = installed("1.0.0");
        s.started("1.0.0");
        s.pending = Some("1.1.0".into());
        assert!(s.failed("1.1.0"));
        assert_eq!(s.candidates(), ["1.0.0"]);
        assert!(!s.wants("1.1.0"), "never downloaded again");
        assert!(s.wants("1.1.1"), "a fixed release is");
    }

    #[test]
    fn a_version_that_started_before_is_not_marked_bad() {
        let mut s = installed("1.0.0");
        s.started("1.0.0");
        s.pending = Some("1.1.0".into());
        s.started("1.1.0");
        assert!(
            !s.failed("1.1.0"),
            "it worked before: the client changed, not rekindle"
        );
        assert_eq!(s.candidates(), ["1.1.0", "1.0.0"]);
    }

    #[test]
    fn a_check_with_no_report_is_tried_again_before_the_version_is_bad() {
        let mut s = installed("1.0.0");
        for _ in 1..NO_REPORT_LIMIT {
            assert!(!s.unreported("1.0.0"));
            assert_eq!(s.candidates(), ["1.0.0"], "still tried next time");
        }
        assert!(s.unreported("1.0.0"));
        assert!(s.candidates().is_empty() && s.no_report.is_empty());
        assert!(!s.wants("1.0.0"));

        let mut s = installed("1.0.0");
        s.unreported("1.0.0");
        s.unreported("1.0.0");
        s.started("1.0.0");
        assert!(s.no_report.is_empty(), "the count is of checks in a row");
        assert!(!s.unreported("1.0.0"), "it started before");
    }

    #[test]
    fn a_launcher_replaces_an_older_one_and_never_a_bad_one() {
        let mut s = installed("1.0.0");
        assert!(
            s.wants_launcher("0.2.0") && !s.launcher_newer_than("0.2.0"),
            "an unrecorded launcher is older than any"
        );
        s.launcher = Some("0.2.0".into());
        assert!(
            !s.wants_launcher("0.2.0") && !s.wants_launcher("0.1.9") && s.wants_launcher("0.3.0")
        );
        assert!(s.launcher_newer_than("0.1.9") && !s.launcher_newer_than("0.2.0"));
        s.bad_launchers.insert("0.3.0".into());
        assert!(!s.wants_launcher("0.3.0") && s.wants_launcher("0.3.1") && !s.wants_launcher("x"));
    }

    #[test]
    fn a_launch_tries_the_new_version_and_falls_back_when_it_never_reports() {
        let mut s = installed("1.0.0");
        s.started("1.0.0");
        s.pending = Some("1.1.0".into());
        assert_eq!(s.launch(|_| true).as_deref(), Some("1.1.0"));
        assert_eq!(s.keep().into_iter().collect::<Vec<_>>(), ["1.0.0", "1.1.0"]);
        // The client crashed while 1.1.0 started: the next launch drops it.
        assert_eq!(s.launch(|_| true).as_deref(), Some("1.0.0"));
        assert!(s.bad.contains("1.1.0") && s.pending.is_none());
        s.reported("1.0.0", true);
        assert_eq!(
            (s.trying.as_deref(), s.current.as_deref()),
            (None, Some("1.0.0"))
        );

        // A version that came up becomes current.
        s.pending = Some("1.2.0".into());
        assert_eq!(s.launch(|_| true).as_deref(), Some("1.2.0"));
        s.reported("1.2.0", true);
        assert_eq!(
            (
                s.current.as_deref(),
                s.previous.as_deref(),
                s.pending.as_deref()
            ),
            (Some("1.2.0"), Some("1.0.0"), None)
        );
        assert_eq!(
            s.launch(|v| v != "1.2.0").as_deref(),
            Some("1.0.0"),
            "a missing folder is skipped"
        );

        // One that came up before and doesn't now is kept.
        let mut s = installed("1.0.0");
        s.started("1.0.0");
        s.launch(|_| true);
        s.reported("1.0.0", false);
        assert_eq!(s.launch(|_| true).as_deref(), Some("1.0.0"));
        assert!(s.launch(|_| false).is_none());
    }

    #[test]
    fn reports_are_healthy_only_when_clean() {
        assert!(
            healthy("rekindle-host pid=1 base=0x1 resolved=10/10 plugins_loaded=2 mode=installed")
                .is_ok()
        );
        assert!(healthy("").is_err());
        let e = healthy("rekindle-host pid=1 error=anchors:no anchor set at C:\\x")
            .unwrap_err()
            .to_string();
        assert!(e.ends_with("error=anchors:no anchor set at C:\\x"), "{e}");
        assert!(healthy("rekindle-host pid=1 surfaces=ERR(boom)").is_err());
    }

    #[test]
    fn a_fresh_install_that_never_starts_leaves_nothing_to_try() {
        let mut s = installed("1.0.0");
        assert!(s.failed("1.0.0"));
        assert!(s.candidates().is_empty() && s.current.is_none());
    }
}
