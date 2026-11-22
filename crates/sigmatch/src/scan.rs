//! Match a signature set against a target binary.
//!
//! We search only the executable sections (`.text` and friends): a function's
//! code cannot live in `.rodata`, and skipping data sections both speeds the
//! scan and removes a source of coincidental hits. If no section table can be
//! read, we fall back to scanning the whole file so the tool still does
//! something useful on an unusual container.

use anyhow::{Context, Result};
use object::{Object, ObjectSection, SectionKind};
use serde::Serialize;

use crate::sig::Signature;
use crate::sigfile::SigFile;

/// Where one function of a signature set matched.
#[derive(Debug, Clone, Serialize)]
pub struct FnMatch {
    pub symbol: String,
    pub significant: usize,
    /// Offsets in the file where the signature matched. More than one means
    /// the signature isn't unique, which is worth knowing before trusting it.
    pub hits: Vec<usize>,
}

/// The result of [`scan`]: each function of the set, by how often it
/// matched.
#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    /// The set's provenance, from [`SigFile::source`].
    pub source: String,
    /// Functions in the set.
    pub total: usize,
    /// Functions that matched exactly once: the confident locations.
    pub unique: Vec<FnMatch>,
    /// Functions that matched more than once.
    pub ambiguous: Vec<FnMatch>,
    /// Functions that matched nowhere.
    pub missing: usize,
}

impl ScanReport {
    /// Fraction of reference functions located at least once, [0,1].
    pub fn recall(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        (self.unique.len() + self.ambiguous.len()) as f64 / self.total as f64
    }
}

/// Byte ranges of the file that hold executable code. Empty if none resolve.
fn exec_ranges(bytes: &[u8]) -> Vec<(usize, usize)> {
    let Ok(obj) = object::File::parse(bytes) else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    for sec in obj.sections() {
        if sec.kind() != SectionKind::Text {
            continue;
        }
        if let Some((off, size)) = sec.file_range() {
            let off = off as usize;
            if let Some(end) = off.checked_add(size as usize)
                && end <= bytes.len()
            {
                ranges.push((off, end));
            }
        }
    }
    ranges
}

/// Matches every function of `file` against the executable sections of
/// `bytes`. Fails if a pattern in the set doesn't parse.
pub fn scan(file: &SigFile, bytes: &[u8]) -> Result<ScanReport> {
    let ranges = exec_ranges(bytes);
    // Whole-file fallback keeps the tool working on containers we can't parse.
    let search: Vec<(usize, usize)> = if ranges.is_empty() {
        vec![(0, bytes.len())]
    } else {
        ranges
    };

    let mut unique = Vec::new();
    let mut ambiguous = Vec::new();
    let mut missing = 0;
    // Index each range once; every signature then verifies only candidates.
    let indexes: Vec<(usize, crate::sig::TrigramIndex)> = search
        .iter()
        .map(|&(lo, hi)| (lo, crate::sig::TrigramIndex::new(&bytes[lo..hi])))
        .collect();

    for entry in &file.functions {
        let sig = Signature::parse(&entry.pattern)
            .with_context(|| format!("bad pattern for {}", entry.symbol))?;

        let mut hits = Vec::new();
        for (lo, index) in &indexes {
            for local in index.find(&sig) {
                hits.push(lo + local);
            }
        }

        let m = FnMatch {
            symbol: entry.symbol.clone(),
            significant: entry.significant,
            hits,
        };
        match m.hits.len() {
            0 => missing += 1,
            1 => unique.push(m),
            _ => ambiguous.push(m),
        }
    }

    Ok(ScanReport {
        source: file.source.clone(),
        total: file.functions.len(),
        unique,
        ambiguous,
        missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sigfile::FnEntry;

    fn file_with(patterns: &[&str]) -> SigFile {
        SigFile {
            source: "test 0.0.0".into(),
            min_significant: 1,
            functions: patterns
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let sig = Signature::parse(p).unwrap();
                    FnEntry {
                        symbol: format!("f{i}"),
                        pattern: p.to_string(),
                        significant: sig.significant(),
                        longest_run: sig.longest_fixed_run(),
                    }
                })
                .collect(),
        }
    }

    #[test]
    fn trigram_index_agrees_with_linear_scan() {
        use crate::sig::{Signature, TrigramIndex};
        let mut hay = Vec::new();
        for i in 0..5000u32 {
            hay.extend_from_slice(&(i.wrapping_mul(2654435761)).to_le_bytes());
        }
        hay.extend_from_slice(&[
            0x48, 0x89, 0x5c, 0x24, 0x08, 0x57, 0x48, 0x89, 0x5c, 0x24, 0x10, 0x57,
        ]);
        let idx = TrigramIndex::new(&hay);
        for p in [
            "48 89 5c 24 ?? 57",
            "48 ?? 5c",
            "5c 24 08",
            "?? 89 5c 24 10",
            "48 89",
        ] {
            let sig = Signature::parse(p).unwrap();
            assert_eq!(idx.find(&sig), sig.scan(&hay), "{p}");
        }
    }

    #[test]
    fn classifies_unique_ambiguous_missing() {
        // No parseable object → whole-file fallback, so offsets are absolute.
        let hay = [
            0xaa, 0xbb, 0xcc, // f0: unique
            0x11, 0x22, //       f1 first occurrence
            0x00, 0x11, 0x22, // f1 second occurrence
        ];
        let file = file_with(&["aa bb cc", "11 22", "de ad be ef"]);
        let rep = scan(&file, &hay).unwrap();
        assert_eq!(rep.total, 3);
        assert_eq!(rep.unique.len(), 1);
        assert_eq!(rep.unique[0].symbol, "f0");
        assert_eq!(rep.ambiguous.len(), 1);
        assert_eq!(rep.ambiguous[0].hits.len(), 2);
        assert_eq!(rep.missing, 1);
        assert!((rep.recall() - 2.0 / 3.0).abs() < 1e-9);
    }
}
