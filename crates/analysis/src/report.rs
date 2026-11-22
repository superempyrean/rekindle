//! Assemble and render the triage result.

use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::crates::{CrateHit, Source};
use crate::gpui_stack::{self, Core, GpuiStack};
use crate::rustc_id::RustcInfo;
use crate::sections::SectionSpan;

/// What `rekindle-analyze` found in one binary.
#[derive(Serialize)]
pub struct Report {
    /// The binary's path.
    pub binary: String,
    pub size_bytes: usize,
    pub rustc: RustcInfo,
    pub crates: Vec<CrateHit>,
    /// Where GPUI comes from (Zed git, crates.io gpui / gpui-pre, in-tree
    /// fork) and which toolkit layers sit on top.
    pub gpui: GpuiStack,
    /// Section table of the container, when it parsed as ELF/PE/Mach-O.
    pub sections: Vec<SectionSpan>,
}

impl Report {
    pub fn new(
        path: &Path,
        size: usize,
        rustc: RustcInfo,
        crates: Vec<CrateHit>,
        sections: Vec<SectionSpan>,
    ) -> Self {
        Self {
            binary: path.display().to_string(),
            size_bytes: size,
            rustc,
            gpui: gpui_stack::classify(&crates),
            crates,
            sections,
        }
    }

    /// The report as pretty-printed JSON.
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Prints the report as text; `verbose` adds example paths.
    pub fn print_text(&self, verbose: bool) {
        println!("binary : {}", self.binary);
        println!("size   : {} bytes", self.size_bytes);
        println!();

        println!("== rustc ==");
        for v in &self.rustc.versions {
            println!("  version         : {v}");
        }
        match self.rustc.primary_commit() {
            Some(commit) => {
                println!("  compiler commit : {commit}");
                if self.rustc.versions.is_empty() {
                    println!(
                        "  (resolve to a version via rustc release metadata, or fetch the \n   matching toolchain to compile byte-comparable copies of deps)"
                    );
                }
            }
            None if self.rustc.std_paths_present => {
                println!("  built with rustc (std paths present) but no /rustc/<commit>/ prefix");
            }
            None => println!("  no Rust std signature found (is this a Rust binary?)"),
        }
        if self.rustc.commit_hashes.len() > 1 {
            println!(
                "  note: {} distinct commit hashes seen",
                self.rustc.commit_hashes.len()
            );
        }
        if verbose {
            for p in &self.rustc.sample_paths {
                println!("    std path: {p}");
            }
        }
        println!();

        println!("== crates ({} distinct) ==", self.crates.len());
        if self.crates.is_empty() {
            println!("  none detected");
        }
        for h in &self.crates {
            let ver = h.version.as_deref().unwrap_or("?");
            let src = match h.source {
                Source::Registry => "registry",
                Source::Git => "git",
                Source::Workspace => "workspace",
                Source::Toolchain => "toolchain",
            };
            let anchor = match &h.section {
                Some(sec) => format!("@{:#x} [{sec}]", h.offset),
                None => format!("@{:#x}", h.offset),
            };
            println!(
                "  {:<28} {:<16} [{src}] x{:<4} {anchor}",
                h.name, ver, h.occurrences
            );
            if verbose {
                println!("      e.g. {}", h.example);
            }
        }

        if !self.gpui.is_empty() {
            println!();
            println!("== gpui stack ==");
            for c in &self.gpui.core {
                match c {
                    Core::Registry { package, version } => {
                        println!("  core   : {package} {version} (crates.io)");
                    }
                    Core::Git { checkout, rev } => println!(
                        "  core   : gpui via git ({} @ {})",
                        checkout.as_deref().unwrap_or("?"),
                        rev.as_deref().unwrap_or("?")
                    ),
                    Core::Workspace => println!(
                        "  core   : gpui from a workspace path (vendored/forked, unversioned)"
                    ),
                }
            }
            for l in &self.gpui.layers {
                println!(
                    "  layer  : {} {}",
                    l.name,
                    l.version.as_deref().unwrap_or("?")
                );
            }
            for h in &self.gpui.hints {
                println!("  -> {h}");
            }
        }

        if !self.sections.is_empty() {
            println!();
            println!("== sections ({}) ==", self.sections.len());
            for s in &self.sections {
                println!(
                    "  {:<20} file {:#010x} + {:#x}",
                    s.name, s.file_start, s.file_len
                );
            }
        }
    }
}
