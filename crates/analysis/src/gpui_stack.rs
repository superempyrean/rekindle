//! Classify where a binary's GPUI comes from.
//!
//! "GPUI" can reach an application several ways, and each needs a different
//! next step for layout recovery and code matching:
//!
//! - a git checkout of Zed (`git/checkouts/zed-<hash>/<rev>/crates/gpui/…`):
//!   the short rev is right there;
//! - a workspace path (`…/crates/gpui/src/…`): vendored or a fork built in-tree,
//!   no version, so it needs the fork-point search;
//! - crates.io `gpui` (`gpui-0.2.x/`);
//! - crates.io `gpui-pre` (`gpui-pre-0.3.11/`): versioned snapshots of Zed,
//!   each naming its Zed commit (`rekindle-sigmatch snapshots` maps them), as
//!   used by Longbridge's gpui-kit;
//!
//! plus toolkit layers on top (gpui-kit, gpui-component, gpui-base), which
//! matter because a product built on gpui-component keeps its colors in
//! `gpui_component::ThemeColor`, not Zed's `theme::ThemeColors`.

use regex::Regex;
use serde::Serialize;

use crate::crates::{CrateHit, Source};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Core {
    /// Git dependency; `checkout` is cargo's checkout dir (e.g. `zed-a1b2c3`),
    /// `rev` the short revision cargo checked out.
    Git {
        checkout: Option<String>,
        rev: Option<String>,
    },
    /// Built from a workspace path: no version information.
    Workspace,
    /// A crates.io package (`gpui` or `gpui-pre`).
    Registry { package: String, version: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Layer {
    pub name: String,
    pub version: Option<String>,
    pub source: Source,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GpuiStack {
    /// Every distinct provenance seen for the GPUI core (normally one).
    pub core: Vec<Core>,
    /// Toolkit crates layered on top (gpui-kit family, gpui-component, …).
    pub layers: Vec<Layer>,
    /// Human-readable next steps.
    pub hints: Vec<String>,
}

impl GpuiStack {
    pub fn is_empty(&self) -> bool {
        self.core.is_empty() && self.layers.is_empty()
    }
}

fn is_core(name: &str) -> bool {
    matches!(name, "gpui" | "gpui-pre")
}

fn is_layer(name: &str) -> bool {
    name == "gpui-kit"
        || name == "gpui-component"
        || name == "gpui-base"
        || name.starts_with("gpui-pre-")
        || name.starts_with("gpui-component-")
        || name.starts_with("gpui-kit-")
}

pub fn classify(hits: &[CrateHit]) -> GpuiStack {
    let checkout_re = Regex::new(r"git[/\\]checkouts[/\\]([^/\\]+)[/\\]([^/\\]+)[/\\]")
        .expect("the pattern is valid");
    let mut out = GpuiStack::default();

    for h in hits {
        if is_core(&h.name) {
            let core = match (h.source, &h.version) {
                (Source::Registry, Some(v)) => Core::Registry {
                    package: h.name.clone(),
                    version: v.clone(),
                },
                (Source::Git, _) => {
                    let caps = checkout_re.captures(&h.example);
                    Core::Git {
                        checkout: caps.as_ref().map(|c| c[1].to_string()),
                        rev: caps.as_ref().map(|c| c[2].to_string()),
                    }
                }
                (Source::Workspace, _) => Core::Workspace,
                _ => continue,
            };
            if !out.core.contains(&core) {
                out.core.push(core);
            }
        } else if is_layer(&h.name) && h.source != Source::Toolchain {
            let layer = Layer {
                name: h.name.clone(),
                version: h.version.clone(),
                source: h.source,
            };
            if !out.layers.contains(&layer) {
                out.layers.push(layer);
            }
        }
    }
    out.layers.sort_by(|a, b| a.name.cmp(&b.name));

    for c in &out.core {
        out.hints.push(match c {
            Core::Registry { package, version } if package == "gpui-pre" => format!(
                "gpui-pre {version} is a snapshot of a specific Zed commit: `rekindle-sigmatch snapshots` maps it; compile that commit for code matching"
            ),
            Core::Registry { package, version } => {
                format!("{package} {version} from crates.io: compile it directly with `rekindle-sigmatch compile {package} --version {version}`")
            }
            Core::Git { checkout, rev: Some(rev) } => format!(
                "git dependency ({}) at rev {rev}: if the checkout is Zed or a fork, compile that rev with `--git … --rev {rev}`",
                checkout.as_deref().unwrap_or("?")
            ),
            Core::Git { .. } => {
                "git dependency with no recoverable rev: find it with `rekindle-analyze <binary> --fork-point <zed clone>`".to_string()
            }
            Core::Workspace => {
                "GPUI built from a workspace path (vendored or forked in-tree): no version. Find the commit it forked from with `rekindle-analyze <binary> --fork-point <zed clone>`".to_string()
            }
        });
    }
    if out.layers.iter().any(|l| l.name == "gpui-component") {
        out.hints.push(
            "gpui-component present: the theme likely lives in `gpui_component::ThemeColor` (139 Hsla fields in declaration order at 0.9.0). Probe that, not Zed's theme::ThemeColors".to_string(),
        );
    }
    if out.core.len() > 1 {
        out.hints.push(
            "more than one GPUI provenance seen: the binary may link two copies; check occurrences"
                .to_string(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crates::detect;
    use crate::strings::Located;

    fn s(text: &str) -> Located {
        Located {
            offset: 0,
            text: text.to_string(),
        }
    }

    #[test]
    fn gpui_kit_on_gpui_pre() {
        let reg = r"C:\Users\x\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f";
        let strings = vec![
            s(&format!(r"{reg}\gpui-pre-0.3.11\src\app.rs")),
            s(&format!(r"{reg}\gpui-pre-macros-0.3.11\src\lib.rs")),
            s(&format!(r"{reg}\gpui-component-0.9.0\src\theme\mod.rs")),
            s(&format!(r"{reg}\gpui-kit-0.9.0\src\lib.rs")),
        ];
        let hits = detect(&strings);
        assert!(
            hits.iter()
                .any(|h| h.name == "gpui-pre" && h.version.as_deref() == Some("0.3.11"))
        );
        let st = classify(&hits);
        assert_eq!(
            st.core,
            vec![Core::Registry {
                package: "gpui-pre".into(),
                version: "0.3.11".into()
            }]
        );
        let names: Vec<_> = st.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, vec!["gpui-component", "gpui-kit", "gpui-pre-macros"]);
        assert!(
            st.hints
                .iter()
                .any(|h| h.contains("gpui_component::ThemeColor"))
        );
    }

    #[test]
    fn zed_git_checkout_rev() {
        let hits = detect(&[s(
            "/home/u/.cargo/git/checkouts/zed-a1b2c3d4/c84af5b/crates/gpui/src/app.rs",
        )]);
        let st = classify(&hits);
        assert_eq!(
            st.core,
            vec![Core::Git {
                checkout: Some("zed-a1b2c3d4".into()),
                rev: Some("c84af5b".into())
            }]
        );
    }

    #[test]
    fn in_tree_fork_and_nothing() {
        let st = classify(&detect(&[s("/build/phoenix/crates/gpui/src/window.rs")]));
        assert_eq!(st.core, vec![Core::Workspace]);
        assert!(st.hints[0].contains("fork-point"));
        assert!(classify(&detect(&[s("/x/registry/src/i/serde-1.0.0/src/lib.rs")])).is_empty());
    }
}
