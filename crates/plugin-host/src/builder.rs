//! `rekindle build`: turn a plugin project into a loadable, signable component.
//!
//! A project is a Rust crate (`Cargo.toml`) or a TypeScript package
//! (`package.json`, see [`js`]). For a Rust crate:
//!
//! 1. Read `[package.metadata.rekindle]` via `cargo metadata` and validate it.
//! 2. `cargo build --release --target wasm32-unknown-unknown`.
//! 3. Wrap the core module into a component with `wit-component` (the type
//!    information wit-bindgen embedded tells it which WIT world to use).
//!    Unused imports are dropped, so the component imports exactly the
//!    interfaces the plugin really calls.
//!
//! The script languages read the same metadata from their own project file and
//! build with their own toolchains instead. Then, for all of them:
//!
//! 4. Embed the manifest as the `rekindle:manifest` custom section.
//! 5. Inspect the result exactly as the host will at load, so mismatches (e.g.
//!    using `http` without requesting `network`) fail the build, not the user.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::capability::Capability;
use crate::manifest::{self, CargoMetadata, Manifest};
use crate::runtime::{self, Inspection};

pub mod guest;
pub mod js;
mod strip;

/// The target Rust plugins compile to.
pub const TARGET: &str = "wasm32-unknown-unknown";

#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// The project directory, or its project file (`Cargo.toml`,
    /// `package.json`).
    pub path: PathBuf,
    /// Rust: the release profile rather than the debug one.
    pub release: bool,
    /// Leave out cargo's progress output and the build's warnings and notes.
    pub quiet: bool,
    /// Type-check a TypeScript plugin first (`tsc --noEmit`), and refuse to
    /// build one with type errors.
    pub typecheck: bool,
}

#[derive(Debug, Clone)]
pub struct BuildOutput {
    /// Where the component was written.
    pub wasm: PathBuf,
    /// The component itself. Prefer this to re-reading `wasm`: concurrent
    /// builds of the same crate replace that file.
    pub bytes: Vec<u8>,
    /// The manifest embedded in the component.
    pub manifest: Manifest,
    /// The component as the host sees it at load.
    pub inspection: Inspection,
    /// A JS or TS plugin's source map (`<id>.js.map`), for its stack traces.
    pub source_map: Option<PathBuf>,
}

/// The language a plugin project is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    /// TypeScript (an entry in plain JavaScript builds too).
    TypeScript,
}

impl Language {
    const ALL: [Language; 2] = [Language::Rust, Language::TypeScript];

    /// The project file that marks a directory as this kind of project.
    fn project_file(self) -> &'static str {
        match self {
            Language::Rust => "Cargo.toml",
            Language::TypeScript => "package.json",
        }
    }
}

/// A plugin project, read and validated but not built.
#[derive(Debug, Clone)]
pub struct Project {
    pub language: Language,
    pub manifest: Manifest,
    /// The directory holding the project file.
    pub dir: PathBuf,
    /// Where builds go: the component lands in `<target_dir>/rekindle/`.
    pub target_dir: PathBuf,
    kind: Kind,
}

#[derive(Debug, Clone)]
enum Kind {
    Rust {
        lib_name: String,
        manifest_path: PathBuf,
    },
    Js(js::Config),
}

/// Read the project at `path`: a project directory, or its project file. In a
/// directory holding more than one, a Rust crate wins, then a JS package.
pub fn read_project(path: &Path) -> Result<Project> {
    let file = if path.is_dir() {
        Language::ALL
            .iter()
            .map(|l| path.join(l.project_file()))
            .find(|f| f.exists())
            .with_context(|| format!("{} has no Cargo.toml or package.json", path.display()))?
    } else {
        path.to_path_buf()
    };
    let name = file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let language = Language::ALL
        .into_iter()
        .find(|l| l.project_file() == name)
        .with_context(|| format!("{} isn't a Cargo.toml or package.json", file.display()))?;
    let dir = dunce(
        file.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    );
    let (manifest, target_dir, kind) = match language {
        Language::Rust => {
            let (m, target_dir, lib_name, manifest_path) = read_manifest(&file)?;
            (
                m,
                target_dir,
                Kind::Rust {
                    lib_name,
                    manifest_path,
                },
            )
        }
        Language::TypeScript => {
            let (m, cfg) = js::read(&file)?;
            (m, dir.join("target"), Kind::Js(cfg))
        }
    };
    Ok(Project {
        language,
        manifest,
        dir,
        target_dir,
        kind,
    })
}

#[derive(Deserialize)]
struct Meta {
    packages: Vec<Pkg>,
    target_directory: PathBuf,
}

#[derive(Deserialize)]
struct Pkg {
    name: String,
    version: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    manifest_path: PathBuf,
    targets: Vec<Target>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Target {
    name: String,
    crate_types: Vec<String>,
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn manifest_path(path: &Path) -> PathBuf {
    if path.extension().is_some_and(|e| e == "toml") {
        path.to_path_buf()
    } else {
        path.join("Cargo.toml")
    }
}

/// Read and validate a crate's rekindle manifest without building it.
fn read_manifest(path: &Path) -> Result<(Manifest, PathBuf, String, PathBuf)> {
    let mp = manifest_path(path);
    let out = cargo()
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(&mp)
        .output()
        .context("running `cargo metadata`")?;
    if !out.status.success() {
        bail!(
            "cargo metadata failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let meta: Meta = serde_json::from_slice(&out.stdout)?;
    let canonical = dunce(&mp);
    let pkg = meta
        .packages
        .into_iter()
        .find(|p| dunce(&p.manifest_path) == canonical)
        .with_context(|| format!("no package found for {}", mp.display()))?;

    let raw = pkg
        .metadata
        .as_ref()
        .and_then(|m| m.get("rekindle"))
        .with_context(|| format!("{} has no [package.metadata.rekindle] table", mp.display()))?;
    let cm: CargoMetadata =
        serde_json::from_value(raw.clone()).context("invalid [package.metadata.rekindle]")?;
    let mut manifest = cm.into_manifest(
        &pkg.name,
        &pkg.version,
        pkg.description.as_deref().unwrap_or(""),
        &pkg.authors,
    )?;
    if let Some(dir) = mp.parent() {
        manifest.strings = manifest::load_strings(dir)?;
        manifest.validate()?;
    }

    let lib = pkg
        .targets
        .iter()
        .find(|t| t.crate_types.iter().any(|c| c == "cdylib"))
        .with_context(|| format!("{} needs `[lib] crate-type = [\"cdylib\"]`", pkg.name))?;
    Ok((
        manifest,
        meta.target_directory,
        lib.name.replace('-', "_"),
        pkg.manifest_path,
    ))
}

fn dunce(p: &Path) -> PathBuf {
    let c = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    // Strip Windows' verbatim prefix so paths from cargo and from us compare.
    let s = c.to_string_lossy();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
}

/// The error for capabilities the plugin uses but the manifest doesn't
/// request, naming all of them so one edit fixes the build.
fn missing_capabilities(missing: &[Capability]) -> anyhow::Error {
    let iface = |c: &Capability| c.interface().unwrap_or("?");
    if let [c] = missing {
        anyhow::anyhow!(
            "the plugin uses the `{}` interface but its manifest doesn't request `{c}`; add it to `capabilities`",
            iface(c)
        )
    } else {
        let each: Vec<String> = missing
            .iter()
            .map(|c| format!("`{}` needs `{c}`", iface(c)))
            .collect();
        anyhow::anyhow!(
            "the plugin uses interfaces whose capabilities its manifest doesn't request: {}; add them to `capabilities`",
            each.join(", ")
        )
    }
}

/// Build the project at `opts.path` into a component with its manifest
/// embedded, and write it to `<target>/rekindle/<id>.wasm`. Fails if the
/// plugin uses an interface its manifest doesn't request.
pub fn build(opts: &BuildOptions) -> Result<BuildOutput> {
    let p = read_project(&opts.path)?;
    let component = match &p.kind {
        Kind::Rust {
            lib_name,
            manifest_path,
        } => {
            let module = cargo_build(opts, &p.target_dir, lib_name, manifest_path)?;
            componentize(&strip::unused_hooks(&module, &p.manifest)?)?
        }
        Kind::Js(cfg) => js::build(
            &p.dir,
            &p.target_dir,
            &p.manifest,
            cfg,
            opts.quiet,
            opts.typecheck,
        )?,
    };
    let is_js = matches!(p.kind, Kind::Js(_));
    let Project {
        manifest,
        target_dir,
        ..
    } = p;
    let component = manifest::embed(&component, &manifest)?;

    let engine = runtime::engine()?;
    let inspection = runtime::inspect_artifact(&engine, &component)?;
    let missing: Vec<Capability> = inspection
        .imported
        .iter()
        .copied()
        .filter(|c| !manifest.requests(*c))
        .collect();
    if !missing.is_empty() {
        return Err(missing_capabilities(&missing));
    }

    if !opts.quiet {
        let unused: Vec<String> = manifest
            .capabilities
            .iter()
            .filter(|c| c.interface().is_some() && !inspection.imported.contains(c))
            .map(|c| format!("`{c}`"))
            .collect();
        if !unused.is_empty() {
            eprintln!(
                "warning: the manifest requests {} but the plugin never calls {}; users are asked to approve every capability, so drop what you don't use",
                unused.join(", "),
                if unused.len() == 1 { "it" } else { "them" }
            );
        }
    }

    let out_dir = target_dir.join("rekindle");
    std::fs::create_dir_all(&out_dir)?;
    let wasm = out_dir.join(format!("{}.wasm", manifest.id));
    let changed = std::fs::read(&wasm).ok().as_deref() != Some(&component[..]);
    write_atomic(&wasm, &component)?;
    // A signature is over the exact bytes: one left from an earlier build
    // would only fail verification later, far from the cause.
    let sig = wasm.with_extension("wasm.sig");
    if changed && sig.exists() {
        std::fs::remove_file(&sig).with_context(|| format!("removing stale {}", sig.display()))?;
        if !opts.quiet {
            eprintln!(
                "note: removed {}, which signed the previous build; run `rekindle sign` again",
                sig.display()
            );
        }
    }

    let source_map =
        Some(js::source_map_path(&target_dir, &manifest.id)).filter(|m| is_js && m.exists());
    Ok(BuildOutput {
        wasm,
        bytes: component,
        manifest,
        inspection,
        source_map,
    })
}

/// Write the SDK, test client and type declarations an editor needs, without
/// building (`rekindle build` writes them too). Nothing for a Rust project:
/// cargo handles its SDK. Returns what was written, for the user.
pub fn write_editor_files(path: &Path) -> Result<String> {
    let p = read_project(path)?;
    let sdk = p.target_dir.join("rekindle");
    Ok(match &p.kind {
        Kind::Rust { .. } => "nothing to write: cargo fetches the Rust SDK".into(),
        Kind::Js(_) => {
            if js::write_editor_files(&p.dir, &p.target_dir, &p.manifest)? {
                format!(
                    "wrote the SDK, test client and type declarations to {}",
                    sdk.display()
                )
            } else {
                format!(
                    "wrote the SDK and test client to {}; run `npm install` for the contract's type declarations",
                    sdk.display()
                )
            }
        }
    })
}

/// The caller's rustflags, plus `--remap-path-prefix` for the directories
/// whose paths rustc writes into panic messages: the workspace (or the
/// crate, when it isn't in one), cargo's home (registry sources) and the
/// toolchain. A built plugin then names no directory on the author's
/// machine.
///
/// The flags are the same for every crate in a workspace. Cargo rebuilds
/// every dependency when the flags change, so per-crate flags would rebuild
/// the shared dependencies each time another plugin in the workspace built.
///
/// Setting the flags in the environment means cargo ignores `rustflags` in
/// `.cargo/config.toml`; set `RUSTFLAGS` instead.
fn rustflags(manifest_path: &Path) -> Vec<String> {
    let mut flags: Vec<String> = match (
        std::env::var("CARGO_ENCODED_RUSTFLAGS"),
        std::env::var("RUSTFLAGS"),
    ) {
        (Ok(f), _) if !f.is_empty() => f.split('\x1f').map(String::from).collect(),
        (_, Ok(f)) => f.split_whitespace().map(String::from).collect(),
        _ => Vec::new(),
    };
    let crate_dir = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let workspace = crate_dir.ancestors().skip(1).find(|d| {
        std::fs::read_to_string(d.join("Cargo.toml"))
            .ok()
            .and_then(|t| t.parse::<toml::Table>().ok())
            .is_some_and(|t| t.contains_key("workspace"))
    });
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(|h| PathBuf::from(h).join(".cargo"))
        });
    let sysroot = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .args(["--print", "sysroot"])
        .current_dir(if crate_dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            &crate_dir
        })
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
    let root = match workspace {
        Some(w) => (Some(w.to_path_buf()), "/workspace"),
        None => (Some(crate_dir), "/plugin"),
    };
    let remaps = [(sysroot, "/rustc-sysroot"), (cargo_home, "/cargo"), root];
    for (from, to) in remaps {
        let Some(from) = from.filter(|p| !p.as_os_str().is_empty()) else {
            continue;
        };
        let from = std::path::absolute(&from).unwrap_or(from);
        flags.push(format!("--remap-path-prefix={}={to}", from.display()));
    }
    flags
}

/// Whether rustup reports the wasm target installed (true if rustup can't
/// tell us: then the hint would be a guess).
fn target_installed() -> bool {
    match Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .any(|l| l.trim() == TARGET),
        _ => true,
    }
}

/// Whether the crate at `manifest_path`, or the workspace it's in, sets
/// `[profile.release] opt-level`.
fn sets_release_opt_level(manifest_path: &Path) -> bool {
    let sets = |p: &Path| -> (bool, bool) {
        let Ok(text) = std::fs::read_to_string(p) else {
            return (false, false);
        };
        let Ok(t) = text.parse::<toml::Table>() else {
            return (false, false);
        };
        let opt = t
            .get("profile")
            .and_then(|p| p.get("release"))
            .and_then(|r| r.get("opt-level"))
            .is_some();
        (opt, t.contains_key("workspace"))
    };
    let (opt, is_root) = sets(manifest_path);
    if opt || is_root {
        return opt;
    }
    // The nearest ancestor that is a workspace root decides for its members.
    manifest_path
        .parent()
        .into_iter()
        .flat_map(|d| d.ancestors().skip(1))
        .map(|d| d.join("Cargo.toml"))
        .find_map(|p| match sets(&p) {
            (opt, true) => Some(opt),
            _ => None,
        })
        .unwrap_or(false)
}

/// `cargo build` the crate for wasm; returns its core module.
fn cargo_build(
    opts: &BuildOptions,
    target_dir: &Path,
    lib_name: &str,
    mp: &Path,
) -> Result<Vec<u8>> {
    let mut cmd = cargo();
    cmd.args(["build", "--lib", "--target", TARGET, "--manifest-path"])
        .arg(mp);
    if opts.release {
        cmd.arg("--release");
        // Optimized for size, unless the project chooses: at -O3 the
        // generated export glue compiles 3-4x slower in the host (wasmtime's
        // register allocator), and events are handled as fast either way.
        if !sets_release_opt_level(mp) {
            cmd.env("CARGO_PROFILE_RELEASE_OPT_LEVEL", "s");
        }
    }
    if opts.quiet {
        cmd.arg("--quiet");
    }
    cmd.env_remove("RUSTFLAGS")
        .env("CARGO_ENCODED_RUSTFLAGS", rustflags(mp).join("\x1f"));
    let status = cmd.status().context("running `cargo build`")?;
    if !status.success() {
        if !target_installed() {
            bail!(
                "cargo build failed: the {TARGET} target isn't installed; run `rustup target add {TARGET}`"
            );
        }
        bail!("cargo build failed (see its errors above)");
    }
    let profile = if opts.release { "release" } else { "debug" };
    let module_path = target_dir
        .join(TARGET)
        .join(profile)
        .join(format!("{lib_name}.wasm"));
    std::fs::read(&module_path).with_context(|| format!("reading {}", module_path.display()))
}

/// Write via a uniquely named temp file + rename, so readers never see a
/// half-written component even when two builds of one crate race.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "wasm.{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, bytes)?;
    // On Windows the rename can transiently fail while another process has
    // the destination open; retry briefly.
    let mut attempts = 0;
    loop {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(_) if attempts < 20 => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) => {
                // The rename's error is the one to report.
                let _ = std::fs::remove_file(&tmp);
                return Err(e).with_context(|| format!("writing {}", path.display()));
            }
        }
    }
}

/// Wrap a wit-bindgen core module into a component.
fn componentize(module: &[u8]) -> Result<Vec<u8>> {
    wit_component::ComponentEncoder::default()
        .module(module)
        .context("reading embedded component type (does the crate use rekindle-sdk?)")?
        .validate(true)
        .encode()
        .context("encoding component")
}
