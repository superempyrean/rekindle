//! Recover vendored crate names and versions from embedded registry paths.
//!
//! Cargo compiles dependencies from a per-user registry cache, and the compiler
//! bakes those source paths into panic locations and some debug strings:
//!
//!   .../registry/src/index.crates.io-6f17d22bba15001f/gpui-0.2.0/src/app.rs
//!   .../registry/src/github.com-1ecc6299db9ec823/tokio-1.40.0/src/net/mod.rs
//!   .../git/checkouts/zed-<hash>/<rev>/crates/gpui/src/...   (git deps)
//!
//! A Bazel build (`rules_rust`'s `crate_universe`) keeps each crate in an
//! external repository instead, named after the crate and its version:
//!
//!   external/+crate+crates__tokio-1.54.1/src/net/mod.rs
//!
//! From `<name>-<semver>/` we get both the crate and its exact version. That is the
//! single most useful thing for the rest of the pipeline, since it lets us
//! compile a byte-matching copy and read true type layouts / fn signatures.
//!
//! We also catch workspace-local crate paths (`.../crates/<name>/src/...`),
//! which for an app like Zed reveal its internal crate structure even though
//! they carry no version.

use std::collections::BTreeMap;

use regex::Regex;
use serde::Serialize;

use crate::strings::Located;

/// A crate the binary's strings name, with how often and where.
#[derive(Debug, Clone, Serialize)]
pub struct CrateHit {
    pub name: String,
    /// Semver if the path carried `<name>-<version>/`; `None` for git/workspace.
    pub version: Option<String>,
    /// How this crate was referenced.
    pub source: Source,
    /// Number of string occurrences: a rough confidence / surface-area signal.
    pub occurrences: usize,
    /// One example path for inspection.
    pub example: String,
    /// File offset of `example`'s first byte: the anchor location.
    pub offset: usize,
    /// Section `offset` falls in (`.rodata`/`.rdata`/…), filled by
    /// [`annotate_sections`]. `None` until then, or if no section matches.
    pub section: Option<String>,
}

/// Where a crate's source path says it came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// From a `registry/src/.../<name>-<ver>/` path, or Bazel's
    /// `external/<repo>__<name>-<ver>/`.
    Registry,
    /// From a `git/checkouts/.../` path.
    Git,
    /// From a workspace-local `crates/<name>/` path (no version).
    Workspace,
    /// From `/rust/deps/<name>-<ver>/` or `/cargo/registry/<hash>/<name>-<ver>/`:
    /// crates vendored into the precompiled
    /// std (e.g. `gimli`, `addr2line` for backtraces). These describe the
    /// toolchain, not the application's own dependency graph.
    Toolchain,
}

/// The crates whose source paths appear in `strings`, one row per name,
/// version and source, most referenced first.
pub fn detect(strings: &[Located]) -> Vec<CrateHit> {
    // Path separators depend on the build host (`/` on Unix, `\` on Windows),
    // so every segment boundary below accepts either.

    // registry: capture name and version from `<name>-<semver>/`
    let registry_re = Regex::new(
        r"registry[/\\]src[/\\][^/\\]+[/\\]([A-Za-z0-9_.-]+?)-(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)[/\\]",
    )
    .expect("the pattern is valid");
    // Bazel `crate_universe`: `external/<repo>__<name>-<semver>/`. Crate
    // names never contain `__`, so the last one ends the repository's name.
    let bazel_re = Regex::new(
        r"external[/\\][^/\\]*__([A-Za-z0-9_.-]+?)-(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)[/\\]",
    )
    .expect("the pattern is valid");
    // git checkouts: capture the crate dir after a `crates/` segment if present,
    // else the checkout name.
    let git_re =
        Regex::new(r"git[/\\]checkouts[/\\][^/\\]+[/\\][^/\\]+[/\\](?:.*[/\\])?crates[/\\]([A-Za-z0-9_-]+)[/\\]")
            .expect("the pattern is valid");
    // workspace-local: `.../crates/<name>/src/...`
    let workspace_re = Regex::new(r"(?:^|[/\\])crates[/\\]([A-Za-z0-9_-]+)[/\\]src[/\\]")
        .expect("the pattern is valid");
    // std's vendored deps, remapped by the toolchain build to `/rust/deps/`,
    // or to `/cargo/registry/<16 hex>/` by newer toolchains.
    let toolchain_re = Regex::new(
        r"[/\\](?:rust[/\\]deps|cargo[/\\]registry[/\\][0-9a-f]{16})[/\\]([A-Za-z0-9_.-]+?)-(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)[/\\]",
    )
    .expect("the pattern is valid");

    // Key by (name, version, source) so the same crate from two sources stays
    // distinct, but repeated references collapse into one row with a count.
    let mut map: BTreeMap<(String, Option<String>, Source), CrateHit> = BTreeMap::new();

    let bump = |name: String,
                version: Option<String>,
                source: Source,
                example: &str,
                offset: usize,
                map: &mut BTreeMap<(String, Option<String>, Source), CrateHit>| {
        let key = (name.clone(), version.clone(), source);
        let entry = map.entry(key).or_insert_with(|| CrateHit {
            name,
            version,
            source,
            occurrences: 0,
            example: example.to_string(),
            offset,
            section: None,
        });
        entry.occurrences += 1;
    };

    // Rust string literals are not NUL-terminated, so the linker often packs
    // several panic paths into one printable run. Take every match per run,
    // and keep the spans so a path already attributed to the registry or a git
    // checkout isn't double-counted by the looser workspace pattern.
    let versioned = [
        (registry_re, Source::Registry),
        (bazel_re, Source::Registry),
        (toolchain_re, Source::Toolchain),
    ];
    for s in strings {
        let mut claimed: Vec<std::ops::Range<usize>> = Vec::new();
        for (re, source) in &versioned {
            for c in re.captures_iter(&s.text) {
                let m = c.get_match();
                claimed.push(m.range());
                bump(
                    c[1].to_string(),
                    Some(c[2].to_string()),
                    *source,
                    &s.text,
                    s.offset + m.start(),
                    &mut map,
                );
            }
        }
        for c in git_re.captures_iter(&s.text) {
            let m = c.get_match();
            claimed.push(m.range());
            bump(
                c[1].to_string(),
                None,
                Source::Git,
                &s.text,
                s.offset + m.start(),
                &mut map,
            );
        }
        for c in workspace_re.captures_iter(&s.text) {
            let m = c.get_match();
            let r = m.range();
            if claimed.iter().any(|k| k.start < r.end && r.start < k.end) {
                continue;
            }
            bump(
                c[1].to_string(),
                None,
                Source::Workspace,
                &s.text,
                s.offset + m.start(),
                &mut map,
            );
        }
    }

    let mut hits: Vec<CrateHit> = map.into_values().collect();
    // Most-referenced first; it's the most useful ordering when scanning output.
    hits.sort_by(|a, b| b.occurrences.cmp(&a.occurrences).then(a.name.cmp(&b.name)));
    hits
}

/// Fill each hit's `section` from its anchor `offset`. Separate from `detect`
/// so string recovery stays independent of container parsing (and testable
/// without a real binary).
pub fn annotate_sections(hits: &mut [CrateHit], sections: &crate::sections::SectionMap) {
    if sections.is_empty() {
        return;
    }
    for h in hits {
        h.section = sections.locate(h.offset).map(str::to_owned);
    }
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
    fn registry_name_and_version() {
        let strings = vec![
            s(
                "/home/u/.cargo/registry/src/index.crates.io-6f17d22bba15001f/tokio-1.40.0/src/net.rs",
            ),
            s(
                "/home/u/.cargo/registry/src/index.crates.io-6f17d22bba15001f/tokio-1.40.0/src/io.rs",
            ),
            s("/home/u/.cargo/registry/src/index.crates.io-6f17d22bba15001f/gpui-0.2.0/src/app.rs"),
        ];
        let hits = detect(&strings);
        let tokio = hits.iter().find(|h| h.name == "tokio").unwrap();
        assert_eq!(tokio.version.as_deref(), Some("1.40.0"));
        assert_eq!(tokio.source, Source::Registry);
        assert_eq!(tokio.occurrences, 2);
        assert!(
            hits.iter()
                .any(|h| h.name == "gpui" && h.version.as_deref() == Some("0.2.0"))
        );
    }

    #[test]
    fn version_with_prerelease() {
        let strings = vec![s(
            "registry/src/index.crates.io-abc/wasmtime-50.0.1-rc.1/src/lib.rs",
        )];
        let hits = detect(&strings);
        assert_eq!(hits[0].name, "wasmtime");
        assert_eq!(hits[0].version.as_deref(), Some("50.0.1-rc.1"));
    }

    #[test]
    fn windows_backslash_registry_path() {
        // Exactly the shape a Windows-built Rust binary embeds.
        let strings = vec![s(
            r"C:\Users\x\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\gpui-0.2.0\src\app.rs",
        )];
        let hits = detect(&strings);
        assert_eq!(hits[0].name, "gpui");
        assert_eq!(hits[0].version.as_deref(), Some("0.2.0"));
        assert_eq!(hits[0].source, Source::Registry);
    }

    #[test]
    fn several_paths_packed_in_one_run() {
        // Shape seen in Zed v1.27.0: adjacent panic locations with no NUL.
        let strings = vec![s(
            "/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/h2-0.4.19/src/a.rs\
             /cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.52.1/src/b.rs\
             /zed/crates/editor/src/editor.rs",
        )];
        let hits = detect(&strings);
        let names: Vec<_> = hits.iter().map(|h| h.name.as_str()).collect();
        assert!(names.contains(&"h2") && names.contains(&"tokio") && names.contains(&"editor"));
    }

    #[test]
    fn bazel_crate_universe_repository() {
        let strings = vec![s(
            "external/+crate+crates__crypto_secretbox-0.1.1/src/lib.rs\
             external/crates__tokio-1.54.1/src/net.rs",
        )];
        let hits = detect(&strings);
        let names: Vec<_> = hits
            .iter()
            .map(|h| (h.name.as_str(), h.version.as_deref(), h.source))
            .collect();
        assert!(names.contains(&("crypto_secretbox", Some("0.1.1"), Source::Registry)));
        assert!(names.contains(&("tokio", Some("1.54.1"), Source::Registry)));
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn std_vendored_deps_are_toolchain() {
        let strings = vec![s("/rust/deps/addr2line-0.25.1/src/function.rs")];
        let hits = detect(&strings);
        assert_eq!(hits[0].name, "addr2line");
        assert_eq!(hits[0].source, Source::Toolchain);
        let strings = vec![s(
            "/cargo/registry/25cdd57fae9f0462/gimli-0.34.0/src/read/line.rs",
        )];
        let hits = detect(&strings);
        assert_eq!(hits[0].name, "gimli");
        assert_eq!(hits[0].version.as_deref(), Some("0.34.0"));
        assert_eq!(hits[0].source, Source::Toolchain);
    }

    #[test]
    fn git_checkout_not_double_counted_as_workspace() {
        let strings = vec![s(
            "/cargo/git/checkouts/zed-abc123/f00d/crates/gpui/src/app.rs",
        )];
        let hits = detect(&strings);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source, Source::Git);
    }

    #[test]
    fn workspace_crate_without_version() {
        let strings = vec![s("/build/zed/crates/gpui/src/platform/mod.rs")];
        let hits = detect(&strings);
        assert_eq!(hits[0].name, "gpui");
        assert!(hits[0].version.is_none());
        assert_eq!(hits[0].source, Source::Workspace);
    }
}
