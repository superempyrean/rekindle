//! The on-disk signature set: what `sigs` writes and `scan` reads.
//!
//! One file describes one reference compile (normally one crate at one
//! version), so a scan of it against a target yields a per-crate verdict.

use serde::{Deserialize, Serialize};

use crate::extract::FnSig;

/// A signature set: one reference compile's functions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SigFile {
    /// What was compiled, e.g. `gpui 0.2.0`: free-form provenance.
    pub source: String,
    /// Fixed-byte floor the signatures were built with.
    pub min_significant: usize,
    pub functions: Vec<FnEntry>,
}

/// One function of a [`SigFile`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FnEntry {
    /// Its mangled symbol in the reference object.
    pub symbol: String,
    /// `48 8b ?? ?? e8 ..` form; the signature is reconstructed from this.
    pub pattern: String,
    /// Fixed (non-wildcard) byte count: the specificity signal.
    pub significant: usize,
    /// Longest run of consecutive fixed bytes.
    pub longest_run: usize,
}

impl SigFile {
    /// The set for `fns`, compiled from `source`.
    pub fn build(source: String, min_significant: usize, fns: &[FnSig]) -> Self {
        let functions = fns
            .iter()
            .map(|f| FnEntry {
                symbol: f.symbol.clone(),
                pattern: f.sig.to_pattern(),
                significant: f.sig.significant(),
                longest_run: f.sig.longest_fixed_run(),
            })
            .collect();
        Self {
            source,
            min_significant,
            functions,
        }
    }
}
