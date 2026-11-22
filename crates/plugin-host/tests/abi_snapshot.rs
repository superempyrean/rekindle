//! The ABI snapshot (design doc §3.4): a checked-in record of the contract
//! and the host's own vocabulary, so a change to either without a version
//! bump fails CI.
//!
//! The snapshot holds the contract version, a hash of the WIT with comments
//! and whitespace normalised away (so doc edits don't count), every
//! interface and function name, and the slot names, decoration kinds,
//! surfaces and capabilities plugins name as strings. To accept a change:
//! bump the package version if plugins built against the old one would
//! break, then run with `REKINDLE_UPDATE_ABI=1`.

use std::fmt::Write as _;

use rekindle_plugin_host::builder::guest::WIT_FILES;
use rekindle_plugin_host::capability::Capability;
use rekindle_plugin_host::manifest::rekindle_wit_version;
use rekindle_plugin_host::model::{ALL_SLOTS, DECORATION_KINDS, SURFACES};
use sha2::{Digest, Sha256};

const SNAPSHOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../plugin-api/abi-snapshot.txt"
);

/// WIT without comments and with whitespace collapsed.
fn normalise(wit: &str) -> String {
    let mut out = String::new();
    for line in wit.lines() {
        let code = line.split("//").next().unwrap_or_default();
        for tok in code.split_whitespace() {
            out.push_str(tok);
            out.push(' ');
        }
    }
    out
}

fn snapshot() -> String {
    let mut files: Vec<_> = WIT_FILES.to_vec();
    files.sort_unstable();
    let mut hash = Sha256::new();
    let mut names = Vec::new();
    for (name, text) in &files {
        let n = normalise(text);
        hash.update(name.as_bytes());
        hash.update(n.as_bytes());
        // `interface x {` / `name: func(` declarations.
        let toks: Vec<&str> = n.split(' ').collect();
        let mut iface = "";
        for (i, tok) in toks.iter().enumerate() {
            if i > 0 && toks[i - 1] == "interface" {
                iface = tok;
            }
            let next = toks.get(i + 1).copied().unwrap_or_default();
            if let Some(f) = tok.strip_suffix(':').filter(|_| next.starts_with("func(")) {
                names.push(format!("func {iface}.{f}"));
            }
        }
    }
    names.sort();
    names.dedup();
    let mut digest = String::new();
    for b in hash.finalize() {
        write!(digest, "{b:02x}").unwrap();
    }
    let mut s = String::new();
    writeln!(s, "version {}", rekindle_wit_version()).unwrap();
    writeln!(s, "wit-sha256 {digest}").unwrap();
    for n in names {
        writeln!(s, "{n}").unwrap();
    }
    for c in Capability::ALL {
        writeln!(s, "capability {c}").unwrap();
    }
    for (slot, kind) in ALL_SLOTS {
        writeln!(s, "slot {slot} {kind:?}").unwrap();
    }
    for (kind, key) in DECORATION_KINDS {
        writeln!(s, "decoration {kind} {key:?}").unwrap();
    }
    for surface in SURFACES {
        writeln!(s, "surface {surface}").unwrap();
    }
    s
}

fn version_of(s: &str) -> Option<&str> {
    s.lines().next()?.strip_prefix("version ")
}

#[test]
fn the_contract_matches_its_snapshot() {
    let now = snapshot();
    let recorded = std::fs::read_to_string(SNAPSHOT)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    if now == recorded {
        return;
    }
    if std::env::var_os("REKINDLE_UPDATE_ABI").is_some() {
        std::fs::write(SNAPSHOT, &now).unwrap();
        return;
    }
    let same_version = version_of(&now) == version_of(&recorded);
    let diff: Vec<String> = {
        let a: std::collections::BTreeSet<&str> = recorded.lines().collect();
        let b: std::collections::BTreeSet<&str> = now.lines().collect();
        a.difference(&b)
            .map(|l| format!("- {l}"))
            .chain(b.difference(&a).map(|l| format!("+ {l}")))
            .collect()
    };
    panic!(
        "the plugin contract changed{}.\n{}\n\nIf plugins built against the old contract would break, bump the version in \
         crates/plugin-api/wit/types.wit (package line), plugin-api's WIT_VERSION and manifest::rekindle_wit_version. \
         Then record the snapshot with REKINDLE_UPDATE_ABI=1 cargo test -p rekindle-plugin-host --test abi_snapshot",
        if same_version {
            " without a version bump"
        } else {
            ""
        },
        diff.join("\n")
    );
}

#[test]
fn versions_agree() {
    let types = WIT_FILES.iter().find(|(n, _)| *n == "types.wit").unwrap().1;
    assert!(types.contains(&format!(
        "package rekindle:plugin@{};",
        rekindle_wit_version()
    )));
}
