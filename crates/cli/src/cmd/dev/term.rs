//! Terminal output for the dev server. `anstream` strips styling when stdout
//! isn't a terminal or `NO_COLOR` is set, and enables ANSI on Windows consoles.

use anstyle::{AnsiColor, Style};
use rekindle_plugin_host::model::{LogLevel, LogRecord};

const DIM: Style = Style::new().dimmed();
const BOLD: Style = Style::new().bold();
const RED: Style = AnsiColor::Red.on_default().bold();
const YELLOW: Style = AnsiColor::Yellow.on_default();
const GREEN: Style = AnsiColor::Green.on_default();
const CYAN: Style = AnsiColor::Cyan.on_default();
const MAGENTA: Style = AnsiColor::Magenta.on_default();

pub(super) fn heading(text: &str) {
    anstream::println!("{BOLD}{text}{BOLD:#}");
}

pub(super) fn line(text: &str) {
    anstream::println!("{text}");
}

pub(super) fn dim(text: &str) {
    anstream::println!("{DIM}{text}{DIM:#}");
}

pub(super) fn ok(text: &str) {
    anstream::println!("{GREEN}✓{GREEN:#} {text}");
}

pub(super) fn note(text: &str) {
    anstream::println!("{CYAN}•{CYAN:#} {text}");
}

pub(super) fn warn(text: &str) {
    anstream::println!("{YELLOW}!{YELLOW:#} {text}");
}

pub(super) fn error(text: &str) {
    anstream::println!("{RED}✗ {text}{RED:#}");
}

/// A multi-line block (e.g. compiler output), indented.
pub(super) fn block(text: &str) {
    for l in text.lines() {
        anstream::println!("    {l}");
    }
}

pub(super) fn url(label: &str, url: &str) {
    anstream::println!("{BOLD}{label}{BOLD:#} {CYAN}{url}{CYAN:#}");
}

pub(super) fn log(rec: &LogRecord) {
    let (style, tag) = match rec.level {
        LogLevel::Error => (RED, "ERROR"),
        LogLevel::Warn => (YELLOW, " WARN"),
        LogLevel::Info => (GREEN, " INFO"),
        LogLevel::Debug => (CYAN, "DEBUG"),
        LogLevel::Trace => (DIM, "TRACE"),
    };
    let secs = rec.at_ms as f64 / 1000.0;
    anstream::println!(
        "{DIM}{secs:>8.3}s{DIM:#} {style}{tag}{style:#} {MAGENTA}{}{MAGENTA:#} {}",
        rec.plugin,
        rec.message
    );
}
