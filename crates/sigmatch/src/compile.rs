//! Compile a named crate version into an `.rlib`, ready for [`crate::extract`].
//!
//! `rekindle-analyze` tells us the exact `rustc` and the versions of the crates
//! whose paths survived. To look for the crates whose paths *didn't* survive,
//! we build a candidate ourselves and match its code. We generate a throwaway
//! package that depends on the one crate, build it on the pinned toolchain, and
//! hand back the dependency's `.rlib`.
//!
//! Byte-for-byte codegen is only reproduced when the reference build matches
//! the target's, so the knobs that shape codegen (`opt-level`, `codegen-units`,
//! `RUSTFLAGS`) are exposed and should be set from the target's own
//! `[profile.release]`. For Zed v1.27.0 that is `codegen-units = 1` and
//! `-C symbol-mangling-version=v0 --cfg tokio_unstable` (plus, for the Windows
//! target, `--cfg windows_slim_errors -C target-feature=+crt-static`).
//!
//! We deliberately build **without LTO**, even when the target used it. Under
//! `lto = "thin"`/`"fat"` a dependency's rlib holds LLVM bitcode, not machine
//! code (codegen is deferred to the final link), so there would be nothing to
//! signature. A non-LTO build emits real object code whose non-inlined
//! functions still match the target's copies.
//!
//! Two things cap recall, and both are inherent, not bugs. **LTO on the target**
//! inlines functions across its whole graph at link time; a function inlined
//! away there has no standalone copy to find. And **generics** are monomorphized
//! for the type arguments each *caller* uses, which live in other crates, so a
//! standalone build only emits the instantiations the crate itself uses.
//! Concrete, non-inlined functions match well, and a match is positive
//! evidence; a miss proves nothing. Measured on Zed v1.27.0's `remote_server`
//! (Windows): `crc32fast`, which leaves no path strings at all, was recovered
//! via 3 of its 12 functions, the `pclmulqdq` SIMD routine among them.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Where the crate's source comes from. A forked dependency (which Phoenix's
/// `gpui` is expected to be) has no crates.io version, so git and local-path
/// sources are first-class, not an afterthought.
pub enum CrateSource {
    /// crates.io: pin an exact version, or `None` for the newest cargo picks.
    Version(Option<String>),
    /// A git dependency, optionally pinned to a revision/tag/branch sha.
    Git { url: String, rev: Option<String> },
    /// A crate whose source is already on disk (e.g. a checked-out fork).
    Path(PathBuf),
}

impl CrateSource {
    /// The dependency spec's body, e.g. `version = "=1.0", ...`. The shared
    /// `default-features`/`features` tail is appended by [`manifest`].
    fn spec_head(&self) -> String {
        match self {
            CrateSource::Version(Some(v)) => format!("version = \"={v}\""),
            CrateSource::Version(None) => "version = \"*\"".to_string(),
            CrateSource::Git { url, rev: Some(r) } => format!("git = {url:?}, rev = {r:?}"),
            CrateSource::Git { url, rev: None } => format!("git = {url:?}"),
            // Debug-formatting a path yields a TOML-valid escaped string.
            CrateSource::Path(p) => format!("path = {:?}", p.display().to_string()),
        }
    }

    /// Human-readable provenance recorded in the signature set.
    pub fn describe(&self, name: &str) -> String {
        match self {
            CrateSource::Version(Some(v)) => format!("{name} {v}"),
            CrateSource::Version(None) => name.to_string(),
            CrateSource::Git { url, rev: Some(r) } => format!("{name} git:{url}@{r}"),
            CrateSource::Git { url, rev: None } => format!("{name} git:{url}"),
            CrateSource::Path(p) => format!("{name} path:{}", p.display()),
        }
    }
}

/// What [`compile_rlib`] builds, and how.
pub struct CompileArgs<'a> {
    pub crate_name: &'a str,
    pub source: &'a CrateSource,
    pub toolchain: &'a str,
    pub features: &'a [String],
    pub no_default_features: bool,
    /// Whole `RUSTFLAGS` string, e.g. the target's own flags. `None` = unset.
    pub rustflags: Option<&'a str>,
    pub codegen_units: u32,
    pub opt_level: &'a str,
    /// Directory the throwaway package and its `target/` are created under.
    pub work_root: &'a Path,
}

const BUILD_PKG: &str = "rekindle_sigmatch_build";

/// The throwaway package's `Cargo.toml`. Profile settings in a non-workspace
/// package govern its dependencies too, so this is where codegen is pinned.
pub fn manifest(args: &CompileArgs) -> String {
    let feats = args
        .features
        .iter()
        .map(|f| format!("{f:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "[package]\nname = \"{BUILD_PKG}\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n\
         [lib]\ncrate-type = [\"rlib\"]\n\n\
         [dependencies]\n\
         {name} = {{ {head}, default-features = {ddf}, features = [{feats}] }}\n\n\
         [profile.release]\nopt-level = {opt}\nlto = false\ncodegen-units = {cgu}\n",
        name = args.crate_name,
        head = args.source.spec_head(),
        ddf = !args.no_default_features,
        opt = args.opt_level,
        cgu = args.codegen_units,
    )
}

/// cargo turns `-` into `_` in the rlib filename; the hash suffix varies.
fn rlib_prefix(crate_name: &str) -> String {
    format!("lib{}-", crate_name.replace('-', "_"))
}

/// Build the package and return the dependency's `.rlib`.
pub fn compile_rlib(args: &CompileArgs) -> Result<PathBuf> {
    let pkg_dir = args.work_root.join(BUILD_PKG);
    let src_dir = pkg_dir.join("src");
    std::fs::create_dir_all(&src_dir).with_context(|| format!("creating {}", src_dir.display()))?;
    std::fs::write(pkg_dir.join("Cargo.toml"), manifest(args))?;
    // An empty lib is enough: cargo builds every declared dependency anyway.
    std::fs::write(src_dir.join("lib.rs"), "// rekindle-sigmatch build shim\n")?;

    let tc = format!("+{}", args.toolchain);
    let mut cmd = Command::new("cargo");
    cmd.args([tc.as_str(), "build", "--release"]);
    if args.no_default_features {
        cmd.arg("--no-default-features");
    }
    cmd.current_dir(&pkg_dir);
    if let Some(flags) = args.rustflags {
        cmd.env("RUSTFLAGS", flags);
    }

    tracing::info!(source = %args.source.describe(args.crate_name), "building candidate");
    let out = cmd.output().context("running cargo build")?;
    if !out.status.success() {
        bail!(
            "cargo build failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let deps = pkg_dir.join("target").join("release").join("deps");
    let prefix = rlib_prefix(args.crate_name);
    let mut found: Option<PathBuf> = None;
    for entry in std::fs::read_dir(&deps).with_context(|| format!("reading {}", deps.display()))? {
        let path = entry?.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with(&prefix)
            && path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("rlib"))
        {
            // Prefer the most recently written, in case stale artifacts linger.
            let newer = match &found {
                None => true,
                Some(cur) => mtime(&path) >= mtime(cur),
            };
            if newer {
                found = Some(path);
            }
        }
    }
    found.with_context(|| format!("no `{prefix}*.rlib` under {}", deps.display()))
}

fn mtime(p: &Path) -> std::time::SystemTime {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args<'a>(name: &'a str, src: &'a CrateSource, feats: &'a [String]) -> CompileArgs<'a> {
        CompileArgs {
            crate_name: name,
            source: src,
            toolchain: "1.98.1",
            features: feats,
            no_default_features: false,
            rustflags: None,
            codegen_units: 1,
            opt_level: "3",
            work_root: Path::new("."),
        }
    }

    #[test]
    fn manifest_pins_version_profile_and_features() {
        let feats = vec!["std".to_string(), "serde".to_string()];
        let src = CrateSource::Version(Some("1.13.2".into()));
        let m = manifest(&args("smallvec", &src, &feats));
        assert!(m.contains("smallvec = { version = \"=1.13.2\", default-features = true, features = [\"std\", \"serde\"] }"));
        assert!(
            m.contains("lto = false"),
            "LTO rlibs hold bitcode, not code"
        );
        assert!(m.contains("codegen-units = 1"));
        assert!(m.contains("opt-level = 3"));
    }

    #[test]
    fn manifest_unpinned_and_no_default_features() {
        let src = CrateSource::Version(None);
        let mut a = args("memchr", &src, &[]);
        a.no_default_features = true;
        let m = manifest(&a);
        assert!(m.contains("version = \"*\""));
        assert!(m.contains("default-features = false"));
        assert!(m.contains("features = []"));
    }

    #[test]
    fn manifest_git_and_path_sources() {
        let git = CrateSource::Git {
            url: "https://github.com/x/gpui".into(),
            rev: Some("abc123".into()),
        };
        let m = manifest(&args("gpui", &git, &[]));
        assert!(m.contains("gpui = { git = \"https://github.com/x/gpui\", rev = \"abc123\""));

        let path = CrateSource::Path(PathBuf::from("vendor/gpui"));
        let m = manifest(&args("gpui", &path, &[]));
        assert!(m.contains("path = \"vendor/gpui\""), "got: {m}");
    }

    #[test]
    fn source_describe() {
        assert_eq!(
            CrateSource::Version(Some("1.0".into())).describe("x"),
            "x 1.0"
        );
        let g = CrateSource::Git {
            url: "u".into(),
            rev: Some("r".into()),
        };
        assert_eq!(g.describe("gpui"), "gpui git:u@r");
    }

    #[test]
    fn rlib_prefix_normalizes_dashes() {
        assert_eq!(rlib_prefix("tracing-core"), "libtracing_core-");
        assert_eq!(rlib_prefix("gpui"), "libgpui-");
    }
}
