//! `rekindle new`: every language and template scaffolds a project that
//! builds, activates and typechecks as it stands.
//!
//! The TypeScript ones need the toolchain: `npm install` (jco) in `plugins/`.
//! Without it a test prints why and passes; set
//! `REKINDLE_REQUIRE_GUEST_TOOLCHAINS=1` to make that a failure instead (CI).

use std::path::{Path, PathBuf};
use std::process::Command;

use rekindle_plugin_host::builder::{self, Language};
use rekindle_plugin_host::model::LogLevel;
use rekindle_plugin_host::testing::Harness;

fn plugin_dir(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins")
        .join(rel)
}

/// The plugin's directory, or `None` (after saying why) if its toolchain
/// isn't installed.
fn toolchain(rel: &str) -> Option<PathBuf> {
    let dir = plugin_dir(rel);
    if dir
        .ancestors()
        .any(|d| d.join("node_modules/@bytecodealliance/jco").exists())
    {
        return Some(dir);
    }
    let how = "`npm install` in plugins/";
    assert!(
        std::env::var_os("REKINDLE_REQUIRE_GUEST_TOOLCHAINS").is_none(),
        "{rel}: toolchain not installed; run {how}"
    );
    eprintln!("skipping {rel}: toolchain not installed (run {how})");
    None
}

macro_rules! needs {
    ($rel:expr) => {
        match toolchain($rel) {
            Some(dir) => dir,
            None => return,
        }
    };
}

/// Type-check a TypeScript project (after a build wrote its SDK and
/// declarations); anything else passes.
fn typecheck(dir: &Path) {
    if !dir.join("tsconfig.json").exists() {
        return;
    }
    let tsc = plugin_dir("node_modules/typescript/bin/tsc");
    let out = Command::new("node")
        .arg(tsc)
        .args(["--noEmit", "-p"])
        .arg(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "tsc {}: {}{}",
        dir.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `rekindle new <name> --lang <lang>` in `parent`; returns the project dir.
fn new(parent: &Path, name: &str, lang: &str) -> PathBuf {
    let _ = std::fs::remove_dir_all(parent.join(name));
    std::fs::create_dir_all(parent).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rekindle"))
        .args(["new", name, "--lang", lang, "--dir"])
        .arg(parent)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    parent.join(name)
}

#[test]
fn every_language_scaffolds_a_valid_project() {
    let parent = std::env::temp_dir().join(format!("rekindle-scaffold-{}", std::process::id()));
    for (lang, language) in [("rust", Language::Rust), ("ts", Language::TypeScript)] {
        let dir = new(&parent, &format!("hello-{lang}"), lang);
        let p = builder::read_project(&dir).unwrap_or_else(|e| panic!("{lang}: {e:#}"));
        assert_eq!(p.language, language);
        assert_eq!(
            (p.manifest.id.as_str(), p.manifest.name.as_str()),
            (
                format!("dev.local.hello-{lang}").as_str(),
                title(lang).as_str()
            )
        );
        assert!(p.manifest.capabilities.is_empty());
    }
    // A bad id is refused up front, not at the first build.
    let refused = Command::new(env!("CARGO_BIN_EXE_rekindle"))
        .args(["new", "x", "--id", "Bad.Id", "--dir"])
        .arg(&parent)
        .output()
        .unwrap();
    assert!(
        !refused.status.success()
            && String::from_utf8_lossy(&refused.stderr).contains("invalid plugin id")
    );
    assert!(!parent.join("x").exists());
    let _ = std::fs::remove_dir_all(&parent);
}

/// What `rekindle new` titles `hello-<lang>`.
fn title(lang: &str) -> String {
    match lang {
        "ts" => "Hello TS".into(),
        other => format!("Hello {}{}", other[..1].to_uppercase(), &other[1..]),
    }
}

/// Scaffold under `plugins/fixtures/` (where the toolchains are), build, run.
fn scaffold_and_run(lang: &str, rel_parent: &str) {
    let parent = needs!(&format!("fixtures/{rel_parent}-{}", std::process::id()));
    let dir = new(&parent, &format!("hello-{lang}"), lang);
    let h = Harness::for_crate(&dir).unwrap_or_else(|e| panic!("{lang}: {e:#}"));
    h.assert_active();
    let want = format!("{} is up; granted: nothing", title(lang));
    assert!(h.logged(LogLevel::Info, &want), "{lang}: {}", h.log_text());
    drop(h);
    // The scaffold typechecks as it stands (the build wrote its declarations).
    typecheck(&dir);
    let _ = std::fs::remove_dir_all(&parent);
}

/// Every TypeScript template scaffolds, builds, activates and typechecks.
#[test]
fn every_script_template_builds_and_activates() {
    let Some(parent) = toolchain(&format!("fixtures/ts-templates-{}", std::process::id())) else {
        return;
    };
    for template in ["blank", "theme", "panel", "events", "network"] {
        let name = format!("t-{template}");
        let _ = std::fs::remove_dir_all(parent.join(&name));
        std::fs::create_dir_all(&parent).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_rekindle"))
            .args([
                "new",
                &name,
                "--lang",
                "ts",
                "--template",
                template,
                "--dir",
            ])
            .arg(&parent)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{template}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let dir = parent.join(&name);
        let h = Harness::for_crate(&dir).unwrap_or_else(|e| panic!("{template}: {e:#}"));
        h.assert_active();
        assert!(
            !h.logged(LogLevel::Warn, ""),
            "{template} warned: {}",
            h.log_text()
        );
        drop(h);
        typecheck(&dir);
    }
    let _ = std::fs::remove_dir_all(&parent);
}

/// The events template reacts to what its capabilities deliver.
#[test]
fn the_events_template_follows_the_selection() {
    let Some(parent) = toolchain(&format!("fixtures/ts-events-{}", std::process::id())) else {
        return;
    };
    std::fs::create_dir_all(&parent).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rekindle"))
        .args(["new", "ev", "--lang", "ts", "--template", "events", "--dir"])
        .arg(&parent)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut h = Harness::for_crate(parent.join("ev")).unwrap_or_else(|e| panic!("{e:#}"));
    h.select_channel(rekindle_plugin_host::backend::fixtures::OFF_TOPIC);
    let panel = format!("{:?}", h.panel("status").expect("status panel").elements);
    assert!(
        panel.contains("Channel switches this session: 1") && panel.contains("#off-topic"),
        "{panel}"
    );
    drop(h);
    let _ = std::fs::remove_dir_all(&parent);
}

/// Every Rust template builds as scaffolded, so a contract change can't
/// quietly break one (a new `Event` variant once did).
#[test]
fn every_rust_template_builds() {
    let parent = std::env::temp_dir().join(format!("rekindle-templates-{}", std::process::id()));
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/scaffold-templates");
    for template in ["blank", "theme", "panel", "events", "network"] {
        let name = format!("t-{template}");
        let _ = std::fs::remove_dir_all(parent.join(&name));
        std::fs::create_dir_all(&parent).unwrap();
        let rekindle = || {
            let mut c = Command::new(env!("CARGO_BIN_EXE_rekindle"));
            c.env("CARGO_TARGET_DIR", &target);
            c
        };
        let out = rekindle()
            .args(["new", &name, "--template", template, "--dir"])
            .arg(&parent)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{template}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let out = rekindle()
            .arg("build")
            .arg(parent.join(&name))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{template} doesn't build:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn typescript_builds_and_runs() {
    scaffold_and_run("ts", "ts-scaffold");
}
