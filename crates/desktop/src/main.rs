//! `rekindle-desktop`: resolve a desktop client build, or check one.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rekindle_desktop::{Image, Symbols};

#[derive(Parser)]
#[command(
    name = "rekindle-desktop",
    about = "Resolve desktop Phoenix's own GPUI types and offsets"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Resolve a build and print what was found.
    Resolve {
        /// The client image (`Discord.exe`).
        #[arg(long)]
        exe: PathBuf,
        /// Write the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Resolve a build and fail if a required shape is missing or changed.
    Check {
        /// The client image (`Discord.exe`).
        #[arg(long)]
        exe: PathBuf,
        /// Write the report as JSON.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("rekindle-desktop: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Resolve { exe, json } => {
            let symbols = resolve(&exe)?;
            print(&symbols, json);
            Ok(())
        }
        Command::Check { exe, json } => {
            let symbols = resolve(&exe)?;
            let report = symbols.report();
            print(&symbols, json);
            let failed: Vec<_> = report.checks.iter().filter(|c| !c.ok).collect();
            if failed.is_empty() {
                if !json {
                    println!(
                        "check: ok ({} fields, StyleRefinement {} bytes)",
                        report.style.len(),
                        report.style_size
                    );
                }
                Ok(())
            } else {
                for check in failed {
                    eprintln!("check: {} failed: {}", check.name, check.detail);
                }
                anyhow::bail!(
                    "{} check(s) failed",
                    report.checks.iter().filter(|c| !c.ok).count()
                )
            }
        }
    }
}

fn resolve(exe: &PathBuf) -> Result<Symbols> {
    let bytes = std::fs::read(exe).with_context(|| format!("read {}", exe.display()))?;
    let image = Image::parse(&bytes).context("parse the client image")?;
    Symbols::resolve(&image)
}

fn print(symbols: &Symbols, json: bool) {
    let report = symbols.report();
    if json {
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
        return;
    }
    println!("image base {:#x}", report.image_base);
    for row in &report.message_row_renders {
        println!("MessageRow::render {:#x} (RVA {:#x})", row.va, row.rva);
    }
    println!(
        "Drawable<Div> {:#x} (RVA {:#x}); {} candidate(s)",
        report.div_vtable.va,
        report.div_vtable.rva,
        report.div_vtables.len()
    );
    println!("StyleRefinement {} bytes", report.style_size);
    for (name, offset) in &report.style {
        println!("  {name:24} +{offset:#x}");
    }
}
