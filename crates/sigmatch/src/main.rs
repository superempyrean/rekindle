//! rekindle-sigmatch: locate crates in a release binary by matching the machine
//! code of functions we compile ourselves, not the paths the compiler happened
//! to leave behind.
//!
//! `rekindle-analyze` recovers crate versions from embedded `registry/src/...`
//! paths, but only crates with a surviving panic site leave such a path. On
//! Zed v1.27.0 that was about half the linked set. The other half is still in
//! the binary as code. Given the exact `rustc` (which `rekindle-analyze` already
//! recovers) we can compile a candidate crate, reduce each function to a
//! relocation-masked byte signature, and find it in the target. A match is
//! positive evidence the crate is present even with no strings at all.
//!
//! Two steps, so the slow compile is separable from scanning:
//!   * `sigs`: build a signature set from a compiled `.rlib`/object.
//!   * `scan`: match a signature set against a target binary.
//!
//! And `short`, which needs no source at all: the shortest unique signature
//! for a function in a binary, given its address (a port of ida-sigmaker).
//!
//! Everything here is static analysis of code we compiled plus a file we read;
//! it inspects a binary, it does not modify one.

use rekindle_sigmatch::{compile, extract, scan, short, sigfile, snapshots};

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use sigfile::SigFile;

#[derive(Parser)]
#[command(
    name = "rekindle-sigmatch",
    about = "Recover crates with no path strings by matching compiled function signatures.",
    version
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Compile a named crate version, then build its signature set.
    Compile(CompileCli),
    /// Build a signature set from a compiled `.rlib` or object file.
    Sigs(SigsCli),
    /// Match a signature set against a target binary.
    Scan(ScanCli),
    /// Shortest unique signature for each function address in a PE binary
    /// (ida-sigmaker's algorithm; no source or IDA needed).
    Short(ShortCli),
    /// Trim a long pattern (e.g. a whole function compiled from source) to its
    /// shortest window that is still unique in the target, at the right place.
    Trim(TrimCli),
    /// List `gpui-pre` versions and the Zed commit each snapshots: the seed
    /// candidates for a fork-point search (compile each with `--git
    /// https://github.com/zed-industries/zed --rev <commit>`).
    Snapshots {
        /// crates.io package to list.
        #[arg(default_value = "gpui-pre")]
        krate: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Args)]
struct CompileCli {
    /// Crate to compile and signature, e.g. `smallvec`.
    name: String,

    /// Toolchain to build with, e.g. `1.98.1` (from `rekindle-analyze`).
    #[arg(long)]
    toolchain: String,

    /// crates.io: exact version to pin; omit to take the newest cargo resolves.
    #[arg(long, group = "src")]
    version: Option<String>,

    /// git source URL (for a forked dependency); pair with `--rev`.
    #[arg(long, group = "src")]
    git: Option<String>,

    /// git revision/tag/branch to pin when `--git` is set.
    #[arg(long, requires = "git")]
    rev: Option<String>,

    /// Local path to the crate's source (e.g. a checked-out fork).
    #[arg(long, group = "src")]
    path: Option<PathBuf>,

    /// Cargo feature to enable (repeatable).
    #[arg(long = "feature")]
    features: Vec<String>,

    /// Pass `--no-default-features` to the build.
    #[arg(long)]
    no_default_features: bool,

    /// Whole `RUSTFLAGS` to build with. Set this to the target's own flags.
    /// Zed v1.27.0: `-C symbol-mangling-version=v0 --cfg tokio_unstable`.
    #[arg(long)]
    rustflags: Option<String>,

    /// Codegen units; match the target's `[profile.release]`.
    #[arg(long, default_value_t = 1)]
    codegen_units: u32,

    /// Optimisation level; match the target's `[profile.release]`.
    #[arg(long, default_value = "3")]
    opt_level: String,

    /// Where to create the throwaway build package (defaults to a temp dir).
    #[arg(long)]
    work_root: Option<PathBuf>,

    /// Drop functions with fewer than this many fixed bytes.
    #[arg(long, default_value_t = 8)]
    min_significant: usize,
}

#[derive(clap::Args)]
struct SigsCli {
    /// Compiled `.rlib` (preferred) or a single relocatable object.
    input: PathBuf,

    /// Provenance recorded in the output, e.g. `gpui 0.2.0`.
    #[arg(long, default_value = "unknown")]
    source: String,

    /// Drop functions with fewer than this many fixed (non-wildcard) bytes;
    /// short signatures match by chance. 8 is a conservative default.
    #[arg(long, default_value_t = 8)]
    min_significant: usize,

    /// Treat `input` as a single object even if it looks like an archive.
    #[arg(long)]
    object: bool,
}

#[derive(clap::Args)]
struct ScanCli {
    /// Signature set produced by `sigs` (JSON).
    sigs: PathBuf,

    /// Target binary to search.
    binary: PathBuf,

    /// List the matched (and ambiguous) symbols, not just the totals.
    #[arg(long)]
    verbose: bool,

    /// Emit the full report as JSON instead of the text summary.
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
struct ShortCli {
    /// The PE32+ binary.
    binary: PathBuf,
    /// Function addresses (VA like 0x14009c040, or RVA), or with `--verify`,
    /// signatures to locate.
    #[arg(required = true)]
    targets: Vec<String>,
    /// Keep these operand kinds as literal bytes (comma-separated: displ, imm,
    /// mem, near, far), for functions that only differ in an offset.
    #[arg(long, default_value = "")]
    keep_operands: String,
    /// Let the pattern run past the end of the function.
    #[arg(long)]
    continue_outside: bool,
    /// Functions (comma-separated VAs) the signature may also match, because
    /// they're interchangeable with the target for the caller's purpose.
    #[arg(long, default_value = "")]
    accept: String,
    /// Also report where each signature matches in these other builds.
    #[arg(long, num_args = 1..)]
    check: Vec<PathBuf>,
    /// Targets are signatures: report where they match, generate nothing.
    #[arg(long)]
    verify: bool,
}

#[derive(clap::Args)]
struct TrimCli {
    /// The PE32+ target binary.
    binary: PathBuf,
    /// The pattern, or `@file` holding it (whole-function patterns are long).
    pattern: String,
    /// Minimum fixed bytes in the window.
    #[arg(long, default_value_t = 12)]
    min_fixed: usize,
    /// Don't start the window before this byte of the pattern.
    #[arg(long, default_value_t = 0)]
    skip: usize,
}

fn trim_cmd(cli: &TrimCli) -> Result<()> {
    let bytes =
        std::fs::read(&cli.binary).with_context(|| format!("reading {}", cli.binary.display()))?;
    let img = short::PeImage::parse(&bytes)?;
    let text = match cli.pattern.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?,
        None => cli.pattern.clone(),
    };
    let pattern = rekindle_sigmatch::sig::Signature::parse(text.trim())?;
    let (offset, window) = short::trim_unique(&img, &pattern, cli.min_fixed, cli.skip)?;
    println!(
        "{}",
        serde_json::json!({
            "offset": offset,
            "signature": window.to_pattern(),
            "fixed": window.significant(),
            "from_fixed": pattern.significant(),
        })
    );
    Ok(())
}

/// Prints one JSON line per target. Returns whether every signature matched
/// exactly once in the binary and in each `--check` build.
fn short_cmd(cli: &ShortCli) -> Result<bool> {
    let bytes =
        std::fs::read(&cli.binary).with_context(|| format!("reading {}", cli.binary.display()))?;
    let img = short::PeImage::parse(&bytes)?;
    let opts = short::ShortOptions {
        wildcard: short::OperandKinds::keeping(&cli.keep_operands)?,
        continue_outside: cli.continue_outside,
        ..Default::default()
    };
    let accept: Vec<u64> = cli
        .accept
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(hex_addr)
        .collect::<Result<_>>()
        .context("bad --accept address")?;
    let others: Vec<(PathBuf, Vec<u8>)> = cli
        .check
        .iter()
        .map(|p| std::fs::read(p).map(|b| (p.clone(), b)))
        .collect::<Result<_, _>>()?;
    let va = |img: &short::PeImage, rvas: Vec<u32>| -> Vec<String> {
        rvas.into_iter()
            .map(|r| format!("{:#x}", img.image_base + u64::from(r)))
            .collect()
    };
    let mut all_unique = true;
    for t in &cli.targets {
        let (sig, target) = if cli.verify {
            (rekindle_sigmatch::sig::Signature::parse(t)?, None)
        } else {
            let addr = hex_addr(t)?;
            match short::signature_among(&img, addr, &accept, &opts) {
                Ok(s) => (s, Some(addr)),
                Err(e) => {
                    all_unique = false;
                    println!(
                        "{}",
                        serde_json::json!({ "target": t, "error": e.to_string() })
                    );
                    continue;
                }
            }
        };
        let here = va(&img, short::matches(&img, &sig));
        let mut checks = serde_json::Map::new();
        for (p, b) in &others {
            let other = short::PeImage::parse(b)?;
            let m = va(&other, short::matches(&other, &sig));
            all_unique &= m.len() == 1;
            checks.insert(p.display().to_string(), serde_json::json!(m));
        }
        all_unique &= here.len() == 1;
        let unique_at_target = target.map(|a| {
            let at = img.image_base + u64::from(img.to_rva(a).unwrap_or(0));
            here == [format!("{at:#x}")]
        });
        println!(
            "{}",
            serde_json::json!({
                "target": t,
                "signature": sig.to_pattern(),
                "bytes": sig.len(),
                "matches": here,
                "unique_at_target": unique_at_target,
                "accepted": !cli.accept.is_empty(),
                "check": checks,
            })
        );
    }
    Ok(all_unique)
}

/// A hexadecimal address, with or without `0x`.
fn hex_addr(s: &str) -> Result<u64> {
    let digits = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u64::from_str_radix(digits, 16).with_context(|| format!("bad address {s:?}"))
}

fn main() -> Result<ExitCode> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    match Cli::parse().cmd {
        Cmd::Compile(cli) => compile_cmd(cli)?,
        Cmd::Sigs(cli) => sigs_cmd(cli)?,
        Cmd::Scan(cli) => scan_cmd(&cli)?,
        Cmd::Short(cli) => {
            if !short_cmd(&cli)? {
                return Ok(ExitCode::FAILURE);
            }
        }
        Cmd::Trim(cli) => trim_cmd(&cli)?,
        Cmd::Snapshots { krate, json } => {
            let snaps = snapshots::fetch(&krate)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&snaps)?);
            } else {
                for s in &snaps {
                    let commit = s.zed_commit.as_deref().unwrap_or("-");
                    let y = if s.yanked { "  (yanked)" } else { "" };
                    println!("{:<12} {}  zed@{commit}{y}", s.version, s.published);
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn compile_cmd(cli: CompileCli) -> Result<()> {
    let work_root = match cli.work_root {
        Some(p) => p,
        None => std::env::temp_dir().join("rekindle-sigmatch"),
    };
    // `--version`/`--git`/`--path` are a clap group, so at most one is set.
    let source = if let Some(path) = cli.path {
        compile::CrateSource::Path(path)
    } else if let Some(url) = cli.git {
        compile::CrateSource::Git { url, rev: cli.rev }
    } else {
        compile::CrateSource::Version(cli.version)
    };

    let rlib = compile::compile_rlib(&compile::CompileArgs {
        crate_name: &cli.name,
        source: &source,
        toolchain: &cli.toolchain,
        features: &cli.features,
        no_default_features: cli.no_default_features,
        rustflags: cli.rustflags.as_deref(),
        codegen_units: cli.codegen_units,
        opt_level: &cli.opt_level,
        work_root: &work_root,
    })?;
    tracing::info!(rlib = %rlib.display(), "compiled");

    let bytes = std::fs::read(&rlib).with_context(|| format!("reading {}", rlib.display()))?;
    let fns = extract::from_rlib(&bytes, cli.min_significant)?;
    tracing::info!(functions = fns.len(), "built signatures");
    if fns.is_empty() {
        tracing::warn!("no functions extracted: the crate may be entirely generic or inline");
    }

    let file = SigFile::build(source.describe(&cli.name), cli.min_significant, &fns);
    println!("{}", serde_json::to_string_pretty(&file)?);
    Ok(())
}

fn sigs_cmd(cli: SigsCli) -> Result<()> {
    let bytes =
        std::fs::read(&cli.input).with_context(|| format!("reading {}", cli.input.display()))?;

    let is_archive = !cli.object && bytes.starts_with(b"!<arch>\n");
    let fns = if is_archive {
        extract::from_rlib(&bytes, cli.min_significant)?
    } else {
        extract::from_object(&bytes, cli.min_significant)?
    };
    tracing::info!(functions = fns.len(), source = %cli.source, "built signatures");

    let file = SigFile::build(cli.source, cli.min_significant, &fns);
    println!("{}", serde_json::to_string_pretty(&file)?);
    Ok(())
}

fn scan_cmd(cli: &ScanCli) -> Result<()> {
    let sig_text = std::fs::read_to_string(&cli.sigs)
        .with_context(|| format!("reading {}", cli.sigs.display()))?;
    let file: SigFile = serde_json::from_str(&sig_text).context("parsing signature set")?;
    let bytes =
        std::fs::read(&cli.binary).with_context(|| format!("reading {}", cli.binary.display()))?;

    let report = scan::scan(&file, &bytes)?;

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!("{}", report.source);
    println!(
        "  matched {}/{} functions ({:.1}% recall): {} unique, {} ambiguous, {} missing",
        report.unique.len() + report.ambiguous.len(),
        report.total,
        report.recall() * 100.0,
        report.unique.len(),
        report.ambiguous.len(),
        report.missing,
    );

    if cli.verbose {
        for m in &report.unique {
            println!(
                "  unique    {:>8x}  {} ({} sig bytes)",
                m.hits[0], m.symbol, m.significant
            );
        }
        for m in &report.ambiguous {
            println!(
                "  ambiguous {:>4} hits  {} ({} sig bytes)",
                m.hits.len(),
                m.symbol,
                m.significant
            );
        }
    }
    Ok(())
}
