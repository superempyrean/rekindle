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
    /// Resolve the client's `div()` constructor and its layout.
    Div {
        /// The client image (`Discord.exe`).
        #[arg(long)]
        exe: PathBuf,
        /// Write the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Resolve the client's `SharedString` text element and its
    /// `AnyElement` conversion.
    Text {
        /// The client image (`Discord.exe`).
        #[arg(long)]
        exe: PathBuf,
        /// Write the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Resolve the message context menu's `MenuItem` layout.
    Menu {
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
        Command::Div { exe, json } => {
            let bytes = std::fs::read(&exe).with_context(|| format!("read {}", exe.display()))?;
            let image = Image::parse(&bytes).context("parse the client image")?;
            let div = rekindle_desktop::div::resolve(&image)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&div)?);
            } else {
                print_div(&div, image.base());
            }
            Ok(())
        }
        Command::Text { exe, json } => {
            let bytes = std::fs::read(&exe).with_context(|| format!("read {}", exe.display()))?;
            let image = Image::parse(&bytes).context("parse the client image")?;
            let text = rekindle_desktop::text::resolve(&image)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&text)?);
            } else {
                print_text(&text, image.base());
            }
            Ok(())
        }
        Command::Menu { exe, json } => {
            let bytes = std::fs::read(&exe).with_context(|| format!("read {}", exe.display()))?;
            let image = Image::parse(&bytes).context("parse the client image")?;
            let menu = rekindle_desktop::Menu::resolve(&image)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&menu)?);
            } else {
                print_menu(&menu, image.base());
            }
            Ok(())
        }
        Command::Check { exe, json } => {
            let bytes = std::fs::read(&exe).with_context(|| format!("read {}", exe.display()))?;
            let image = Image::parse(&bytes).context("parse the client image")?;
            let symbols = Symbols::resolve(&image)?;
            let menu = rekindle_desktop::Menu::resolve(&image)?;
            let div = rekindle_desktop::div::resolve(&image)?;
            let text = rekindle_desktop::text::resolve(&image)?;
            let report = symbols.report();
            if json {
                let out = serde_json::json!({ "symbols": report, "menu": menu, "div": div, "text": text });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                print(&symbols, false);
                print_menu(&menu, image.base());
                print_div(&div, image.base());
                print_text(&text, image.base());
            }
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

fn print_text(text: &rekindle_desktop::Text, base: u64) {
    println!(
        "SharedString into_any {:#x} (RVA {:#x}); Drawable<SharedString> vtable {:#x}, {:#x} bytes, element at +{:#x}",
        text.into_any,
        text.into_any - base,
        text.vtable,
        text.drawable_size,
        text.element_at
    );
}

fn print_div(div: &rekindle_desktop::Div, base: u64) {
    println!(
        "div() {:#x} (RVA {:#x}) via init {:#x}; Div {:#x} bytes, Interactivity at +{:#x}, style at +{:#x} (accessor {:#x}), into_any {:#x}, paint {:#x}, Drawable element +{:#x}",
        div.new,
        div.new - base,
        div.initialize,
        div.size,
        div.interactivity,
        div.style_at,
        div.style_fn,
        div.into_any,
        div.paint,
        div.drawable_element
    );
    println!(
        "children SmallVec at +{:#x}, {:#x}-byte elements, {:#x} inline, length/capacity at +{:#x}",
        div.children.at, div.children.element, div.children.inline, div.children.capacity_at
    );
}

fn print_menu(menu: &rekindle_desktop::Menu, base: u64) {
    println!(
        "MessageContextMenu {:#x} (RVA {:#x})",
        menu.build_fn,
        menu.build_fn - base
    );
    println!(
        "Vec<MenuItem>::push {:#x} (RVA {:#x})",
        menu.vec_push,
        menu.vec_push - base
    );
    println!(
        "builders: props_new {:#x}, with_id {:#x}, with_icon {:#x}, with_color {:#x}",
        menu.props_new, menu.with_id, menu.with_icon, menu.with_color
    );
    println!(
        "item {:#x} bytes, props at {:#x} ({:#x} bytes)",
        menu.item_size, menu.props, menu.props_size
    );
    println!(
        "label at +{:#x} (tag), +{:#x} (ptr), +{:#x} (len); Action tag {}",
        menu.label.tag, menu.label.ptr, menu.label.len, menu.action_tag
    );
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
