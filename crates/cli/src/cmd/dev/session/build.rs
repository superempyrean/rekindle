//! Rebuilding a plugin from its source when the source changes.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rekindle_plugin_host::builder::{self, BuildOptions, Language};
use rekindle_plugin_host::sourcemap::SourceMap;
use serde::Serialize;

use super::Session;
use crate::cmd::dev::term;
use crate::cmd::dev::watch::Watcher;

/// How long the source tree must be quiet before a rebuild starts.
const SETTLE: Duration = Duration::from_millis(250);
/// How often source trees are rescanned.
const POLL_EVERY: Duration = Duration::from_millis(400);

/// A successful build: the component and any compiler warnings.
pub(in crate::cmd::dev) struct Built {
    pub(in crate::cmd::dev) bytes: Vec<u8>,
    pub(in crate::cmd::dev) warnings: Option<String>,
    /// A JS plugin's source map, for its stack traces.
    pub(in crate::cmd::dev) source_map: Option<Arc<SourceMap>>,
}

/// Build a plugin project, capturing the compiler's output so a failure can
/// be shown in the browser too. For a Rust crate, runs cargo once with
/// captured output (that is the real compile), then `rekindle build`'s
/// pipeline, which finds the crate fresh and only componentizes and
/// validates. Other languages go straight to the pipeline, whose errors
/// already carry their toolchain's output.
pub(in crate::cmd::dev) fn compile(opts: &BuildOptions) -> Result<Built, String> {
    let project = builder::read_project(&opts.path).map_err(|e| format!("{e:#}"))?;
    let warnings = match project.language {
        Language::Rust => cargo_compile(&project.dir.join("Cargo.toml"), opts.release)?,
        Language::TypeScript => None,
    };
    let built = builder::build(&BuildOptions {
        quiet: true,
        ..opts.clone()
    })
    .map_err(|e| format!("{e:#}"))?;
    let source_map = built
        .source_map
        .as_deref()
        .and_then(|p| SourceMap::read(p).ok())
        .map(Arc::new);
    Ok(Built {
        bytes: built.bytes,
        warnings,
        source_map,
    })
}

/// How a plugin's latest build went, as the page shows it.
#[derive(Debug, Clone, Serialize, Hash)]
pub(super) struct BuildStatus {
    state: BuildState,
    /// Compiler/build output for a failure.
    error: Option<String>,
    /// Increments on every finished build.
    generation: u64,
    duration_ms: u64,
    /// Source files that triggered this build.
    changed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
enum BuildState {
    Building,
    Ok,
    Failed,
}

/// Where a plugin rebuilds from, and how its builds are going.
pub(super) struct Source {
    opts: BuildOptions,
    watcher: Watcher,
    pub(super) status: BuildStatus,
    building: bool,
    /// A change (or forced reload) arrived mid-build; build again after.
    queued: Option<Vec<String>>,
}

impl Source {
    /// Watch the project `opts` builds. `first_error` is the first build's
    /// failure, if it failed.
    pub(super) fn new(opts: BuildOptions, first_error: Option<&String>) -> Self {
        let (state, error) = match first_error {
            None => (BuildState::Ok, None),
            Some(e) => (BuildState::Failed, Some(e.clone())),
        };
        Self {
            watcher: Watcher::new(&crate_dir(&opts.path), SETTLE),
            opts,
            status: BuildStatus {
                state,
                error,
                generation: 1,
                duration_ms: 0,
                changed: Vec::new(),
            },
            building: false,
            queued: None,
        }
    }
}

/// A build finished on its worker thread.
pub(super) struct BuildDone {
    idx: usize,
    elapsed: Duration,
    result: Result<Built, String>,
}

impl Session {
    /// Rebuild each plugin whose source changed and has been quiet since.
    pub(super) fn poll_sources(&mut self, now: Instant) {
        if now.duration_since(self.last_poll) < POLL_EVERY {
            return;
        }
        self.last_poll = now;
        for idx in 0..self.entries.len() {
            let changed = self.entries[idx]
                .source
                .as_mut()
                .and_then(|s| s.watcher.poll(now));
            if let Some(changed) = changed {
                let names: Vec<String> = changed
                    .iter()
                    .map(|p| p.display().to_string().replace('\\', "/"))
                    .collect();
                term::note(&format!("changed: {}", names.join(", ")));
                self.start_build(idx, names);
            }
        }
    }

    /// Build entry `idx` on a worker thread, or queue the build if one is
    /// running.
    pub(super) fn start_build(&mut self, idx: usize, changed: Vec<String>) {
        let Some(src) = self.entries[idx].source.as_mut() else {
            return;
        };
        if src.building {
            src.queued.get_or_insert_with(Vec::new).extend(changed);
            return;
        }
        src.building = true;
        src.status.state = BuildState::Building;
        src.status.changed = changed;
        let opts = src.opts.clone();
        let tx = self.build_tx.clone();
        term::note(&format!("building {}…", self.entries[idx].id));
        std::thread::spawn(move || {
            let t = Instant::now();
            let result = compile(&opts);
            // The receiver goes with the session, and then nobody is waiting.
            let _ = tx.send(BuildDone {
                idx,
                elapsed: t.elapsed(),
                result,
            });
        });
    }

    /// Load a finished build, or record its failure, and start the queued
    /// build if there is one.
    pub(super) fn finish_build(&mut self, done: BuildDone) {
        let id = self.entries[done.idx].id.clone();
        let Some(src) = self.entries[done.idx].source.as_mut() else {
            return;
        };
        src.building = false;
        src.status.generation += 1;
        src.status.duration_ms = done.elapsed.as_millis() as u64;
        let secs = done.elapsed.as_secs_f64();
        match done.result {
            Ok(built) => {
                src.status.state = BuildState::Ok;
                src.status.error = None;
                if let Some(w) = &built.warnings {
                    term::warn("build warnings:");
                    term::block(w);
                }
                term::ok(&format!("built {id} in {secs:.1}s"));
                if built.source_map.is_some() {
                    self.set_source_map(&id, built.source_map);
                }
                // `install` records and prints a load failure itself.
                let _ = self.install(done.idx, built.bytes);
            }
            Err(e) => {
                term::error(&format!(
                    "build of {id} failed after {secs:.1}s; the previous build keeps running"
                ));
                term::block(&e);
                src.status.state = BuildState::Failed;
                src.status.error = Some(e);
            }
        }
        let queued = self.entries[done.idx]
            .source
            .as_mut()
            .and_then(|s| s.queued.take());
        if let Some(changed) = queued {
            self.start_build(done.idx, changed);
        }
    }
}

/// The project directory for a path to it or to its project file
/// (`Cargo.toml`, `package.json`).
fn crate_dir(path: &Path) -> PathBuf {
    let file = ["Cargo.toml", "package.json"]
        .iter()
        .any(|f| path.file_name().is_some_and(|n| n == *f));
    if file {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        path.to_path_buf()
    }
}

/// `cargo build` the crate with captured output; its warnings, if any.
fn cargo_compile(manifest_path: &Path, release: bool) -> Result<Option<String>, String> {
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.args([
        "build",
        "--lib",
        "--color",
        "never",
        "--target",
        builder::TARGET,
        "--manifest-path",
    ])
    .arg(manifest_path);
    if release {
        cmd.arg("--release");
    }
    let out = cmd.output().map_err(|e| format!("running cargo: {e}"))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let diagnostics: String = stderr
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("Compiling ")
                || t.starts_with("Finished ")
                || t.starts_with("Blocking ")
                || t.starts_with("Locking "))
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !out.status.success() {
        let text = diagnostics.trim();
        return Err(if text.is_empty() {
            "cargo build failed".to_string()
        } else {
            text.to_string()
        });
    }
    Ok(diagnostics
        .contains("warning")
        .then(|| diagnostics.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_dir_accepts_dir_or_project_file() {
        assert_eq!(
            crate_dir(Path::new("plugins/x")),
            PathBuf::from("plugins/x")
        );
        for f in ["Cargo.toml", "package.json"] {
            assert_eq!(
                crate_dir(&Path::new("plugins/x").join(f)),
                PathBuf::from("plugins/x")
            );
        }
    }
}
