//! Host-side mirrors of the WIT data types.
//!
//! Backends (the mock client, the dev preview and the extension inside the
//! client) work with these plain, serde-friendly types instead of the
//! generated bindings, so they don't depend on wasmtime's codegen.

mod actions;
mod color;
mod elements;
mod services;
mod slots;
mod ui;

pub use actions::*;
pub use color::*;
pub use elements::*;
pub use services::*;
pub use slots::*;
pub use ui::*;

use serde::{Deserialize, Serialize};

/// A log record's severity, lowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// One record of a plugin's log. `at_ms` counts from the runtime's start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogRecord {
    pub plugin: String,
    pub level: LogLevel,
    pub message: String,
    pub at_ms: u64,
}
