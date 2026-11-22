//! Recover the `rustc` build identity from embedded standard-library paths.
//!
//! When the standard library panics (bounds checks, `unwrap`, etc.), the
//! compiler bakes the *source* location into the binary. For std, that path is
//! rooted at the toolchain sysroot and looks like:
//!
//!   /rustc/<commit>/library/core/src/slice/index.rs
//!   /rustc/<commit>/`library/alloc/src/raw_vec.rs`
//!
//! `<commit>` is the 40-hex git hash of the exact compiler. That hash maps
//! 1:1 to a released version (we can resolve it against rustc's release
//! metadata, or just use it directly to fetch a matching toolchain). On some
//! builds the path is a relative `library/std/src/...` with no commit, which
//! still confirms "built with rustc" even if it doesn't pin the version.
//!
//! An ELF also names the compiler in its `.comment` section, version and all:
//! `rustc version 1.99.0-nightly (73dc9167f 2026-08-01)`. A PE has no such
//! section.

use std::collections::BTreeMap;

use regex::Regex;
use serde::Serialize;

use crate::strings::Located;

#[derive(Debug, Default, Serialize)]
pub struct RustcInfo {
    /// The version from an ELF's `.comment`, e.g.
    /// `1.99.0-nightly (73dc9167f 2026-08-01)`. One per linked toolchain.
    pub versions: Vec<String>,
    /// Distinct 40-hex commit hashes seen in `/rustc/<hash>/` paths.
    pub commit_hashes: Vec<String>,
    /// How many std-path strings referenced each commit (confidence signal).
    pub hash_hits: BTreeMap<String, usize>,
    /// True if we saw std library paths at all (even without a commit hash).
    pub std_paths_present: bool,
    /// A few example std paths, for eyeballing.
    pub sample_paths: Vec<String>,
}

impl RustcInfo {
    /// The best-guess compiler commit: the most frequently referenced hash.
    pub fn primary_commit(&self) -> Option<&str> {
        self.hash_hits
            .iter()
            .max_by_key(|(_, n)| **n)
            .map(|(h, _)| h.as_str())
    }
}

pub fn detect(strings: &[Located]) -> RustcInfo {
    // `/rustc/<40 hex>/library/...`. Path separators are normalized per build
    // host: a Windows build yields `/rustc/<hash>\library\core\...`, so accept
    // either `/` or `\` between segments.
    let commit_re = Regex::new(r"[/\\]rustc[/\\]([0-9a-f]{40})[/\\]library[/\\]")
        .expect("the pattern is valid");
    // Any std library source path, with or without the commit prefix.
    let stdpath_re =
        Regex::new(r"(?:^|[/\\])library[/\\](?:core|alloc|std|proc_macro)[/\\]src[/\\]")
            .expect("the pattern is valid");

    let version_re = Regex::new(
        r"^rustc version (\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)? \([0-9a-f]+ \d{4}-\d{2}-\d{2}\))",
    )
    .expect("the pattern is valid");

    let mut info = RustcInfo::default();

    for s in strings {
        if let Some(cap) = version_re.captures(&s.text)
            && !info.versions.contains(&cap[1].to_string())
        {
            info.versions.push(cap[1].to_string());
        }
        if let Some(cap) = commit_re.captures(&s.text) {
            let hash = cap[1].to_string();
            *info.hash_hits.entry(hash).or_insert(0) += 1;
        }
        if stdpath_re.is_match(&s.text) {
            info.std_paths_present = true;
            if info.sample_paths.len() < 8 {
                info.sample_paths.push(s.text.clone());
            }
        }
    }

    info.commit_hashes = info.hash_hits.keys().cloned().collect();
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> Located {
        Located {
            offset: 0,
            text: text.to_string(),
        }
    }

    #[test]
    fn extracts_commit_hash() {
        let hash = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0";
        let strings = vec![
            s(&format!("/rustc/{hash}/library/core/src/slice/index.rs")),
            s(&format!("/rustc/{hash}/library/alloc/src/raw_vec.rs")),
            s("unrelated string"),
        ];
        let info = detect(&strings);
        assert_eq!(info.primary_commit(), Some(hash));
        assert_eq!(info.hash_hits[hash], 2);
        assert!(info.std_paths_present);
    }

    #[test]
    fn windows_backslash_std_path() {
        // Windows builds remap to `/rustc/<hash>` but keep backslashes after.
        let hash = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0";
        let strings = vec![s(&format!(
            r"/rustc/{hash}\library\core\src\slice\index.rs"
        ))];
        let info = detect(&strings);
        assert_eq!(info.primary_commit(), Some(hash));
        assert!(info.std_paths_present);
    }

    #[test]
    fn elf_comment_version() {
        let strings = vec![
            s("Linker: LLD 18.0.2"),
            s("rustc version 1.99.0-nightly (73dc9167f 2026-08-01)"),
            s("rustc version 1.100.0 (0ef1bc386 2026-11-09)"),
        ];
        let info = detect(&strings);
        assert_eq!(
            info.versions,
            [
                "1.99.0-nightly (73dc9167f 2026-08-01)",
                "1.100.0 (0ef1bc386 2026-11-09)"
            ]
        );
    }

    #[test]
    fn std_paths_without_commit_still_flagged() {
        let strings = vec![s("library/std/src/panic.rs")];
        let info = detect(&strings);
        assert!(info.std_paths_present);
        assert!(info.primary_commit().is_none());
    }
}
