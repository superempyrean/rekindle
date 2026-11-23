//! rekindle-anchor: resolve integration-point anchors against a build, and
//! report which broke between builds.
//!
//! Anchors are durable descriptors (strings, symbols, byte signatures); a
//! release binary moves things between builds, so we re-resolve per build
//! instead of hardcoding addresses. This is static analysis: it reads a file
//! and reports locations; it never modifies a binary.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rekindle_anchor::{
    Anchor, AnchorSet, Change, DiffReport, Operand, ResolutionReport, Status, Verdict, diff,
};
use rekindle_sigmatch::sig::Signature;

#[derive(Parser)]
#[command(
    name = "rekindle-anchor",
    version,
    about = "Resolve integration-point anchors per build and report breakage"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Resolve an anchor set against one binary.
    Resolve {
        anchors: PathBuf,
        binary: PathBuf,
        #[arg(long)]
        json: bool,
        /// Write the resolution report here (for a later `diff-report`).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Exit 0 even if some anchors are broken.
        #[arg(long)]
        allow_broken: bool,
    },
    /// Resolve an anchor set against two builds and report what changed.
    Diff {
        anchors: PathBuf,
        old_binary: PathBuf,
        new_binary: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Suggest function anchors from a binary alone: for each NAME=ADDR, a
    /// short unique signature plus string-ref locators, each verified. Prints
    /// an anchor set (JSON).
    Suggest {
        binary: PathBuf,
        /// `name=0xADDR` pairs.
        #[arg(required = true)]
        functions: Vec<String>,
        /// At most this many string-ref locators per anchor.
        #[arg(long, default_value_t = 2)]
        max_strings: usize,
        /// `name=@file`: a whole-function pattern compiled from source for that
        /// function (e.g. gpui's), trimmed to a short window as a locator.
        #[arg(long)]
        source: Vec<String>,
        /// `name=0xA,0xB`: functions the anchor may also resolve to because
        /// they're interchangeable with the target (a generic over types with
        /// one layout and delegating behaviour). Marks the anchor `any_of`.
        #[arg(long)]
        accept: Vec<String>,
        #[arg(long, default_value = "")]
        target: String,
    },
    /// Build operand anchors (a layout entry as the value of an instruction's
    /// operand) and print them as an anchor set (JSON).
    SuggestOperand {
        binary: PathBuf,
        /// `name=0xINSN:disp|imm@within[~base]`: the instruction, which
        /// operand, the (function) anchor whose function contains it, and
        /// optionally the operand anchor its value is relative to.
        #[arg(required = true)]
        specs: Vec<String>,
        /// At most this many locators per anchor (the instruction itself, then
        /// others in the function carrying the same value).
        #[arg(long, default_value_t = 3)]
        max_locators: usize,
    },
    /// Build operand anchors from values: each the first instruction in a
    /// function carrying the value (a layout entry measured from source, in a
    /// build whose code moved). Prints an anchor set (JSON).
    SuggestValue {
        binary: PathBuf,
        /// `name=VALUE:disp|imm@within=0xFUNCTION`: the value, which
        /// operand, and the (function) anchor plus its address in this build.
        #[arg(required = true)]
        specs: Vec<String>,
        #[arg(long, default_value_t = 3)]
        max_locators: usize,
    },
    /// Find where a value is carried under a function's calls: prints
    /// `{"calls": [VA, ...], "insn": VA}` (or `null`) as JSON.
    FindValue {
        binary: PathBuf,
        /// The function's address.
        #[arg(value_parser = parse_hex)]
        function: u64,
        value: i64,
        /// `disp` or `imm`.
        operand: String,
        #[arg(long, default_value_t = 2)]
        depth: usize,
    },
    /// Build function anchors for callees: each the function a `call` inside
    /// another (function) anchor's function calls, located by that call.
    /// Prints an anchor set (JSON).
    SuggestCall {
        binary: PathBuf,
        /// `name=0xCALL@within`: the call instruction and the anchor whose
        /// function contains it.
        #[arg(required = true)]
        specs: Vec<String>,
    },
    /// Build shape locators: for each 0xINSN, the shortest run of
    /// instructions around it with a unique shape (registers as variables,
    /// no displacements). Prints the locators (JSON), one per line.
    SuggestShape {
        binary: PathBuf,
        #[arg(required = true, value_parser = parse_hex)]
        insns: Vec<u64>,
        /// Unique inside the instruction's own function (for an anchor
        /// `within` it), instead of the whole binary.
        #[arg(long)]
        scoped: bool,
        /// Write the instruction's immediates as `*` (for an `imm` operand).
        #[arg(long)]
        wild_imm: bool,
    },
    /// Print what an operand anchor could read off each instruction of the
    /// function containing ADDR: `[[va, disp|null, imm|null], ...]` (JSON).
    Operands {
        binary: PathBuf,
        #[arg(value_parser = parse_hex)]
        addr: u64,
    },
    /// Give anchors this build weakened new locators, where at least two
    /// still agree. Prints the healed set (JSON); what was healed and what
    /// wasn't goes to stderr.
    Heal {
        anchors: PathBuf,
        binary: PathBuf,
        /// Write the healed set here instead of printing it.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Diff two saved resolution reports (from `resolve --out`).
    DiffReport {
        before: PathBuf,
        after: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    match Cli::parse().cmd {
        Cmd::Heal {
            anchors,
            binary,
            out,
        } => heal_cmd(&anchors, &binary, out.as_deref()),
        Cmd::Suggest {
            binary,
            functions,
            max_strings,
            source,
            accept,
            target,
        } => suggest_cmd(&binary, &functions, max_strings, &source, &accept, target),
        Cmd::SuggestOperand {
            binary,
            specs,
            max_locators,
        } => {
            let bytes = read(&binary)?;
            print_suggested(&specs, String::new(), |spec| {
                operand_anchor(&bytes, spec, max_locators)
            })
        }
        Cmd::SuggestValue {
            binary,
            specs,
            max_locators,
        } => {
            let bytes = read(&binary)?;
            print_suggested(&specs, String::new(), |spec| {
                value_anchor(&bytes, spec, max_locators)
            })
        }
        Cmd::SuggestCall { binary, specs } => {
            let bytes = read(&binary)?;
            print_suggested(&specs, String::new(), |spec| call_anchor(&bytes, spec))
        }
        Cmd::FindValue {
            binary,
            function,
            value,
            operand,
            depth,
        } => {
            let found = rekindle_anchor::find_value(
                &read(&binary)?,
                function,
                value,
                parse_operand(&operand)?,
                depth,
            )?;
            let json =
                found.map(|(calls, insn)| serde_json::json!({ "calls": calls, "insn": insn }));
            println!("{}", serde_json::to_string(&json)?);
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Operands { binary, addr } => {
            println!(
                "{}",
                serde_json::to_string(&rekindle_anchor::operands(&read(&binary)?, addr)?)?
            );
            Ok(ExitCode::SUCCESS)
        }
        Cmd::SuggestShape {
            binary,
            insns,
            scoped,
            wild_imm,
        } => shape_cmd(&binary, &insns, scoped, wild_imm),
        Cmd::Resolve {
            anchors,
            binary,
            json,
            out,
            allow_broken,
        } => resolve_cmd(&anchors, &binary, json, out.as_deref(), allow_broken),
        Cmd::Diff {
            anchors,
            old_binary,
            new_binary,
            json,
        } => {
            let set = AnchorSet::from_json(&read(&anchors)?)?;
            let before = set.resolve(&read(&old_binary)?);
            let after = set.resolve(&read(&new_binary)?);
            emit_diff(&diff(&before, &after), json)
        }
        Cmd::DiffReport {
            before,
            after,
            json,
        } => {
            let b = ResolutionReport::from_json(&read(&before)?)?;
            let a = ResolutionReport::from_json(&read(&after)?)?;
            emit_diff(&diff(&b, &a), json)
        }
    }
}

fn read(p: &Path) -> Result<Vec<u8>> {
    std::fs::read(p).with_context(|| format!("reading {}", p.display()))
}

/// A hexadecimal address, with or without `0x`.
fn parse_hex(s: &str) -> Result<u64> {
    let digits = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u64::from_str_radix(digits, 16).with_context(|| format!("bad address {s:?}"))
}

/// `disp` or `imm`.
fn parse_operand(kind: &str) -> Result<Operand> {
    match kind {
        "disp" => Ok(Operand::Disp),
        "imm" => Ok(Operand::Imm),
        k => bail!("unknown operand kind {k:?}"),
    }
}

fn exit_code(failed: bool) -> ExitCode {
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Builds an anchor from each spec and prints them as one anchor set. A
/// spec that fails is reported on stderr and fails the command, after the
/// others are printed.
fn print_suggested(
    specs: &[String],
    target: String,
    suggest: impl Fn(&str) -> Result<Anchor>,
) -> Result<ExitCode> {
    let mut anchors = Vec::new();
    let mut failed = false;
    for spec in specs {
        match suggest(spec) {
            Ok(a) => anchors.push(a),
            Err(e) => {
                eprintln!("{spec}: {e:#}");
                failed = true;
            }
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&AnchorSet { target, anchors })?
    );
    Ok(exit_code(failed))
}

/// `name=0xINSN:disp|imm@within[~base]`.
fn operand_anchor(bytes: &[u8], spec: &str, max_locators: usize) -> Result<Anchor> {
    let (name, rest) = spec
        .split_once('=')
        .context("expected name=0xINSN:kind@within")?;
    let (insn, rest) = rest.split_once(':').context("expected :disp or :imm")?;
    let (kind, within) = rest.split_once('@').context("expected @within")?;
    let (within, base) = match within.split_once('~') {
        Some((w, b)) => (w, Some(b)),
        None => (within, None),
    };
    rekindle_anchor::suggest_operand(
        bytes,
        name,
        parse_hex(insn)?,
        parse_operand(kind)?,
        within,
        base,
        max_locators,
    )
}

/// `name=VALUE:disp|imm@within=0xFUNCTION`.
fn value_anchor(bytes: &[u8], spec: &str, max_locators: usize) -> Result<Anchor> {
    let (name, rest) = spec
        .split_once('=')
        .context("expected name=VALUE:kind@within=0xFN")?;
    let (value, rest) = rest.split_once(':').context("expected :disp or :imm")?;
    let (kind, rest) = rest.split_once('@').context("expected @within=0xFN")?;
    let (within, function) = rest.split_once('=').context("expected within=0xFN")?;
    rekindle_anchor::suggest_operand_by_value(
        bytes,
        name,
        parse_hex(function)?,
        value.parse()?,
        parse_operand(kind)?,
        within,
        max_locators,
    )
}

/// `name=0xCALL@within`.
fn call_anchor(bytes: &[u8], spec: &str) -> Result<Anchor> {
    let (name, rest) = spec
        .split_once('=')
        .context("expected name=0xCALL@within")?;
    let (insn, within) = rest.split_once('@').context("expected @within")?;
    rekindle_anchor::suggest_call(bytes, name, parse_hex(insn)?, within)
}

/// Prints a shape locator per instruction, one per line.
fn shape_cmd(binary: &Path, insns: &[u64], scoped: bool, wild_imm: bool) -> Result<ExitCode> {
    let bytes = read(binary)?;
    let mut failed = false;
    for &insn in insns {
        match rekindle_anchor::suggest_shape(&bytes, insn, scoped, wild_imm) {
            Ok(loc) => println!("{}", serde_json::to_string(&loc)?),
            Err(e) => {
                eprintln!("{insn:#x}: {e:#}");
                failed = true;
            }
        }
    }
    Ok(exit_code(failed))
}

fn heal_cmd(anchors: &Path, binary: &Path, out: Option<&Path>) -> Result<ExitCode> {
    let set = AnchorSet::from_json(&read(anchors)?)?;
    let bytes = read(binary)?;
    let report = set.resolve(&bytes);
    let h = rekindle_anchor::heal(&set, &report, &bytes);
    for (name, kept, now) in &h.healed {
        eprintln!("healed  {name}: {kept} locators agreed, {now} now");
    }
    for (name, why) in &h.skipped {
        eprintln!("skipped {name}: {why}");
    }
    let json = serde_json::to_string_pretty(&h.set)?;
    match out {
        Some(p) => {
            std::fs::write(p, json + "\n").with_context(|| format!("writing {}", p.display()))?;
        }
        None => println!("{json}"),
    }
    Ok(ExitCode::SUCCESS)
}

fn suggest_cmd(
    binary: &Path,
    functions: &[String],
    max_strings: usize,
    source: &[String],
    accept: &[String],
    target: String,
) -> Result<ExitCode> {
    let bytes = read(binary)?;
    let mut sources = BTreeMap::new();
    for s in source {
        let (name, path) = s
            .split_once("=@")
            .with_context(|| format!("expected name=@file, got {s:?}"))?;
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
        sources.insert(name, Signature::parse(text.trim())?);
    }
    let mut accepts: BTreeMap<&str, Vec<u64>> = BTreeMap::new();
    for a in accept {
        let (name, list) = a
            .split_once('=')
            .with_context(|| format!("expected name=0xA,0xB, got {a:?}"))?;
        let addrs = list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(parse_hex)
            .collect::<Result<_>>()?;
        accepts.insert(name, addrs);
    }
    print_suggested(functions, target, |f| {
        let (name, addr) = f.split_once('=').context("expected name=0xADDR")?;
        rekindle_anchor::suggest(
            &bytes,
            name,
            parse_hex(addr)?,
            max_strings,
            sources.get(name),
            accepts.get(name).map_or(&[][..], Vec::as_slice),
        )
    })
}

fn resolve_cmd(
    anchors: &Path,
    binary: &Path,
    json: bool,
    out: Option<&Path>,
    allow_broken: bool,
) -> Result<ExitCode> {
    let set = AnchorSet::from_json(&read(anchors)?)?;
    let report = set.resolve(&read(binary)?);
    if let Some(path) = out {
        std::fs::write(path, serde_json::to_vec_pretty(&report)?)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_resolution(&binary.display().to_string(), &report);
    }
    Ok(exit_code(!report.broken().is_empty() && !allow_broken))
}

fn emit_diff(d: &DiffReport, json: bool) -> Result<ExitCode> {
    if json {
        println!("{}", serde_json::to_string_pretty(d)?);
    } else {
        print_diff(d);
    }
    // CI gate: a break fails the build.
    Ok(exit_code(!d.broke().is_empty()))
}

fn print_resolution(binary: &str, report: &ResolutionReport) {
    println!("anchors in {binary}");
    if !report.target.is_empty() {
        println!("  target: {}", report.target);
    }
    let s = &report.summary;
    println!(
        "  {} anchors: {} resolved, {} ambiguous, {} broken\n",
        s.total, s.resolved, s.ambiguous, s.broken
    );
    for a in &report.anchors {
        let mark = match a.verdict {
            Verdict::Resolved => "ok  ",
            Verdict::Ambiguous => "AMB ",
            Verdict::Broken => "BROK",
            Verdict::Conflict => "CONF",
        };
        let loc = match a.offset() {
            Some(off) => format!(" @ {off:#x}"),
            None => String::new(),
        };
        println!(
            "  [{mark}] {:<28} {}/{} locators{loc}",
            a.name, a.unique_locators, a.total_locators
        );
        if a.verdict != Verdict::Resolved {
            for l in &a.locators {
                let d = match &l.status {
                    Status::Unique { offset, .. } => format!("unique @ {offset:#x}"),
                    Status::Ambiguous { offsets } => format!("{} matches", offsets.len()),
                    Status::Missing => "missing".to_string(),
                };
                println!("           - {}: {d}", l.label);
            }
        }
    }
}

fn print_diff(d: &DiffReport) {
    let mut regressions = 0;
    for c in &d.changes {
        let tag = match c.change {
            Change::Ok => continue, // quiet on unchanged
            Change::Broke => "BROKE",
            Change::Recovered => "recovered",
            Change::Weakened => "weakened",
            Change::StillBroken => "still-broken",
            Change::New => "new",
            Change::Dropped => "dropped",
        };
        if matches!(c.change, Change::Broke | Change::Weakened) {
            regressions += 1;
        }
        let va = |v: Option<Verdict>| v.map_or_else(|| "-".into(), |v| format!("{v:?}"));
        println!(
            "  {tag:<12} {:<28} {} -> {}",
            c.name,
            va(c.before),
            va(c.after)
        );
    }
    let ok = d.changes.iter().filter(|c| c.change == Change::Ok).count();
    println!(
        "\n{ok} unchanged, {} broke, {regressions} regression(s) total",
        d.broke().len()
    );
    if d.broke().is_empty() && regressions == 0 {
        println!("no anchors broke.");
    }
}
