//! rekindle-analyze: first-pass triage of a Rust release binary.
//!
//! Given a compiled Rust application (our development target is Zed; the
//! eventual target is Discord's Phoenix client), recover two things that make
//! every later step easier:
//!
//!   1. The exact `rustc` version it was built with, from embedded panic paths
//!      like `/rustc/<40-hex-commit>/library/...`.
//!   2. Which crates (and often which versions) are vendored in, from the
//!      `cargo registry` source paths the compiler bakes into panic/debug
//!      strings, e.g. `.../registry/src/index.crates.io-.../gpui-0.1.0/...`.
//!
//! Both are standard, non-invasive static analysis: we only read strings from
//! the file. Knowing the compiler + crate versions lets us compile matching
//! copies locally and recover real type layouts and function signatures. That is
//! the foundation the anchor/re-resolution pipeline is built on.

use rekindle_analysis::{crates, fork_point, locations, report, rustc_id, sections, strings};

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};

#[derive(Parser)]
#[command(
    name = "rekindle-analyze",
    about = "Fingerprint the Rust compiler and vendored crates in a release binary.",
    version
)]
struct Cli {
    /// Path to the binary (ELF / PE / Mach-O) to inspect.
    binary: PathBuf,

    /// Minimum ASCII run length to treat as a string.
    #[arg(long, default_value_t = 5)]
    min_len: usize,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,

    /// Show every crate occurrence, not just one row per crate.
    #[arg(long)]
    verbose_crates: bool,

    /// Instead of the report, find the commit of this git repository that the
    /// binary was built from (or that its fork of it branched from), by its
    /// panic locations. Prints the best-scoring commits, then each file's
    /// agreement at the best one: a fork's edited files score lowest.
    #[arg(long, value_name = "REPO")]
    fork_point: Option<PathBuf>,

    /// With `--fork-point`: the candidates, as `git rev-list` arguments, e.g.
    /// "--first-parent v1.26.0..v1.27.0" or "--tags --no-walk".
    #[arg(long, default_value = "--tags --no-walk", allow_hyphen_values = true)]
    revs: String,

    /// With `--fork-point`: how many commits to list.
    #[arg(long, default_value_t = 5)]
    top: usize,
}

#[derive(Copy, Clone, ValueEnum)]
enum Format {
    Text,
    Json,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let bytes =
        std::fs::read(&cli.binary).with_context(|| format!("reading {}", cli.binary.display()))?;
    tracing::info!(size = bytes.len(), path = %cli.binary.display(), "loaded binary");

    if let Some(repo) = &cli.fork_point {
        let locs = locations::extract(&bytes);
        let revs: Vec<String> = cli.revs.split_whitespace().map(str::to_string).collect();
        let fp = fork_point::search(repo, &locs, &revs, cli.top)?;
        match cli.format {
            Format::Json => println!("{}", serde_json::to_string_pretty(&fp)?),
            Format::Text => print_fork_point(&fp),
        }
        return Ok(());
    }

    // Pull readable string regions. We scan the whole file rather than only
    // `.rodata`, because packed/renamed sections differ across platforms and
    // the cost is trivial.
    let strings = strings::extract(&bytes, cli.min_len);
    tracing::info!(count = strings.len(), "extracted strings");

    let rustc = rustc_id::detect(&strings);
    let mut found_crates = crates::detect(&strings);

    // Resolve each crate anchor to the section it lives in. Non-fatal: an
    // unparseable container just leaves sections empty and anchors section-less.
    let section_map = sections::SectionMap::parse(&bytes)?;
    if section_map.is_empty() {
        tracing::warn!("no section table recovered; anchors will lack a section name");
    }
    crates::annotate_sections(&mut found_crates, &section_map);
    let section_spans = section_map.spans().to_vec();

    let report = report::Report::new(&cli.binary, bytes.len(), rustc, found_crates, section_spans);

    match cli.format {
        Format::Text => report.print_text(cli.verbose_crates),
        Format::Json => println!("{}", report.to_json()?),
    }

    Ok(())
}

fn print_fork_point(fp: &fork_point::ForkPoint) {
    let pct = |h: usize, n: usize| 100.0 * h as f64 / n as f64;
    println!(
        "{} panic locations in {} files of the repository, {} commits scored",
        fp.locations, fp.files, fp.commits_scored
    );
    for (i, c) in fp.ranked.iter().enumerate() {
        println!(
            "  {}. {:5.1}%  {:.10} {}  {}",
            i + 1,
            pct(c.hits, fp.locations),
            c.commit,
            c.date,
            c.subject
        );
        if c.ties > 1 {
            println!(
                "               and {} more with the same score, back to {:.10}",
                c.ties - 1,
                c.oldest
            );
        }
    }
    if let Some(best) = fp.ranked.first() {
        println!("at {:.10}, files by agreement (lowest first):", best.commit);
        for f in fp.best_files.iter().take(12) {
            println!(
                "  {:5.1}%  {:4}/{:<4} {}",
                pct(f.hits, f.locations),
                f.hits,
                f.locations,
                f.path
            );
        }
        let on = if fp.debug_asserts_located > 0 {
            "on"
        } else {
            "off"
        };
        println!(
            "debug assertions {on}: {} of the {} `debug_assert` calls in these files have a panic location",
            fp.debug_asserts_located, fp.debug_asserts
        );
        let edited: Vec<_> = fp
            .best_files
            .iter()
            .filter(|f| !f.edits.is_empty())
            .collect();
        if !edited.is_empty() {
            println!("edits that move the binary's lines against it:");
            for f in edited {
                for e in &f.edits {
                    println!(
                        "  {}: {:+} lines between lines {} and {}",
                        f.path, e.inserted, e.after, e.before
                    );
                    if !e.types.is_empty() {
                        println!("      types defined there: {}", e.types.join(", "));
                    }
                }
            }
            println!(
                "types whose layout can't be taken from {:.10}, theirs and the types that hold them by value ({}):",
                best.commit,
                fp.suspect_types.len()
            );
            println!("  {}", fp.suspect_types.join(", "));
        }
    }
}
