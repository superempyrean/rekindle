//! `gpui-pre` snapshot → Zed commit map: seeds for the fork-point search.
//!
//! Longbridge's gpui-kit depends on `gpui-pre`, a crates.io republication of
//! Zed's GPUI. Each version's description names the Zed commit it snapshots
//! ("… (gpui-pre snapshot of zed@f816e1f)"). If a target turns out to be built
//! on gpui-pre (or on a fork of it), these commits are the natural first
//! candidates to compile and score, ahead of walking Zed's whole history.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// One published version of a snapshot crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub krate: String,
    pub version: String,
    /// Publication date (YYYY-MM-DD).
    pub published: String,
    /// Zed commit (as abbreviated in the description), if stated.
    pub zed_commit: Option<String>,
    pub yanked: bool,
}

#[derive(Deserialize)]
struct VersionsPage {
    versions: Vec<VersionRow>,
}

#[derive(Deserialize)]
struct VersionRow {
    num: String,
    created_at: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    yanked: bool,
}

/// Parse a crates.io `/api/v1/crates/<name>/versions` response.
pub fn parse(krate: &str, json: &[u8]) -> Result<Vec<Snapshot>> {
    let page: VersionsPage =
        serde_json::from_slice(json).context("unexpected crates.io response")?;
    Ok(page
        .versions
        .into_iter()
        .map(|v| Snapshot {
            krate: krate.to_string(),
            zed_commit: v.description.as_deref().and_then(zed_commit),
            published: v.created_at.chars().take(10).collect(),
            version: v.num,
            yanked: v.yanked,
        })
        .collect())
}

/// Extract `<hex>` from "... zed@<hex> ...".
fn zed_commit(desc: &str) -> Option<String> {
    let at = desc.find("zed@")? + 4;
    let hex: String = desc[at..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    (hex.len() >= 7).then_some(hex)
}

/// Fetch every published version of `krate` from crates.io.
pub fn fetch(krate: &str) -> Result<Vec<Snapshot>> {
    let url = format!("https://crates.io/api/v1/crates/{krate}/versions?per_page=100");
    // crates.io requires an identifying User-Agent.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent("rekindle-sigmatch")
        .build()
        .into();
    let body = agent
        .get(&url)
        .call()
        .with_context(|| format!("GET {url}"))?
        .body_mut()
        .read_to_vec()
        .context("reading crates.io response")?;
    parse(krate, &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions_and_commits() {
        let json = br#"{"versions":[
            {"num":"0.3.11","created_at":"2026-11-02T10:00:00Z","description":"Zed's GPU-accelerated UI framework (gpui-pre snapshot of zed@f816e1f)","yanked":false},
            {"num":"0.1.0","created_at":"2026-01-01T00:00:00Z","description":"no commit here","yanked":true}
        ],"meta":{}}"#;
        let s = parse("gpui-pre", json).unwrap();
        assert_eq!(s[0].zed_commit.as_deref(), Some("f816e1f"));
        assert_eq!(s[0].published, "2026-11-02");
        assert_eq!(s[1].zed_commit, None);
        assert!(s[1].yanked);
    }

    #[test]
    fn commit_needs_seven_hex() {
        assert_eq!(zed_commit("zed@abc"), None);
        assert_eq!(
            zed_commit("snapshot of zed@0123456789abcdef)"),
            Some("0123456789abcdef".into())
        );
    }
}
