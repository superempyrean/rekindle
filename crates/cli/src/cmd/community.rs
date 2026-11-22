//! The community index: `browse` and `get` (the store, from a terminal),
//! `check` and `submit` (authors), and `index build` (the index).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use rekindle_plugin_host::builder::{self, BuildOptions};
use rekindle_plugin_host::channel::{self, Location, Source};
use rekindle_plugin_host::home::Home;
use rekindle_plugin_host::index::{self, Level, Submission};
use rekindle_plugin_host::{manifest, signing, store};

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// List the plugins in the community index (from the last update check).
    Browse,
    /// Download a plugin from the community index and install it, after
    /// showing what it can do.
    Get {
        id: String,
        /// Turn it on right away.
        #[arg(long)]
        enable: bool,
        /// Don't ask before installing (scripts).
        #[arg(long, short)]
        yes: bool,
    },
    /// Run the community index's automated checks on a plugin: a project
    /// directory (built first), a `.wasm` or a `.theme.json`.
    Check {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Check a plugin and write the submission file for the community
    /// index, for the commit checked out now.
    Submit {
        /// Plugin project directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Your account on the index's host (it owns the id's namespace).
        #[arg(long)]
        author: String,
        /// The repository's https URL (default: the `origin` remote).
        #[arg(long)]
        repo: Option<String>,
        /// Where to write the submission (default: `<id>.json` in the
        /// project directory).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// The index's own tools.
    Index {
        #[command(subcommand)]
        cmd: IndexCmd,
    },
}

#[derive(Subcommand)]
pub(crate) enum IndexCmd {
    /// Build each submission from its source, check it, and write the files
    /// `rekindle channel build --community` lists.
    Build {
        /// A submission `.json`, or a directory of them.
        submissions: PathBuf,
        /// Output directory.
        #[arg(long)]
        out: PathBuf,
        /// The published channel (path or https URL), for who owns which
        /// namespace and which versions are already listed.
        #[arg(long)]
        channel: Option<String>,
        /// Where sources are checked out (default: a directory in the
        /// system temp directory).
        #[arg(long)]
        work: Option<PathBuf>,
    },
}

pub(crate) fn run(c: Commands) -> Result<()> {
    match c {
        Commands::Browse => browse(),
        Commands::Get { id, enable, yes } => {
            let home = Home::from_env()?;
            let (bytes, sig) = store::download(&home, &id)?;
            super::plugins::install_bytes(&bytes, sig.as_ref(), enable, yes)
        }
        Commands::Check { path } => {
            if print_findings(&index::prereview(&artifact(&path)?)?) {
                bail!("the community index won't take it until the errors above are fixed");
            }
            Ok(())
        }
        Commands::Submit {
            path,
            author,
            repo,
            out,
        } => submit(&path, author, repo, out),
        Commands::Index {
            cmd:
                IndexCmd::Build {
                    submissions,
                    out,
                    channel,
                    work,
                },
        } => index_build(&submissions, &out, channel, work),
    }
}

fn browse() -> Result<()> {
    let home = Home::from_env()?;
    let items = store::list(&home);
    if items.is_empty() {
        println!("the store is empty: run `rekindle update` to fetch the release channel");
    }
    for i in &items {
        let p = &i.entry.plugin;
        let state = match &i.installed {
            Some(v) if i.is_update() => format!("  [installed {v}, update available]"),
            Some(_) => "  [installed]".into(),
            None => String::new(),
        };
        println!(
            "{} {} ({}) by {}{state}",
            p.id, p.version, p.name, i.entry.author
        );
        if !p.description.is_empty() {
            println!("  {}", p.description);
        }
        let caps: Vec<&str> = i.entry.capabilities.iter().map(|c| c.as_str()).collect();
        if !caps.is_empty() {
            println!("  requests: {}", caps.join(", "));
        }
    }
    Ok(())
}

/// A plugin's bytes: a built `.wasm` or a `.theme.json` as it is, or a
/// project built now.
fn artifact(path: &Path) -> Result<Vec<u8>> {
    let built = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("wasm"))
        || path
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".theme.json");
    if built && path.is_file() {
        return std::fs::read(path).with_context(|| format!("reading {}", path.display()));
    }
    let built = builder::build(&BuildOptions {
        path: path.to_path_buf(),
        release: true,
        quiet: true,
        typecheck: true,
    })?;
    Ok(built.bytes)
}

/// Print the findings; whether any is an error.
fn print_findings(findings: &[index::Finding]) -> bool {
    if findings.is_empty() {
        println!("no findings: it passes the automated checks");
    }
    for f in findings {
        let tag = match f.level {
            Level::Error => "error",
            Level::Note => "note",
        };
        println!("{tag}: {}", f.message);
    }
    index::has_errors(findings)
}

fn git_out(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `git@host:owner/repo.git` as `https://host/owner/repo.git`.
fn https_url(remote: &str) -> String {
    match remote.strip_prefix("git@").and_then(|r| r.split_once(':')) {
        Some((host, path)) => format!("https://{host}/{path}"),
        None => remote.to_string(),
    }
}

fn submit(path: &Path, author: String, repo: Option<String>, out: Option<PathBuf>) -> Result<()> {
    let dir = if path.is_file() {
        path.parent().unwrap_or(Path::new(".")).to_path_buf()
    } else {
        path.to_path_buf()
    };
    let bytes = artifact(&dir)?;
    if print_findings(&index::prereview(&bytes)?) {
        bail!("fix the errors above before submitting");
    }
    let m = manifest::extract(&bytes)?.context("the build has no manifest")?;
    if !git_out(&dir, &["status", "--porcelain", "--", "."])?.is_empty() {
        bail!(
            "{} has uncommitted changes; the index builds the commit, so commit them first",
            dir.display()
        );
    }
    let commit = git_out(&dir, &["rev-parse", "HEAD"])?;
    let repo = match repo {
        Some(r) => r,
        None => https_url(&git_out(&dir, &["remote", "get-url", "origin"])?),
    };
    let prefix = git_out(&dir, &["rev-parse", "--show-prefix"])?;
    let source = Source {
        repo,
        commit,
        path: prefix.trim_end_matches('/').to_string(),
    };
    let sub = Submission {
        id: m.id.clone(),
        version: m.version.clone(),
        author,
        source,
    };
    sub.validate()?;
    let out = out.unwrap_or_else(|| dir.join(format!("{}.json", m.id)));
    std::fs::write(&out, serde_json::to_string_pretty(&sub)? + "\n")
        .with_context(|| format!("writing {}", out.display()))?;
    println!("wrote {}", out.display());
    println!(
        "  {} {} from {} at {}",
        sub.id,
        sub.version,
        sub.source.repo,
        &sub.source.commit[..12]
    );
    println!(
        "next: push that commit, then open a pull request adding this file as `submissions/{}.json` to superempyrean/rekindle-index",
        m.id
    );
    Ok(())
}

fn index_build(
    submissions: &Path,
    out: &Path,
    channel_loc: Option<String>,
    work: Option<PathBuf>,
) -> Result<()> {
    let published = match channel_loc {
        Some(l) => {
            let (bytes, sig) = channel::fetch(&Location::parse(&l)?)?;
            Some(channel::verify(&bytes, &sig, &signing::first_party_keys())?.0)
        }
        None => None,
    };
    let mut owners = published.as_ref().map(index::owners).unwrap_or_default();
    let files: Vec<PathBuf> = if submissions.is_dir() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(submissions)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect();
        v.sort();
        v
    } else {
        vec![submissions.to_path_buf()]
    };
    let work = work.unwrap_or_else(|| {
        std::env::temp_dir().join(format!("rekindle-index-{}", std::process::id()))
    });
    let mut failed = 0;
    for f in files {
        let sub: Submission = serde_json::from_slice(&std::fs::read(&f)?)
            .with_context(|| format!("parsing {}", f.display()))?;
        let listed = published.as_ref().is_some_and(|c| {
            c.community
                .iter()
                .any(|e| e.plugin.id == sub.id && e.plugin.version == sub.version)
        });
        if listed {
            println!("{} {}: already listed", sub.id, sub.version);
            continue;
        }
        println!("{} {} by {}", sub.id, sub.version, sub.author);
        match index::build_submission(&sub, &owners, &work.join(&sub.id), out) {
            Ok(b) => {
                print_findings(&b.findings);
                let Some(file) = b.file else {
                    failed += 1;
                    continue;
                };
                owners
                    .entry(channel::namespace(&sub.id).to_string())
                    .or_insert(sub.author.clone());
                println!("  -> {}", file.display());
            }
            Err(e) => {
                println!("  failed: {e:#}");
                failed += 1;
            }
        }
    }
    // Checkouts left in the temp directory are only clutter.
    let _ = std::fs::remove_dir_all(&work);
    if failed > 0 {
        bail!("{failed} submission(s) weren't built");
    }
    Ok(())
}
