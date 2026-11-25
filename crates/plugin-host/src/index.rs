//! The community index's tools: the automated checks a plugin goes through
//! before a person reviews it ([`prereview`]), the submission an author
//! sends ([`Submission`]), and building a submission from its source
//! ([`build_submission`]).
//!
//! The index builds every plugin itself, from the commit the author names.
//! The file users install is the index's build, never one the author
//! uploaded, so a reviewer reads the source the file was built from.
//!
//! `rekindle submit` runs [`prereview`] and writes a [`Submission`];
//! `rekindle index build` turns submissions into the files
//! `rekindle channel build --community` lists.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::builder::{self, BuildOptions};
use crate::channel::{self, Channel, Listing, Source};
use crate::manifest::{self, is_discord_host};
use crate::runtime;

/// Max size of a community plugin.
pub const MAX_BYTES: usize = 16 * 1024 * 1024;

/// How much a [`Finding`] holds up a listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    /// The index won't take it until this is fixed.
    Error,
    /// A reviewer looks at it.
    Note,
}

/// One thing the index's checks found in a submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub level: Level,
    pub message: String,
}

fn error(message: impl Into<String>) -> Finding {
    Finding {
        level: Level::Error,
        message: message.into(),
    }
}

fn note(message: impl Into<String>) -> Finding {
    Finding {
        level: Level::Note,
        message: message.into(),
    }
}

/// Whether any finding stops the plugin from being listed.
pub fn has_errors(findings: &[Finding]) -> bool {
    findings.iter().any(|f| f.level == Level::Error)
}

/// The automated checks for a built plugin or theme file. An `Err` means
/// the file couldn't be read as a plugin at all.
///
/// Errors: the first-party namespace, a plugin API this host can't load,
/// a file over [`MAX_BYTES`], an interface used without its capability.
/// Notes, for the reviewer: capabilities declared but unused, Discord
/// hosts, and an `updates` source (ignored for a community plugin, which
/// updates through the index).
pub fn prereview(bytes: &[u8]) -> Result<Vec<Finding>> {
    let mut out = Vec::new();
    if bytes.len() > MAX_BYTES {
        out.push(error(format!(
            "the file is {} MiB; the index takes up to {} MiB",
            bytes.len() >> 20,
            MAX_BYTES >> 20
        )));
    }
    let insp = runtime::inspect_artifact(&runtime::engine()?, bytes)?;
    let m = &insp.manifest;
    m.validate()?;
    if channel::is_first_party_id(&m.id) {
        out.push(error(format!(
            "`{}` is in the {} namespace, which is rekindle's own; pick an id under your name",
            m.id,
            channel::FIRST_PARTY_NAMESPACE
        )));
    }
    if !channel::wit_compatible(&m.wit, manifest::rekindle_wit_version()) {
        out.push(error(format!(
            "built against rekindle:plugin@{}; the index takes @{}",
            m.wit,
            manifest::rekindle_wit_version()
        )));
    }
    for cap in &insp.imported {
        if !m.requests(*cap) {
            out.push(error(format!(
                "uses the `{cap}` interface without requesting the `{cap}` capability"
            )));
        }
    }
    if !m.theme {
        for cap in &m.capabilities {
            if cap.interface().is_some() && !insp.imported.contains(cap) {
                out.push(note(format!("requests `{cap}` but never uses its interface; remove it unless a hook needs it")));
            }
        }
    }
    if m.any_host() {
        out.push(note("contacts any website (`network = [\"*\"]`): the listing needs `\"any_host\": true` once a maintainer has approved it"));
    }
    for h in m.network.iter().filter(|h| is_discord_host(h)) {
        out.push(note(format!("contacts Discord's API at {h}")));
    }
    if m.requests(crate::capability::Capability::WireRaw) {
        out.push(note("requests `wire-raw`: check the work it sets (`wire-raw.set-work`) does what its description says"));
    }
    if m.updates.is_some() {
        out.push(note("has an `updates` source, which a community plugin doesn't use: the index publishes its updates"));
    }
    Ok(out)
}

/// `submissions/<id>.json` in the index repository: what an author asks the
/// index to list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Submission {
    pub id: String,
    pub version: String,
    /// The account submitting it. The first submission in a namespace
    /// claims it, and later ones must come from the same account.
    pub author: String,
    pub source: Source,
    /// The plugin may reach any website (`network = ["*"]`). A maintainer
    /// sets it when approving that, and the build refuses such a plugin
    /// without it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub any_host: bool,
}

impl Submission {
    /// Check the id, the version, the author and the source are well formed.
    pub fn validate(&self) -> Result<()> {
        manifest::validate_id(&self.id)?;
        semver::Version::parse(&self.version)
            .with_context(|| format!("version `{}` is not semver", self.version))?;
        if self.author.trim().is_empty() {
            bail!("the submission has no author");
        }
        self.source.validate()
    }
}

/// Whether `author` may submit in namespace `ns`, given who owns which
/// (`owners`). Accounts compare case-insensitively, as GitHub logins do.
///
/// - `io.github.<login>` and everything under it belong to that GitHub
///   account, claimed or not.
/// - A namespace already listed stays with the account that claimed it.
/// - Any other new namespace is claimed by this submission, and the finding
///   asks a maintainer to confirm the author may use it (that they own the
///   domain, or the name is theirs) before merging.
pub fn namespace_check(
    ns: &str,
    author: &str,
    owners: &BTreeMap<String, String>,
) -> Result<Option<Finding>> {
    let same = |a: &str| a.eq_ignore_ascii_case(author);
    // `io.github.alice` would sit beside every GitHub account's namespace,
    // in one that names no account.
    if ns == "io.github" {
        bail!("io.github belongs to no one; use io.github.{author}.<plugin>");
    }
    if let Some(login) = channel::github_login(ns) {
        if !same(login) {
            bail!(
                "{ns} belongs to the GitHub account {login}; submit it as {login}, or use io.github.{author}"
            );
        }
        return Ok(None);
    }
    match owners.get(ns) {
        Some(owner) if !same(owner) => bail!("namespace {ns} belongs to {owner}, not {author}"),
        Some(_) => Ok(None),
        None => Ok(Some(note(format!(
            "first submission in {ns}: a maintainer confirms {author} may use it (owns the domain, or the name is theirs) before merging; io.github.{author} needs no check"
        )))),
    }
}

/// Who owns each namespace, from the entries a channel already lists.
pub fn owners(c: &Channel) -> BTreeMap<String, String> {
    c.community
        .iter()
        .map(|e| {
            (
                channel::namespace(&e.plugin.id).to_string(),
                e.author.clone(),
            )
        })
        .collect()
}

fn git(dir: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .context("running git")?;
    if !status.success() {
        bail!("`git {}` failed", args.join(" "));
    }
    Ok(())
}

/// What [`build_submission`] produced.
#[derive(Debug)]
pub struct Built {
    /// The plugin file in the output directory, with its listing beside it.
    /// `None` if the findings include an error: nothing is written then.
    pub file: Option<PathBuf>,
    pub findings: Vec<Finding>,
}

/// Fetch a submission's source at its commit into `work`, build it, run
/// [`prereview`], and check it is the id and version submitted and that
/// `owners` gives its namespace to this author (or to nobody yet). If no
/// finding is an error, writes `<id>-<version>.wasm` (or `.theme.json`) and
/// its [`Listing`] into `out`.
pub fn build_submission(
    sub: &Submission,
    owners: &BTreeMap<String, String>,
    work: &Path,
    out: &Path,
) -> Result<Built> {
    sub.validate()?;
    let claim = namespace_check(channel::namespace(&sub.id), &sub.author, owners)?;
    // Files left from an earlier build would survive the checkout below.
    match std::fs::remove_dir_all(work) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(e).with_context(|| format!("clearing {}", work.display()));
        }
        _ => {}
    }
    std::fs::create_dir_all(work).with_context(|| format!("creating {}", work.display()))?;
    git(work, &["init", "--quiet"])?;
    git(
        work,
        &[
            "fetch",
            "--quiet",
            "--depth",
            "1",
            &sub.source.repo,
            &sub.source.commit,
        ],
    )?;
    git(work, &["checkout", "--quiet", "FETCH_HEAD"])?;
    let dir = work.join(&sub.source.path);

    let theme_file = std::fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| channel::is_theme_path(p));
    let (bytes, ext) = match theme_file {
        Some(t) if !dir.join("Cargo.toml").exists() && !dir.join("package.json").exists() => (
            std::fs::read(&t).with_context(|| format!("reading {}", t.display()))?,
            "theme.json",
        ),
        _ => (
            builder::build(&BuildOptions {
                path: dir.clone(),
                release: true,
                quiet: true,
                typecheck: true,
            })?
            .bytes,
            "wasm",
        ),
    };
    let mut findings = prereview(&bytes)?;
    findings.extend(claim);
    let m = manifest::extract(&bytes)?.context("the build has no manifest")?;
    if (m.id.as_str(), m.version.as_str()) != (sub.id.as_str(), sub.version.as_str()) {
        bail!(
            "the source builds {} {}, but the submission is for {} {}",
            m.id,
            m.version,
            sub.id,
            sub.version
        );
    }
    let file = out.join(format!("{}-{}.{ext}", sub.id, sub.version));
    let listing_path = channel::listing_path_for(&file);
    if has_errors(&findings) {
        // `channel build --community` publishes every file with a listing,
        // so a failed build leaves none, not even one from an earlier run.
        remove_if_present(&listing_path)?;
        remove_if_present(&file)?;
        return Ok(Built {
            file: None,
            findings,
        });
    }
    std::fs::create_dir_all(out).with_context(|| format!("creating {}", out.display()))?;
    std::fs::write(&file, &bytes).with_context(|| format!("writing {}", file.display()))?;
    let listing = Listing {
        author: sub.author.clone(),
        source: sub.source.clone(),
        any_host: sub.any_host,
    };
    std::fs::write(&listing_path, serde_json::to_vec_pretty(&listing)?)
        .with_context(|| format!("writing {}", listing_path.display()))?;
    Ok(Built {
        file: Some(file),
        findings,
    })
}

fn remove_if_present(path: &Path) -> Result<()> {
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

    fn sub(id: &str, author: &str) -> Submission {
        Submission {
            id: id.into(),
            version: "1.0.0".into(),
            author: author.into(),
            source: Source {
                repo: "https://example.com/a.git".into(),
                commit: "a".repeat(40),
                path: String::new(),
            },
            any_host: false,
        }
    }

    #[test]
    fn submissions_are_checked() {
        sub("dev.alice.notes", "alice").validate().unwrap();
        let mut s = sub("dev.alice.notes", "alice");
        s.source.commit = "main".into();
        assert!(s.validate().is_err(), "a branch is not a commit");
        let mut s = sub("dev.alice.notes", "alice");
        s.source.repo = "http://example.com/a.git".into();
        assert!(s.validate().is_err());
        for path in ["../elsewhere", "/elsewhere", "C:/elsewhere", "C:elsewhere"] {
            let mut s = sub("dev.alice.notes", "alice");
            s.source.path = path.into();
            assert!(s.validate().is_err(), "{path}");
        }
        let mut s = sub("dev.alice.notes", "alice");
        s.source.path = "plugins/notes".into();
        s.validate().unwrap();
    }

    #[test]
    fn a_namespace_belongs_to_its_first_author() {
        let owners = BTreeMap::from([("dev.alice".to_string(), "alice".to_string())]);
        let work = std::env::temp_dir().join(format!("rekindle-index-{}", std::process::id()));
        let err = build_submission(&sub("dev.alice.notes", "mallory"), &owners, &work, &work)
            .unwrap_err();
        assert!(err.to_string().contains("belongs to alice"), "{err:#}");
        // Logins compare case-insensitively, as on GitHub.
        assert!(
            namespace_check("dev.alice", "Alice", &owners)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_github_namespace_belongs_to_that_account() {
        let none = BTreeMap::new();
        assert!(
            namespace_check("io.github.alice", "alice", &none)
                .unwrap()
                .is_none(),
            "no maintainer check needed"
        );
        assert!(
            namespace_check("io.github.alice.tools", "ALICE", &none)
                .unwrap()
                .is_none()
        );
        let err = namespace_check("io.github.alice", "mallory", &none).unwrap_err();
        assert!(
            err.to_string()
                .contains("belongs to the GitHub account alice"),
            "{err:#}"
        );
        // `io.github.alice` itself is in `io.github`, which names no account.
        assert!(namespace_check("io.github", "alice", &none).is_err());
    }

    #[test]
    fn a_new_custom_namespace_asks_a_maintainer() {
        let f = namespace_check("com.example", "alice", &BTreeMap::new())
            .unwrap()
            .expect("a finding");
        assert_eq!(f.level, Level::Note);
        assert!(
            f.message.contains("a maintainer confirms alice may use it"),
            "{}",
            f.message
        );
    }
}
