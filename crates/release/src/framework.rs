//! rekindle's own files, as the release channel lists them for the
//! launcher: the host library and the recovered data it reads (anchors,
//! layout, palette). One flat directory per version.
//!
//! The channel's `framework` section names a version and every file in it
//! with its size and sha256. The channel is signed as a whole, so the
//! files need no signatures of their own: a file that doesn't match its
//! listing is refused, and the version is not installed.
//!
//! A new version's files come from an installed version when one holds
//! the same bytes (most releases change only the recovered data, not the
//! host), and otherwise from the channel, as `<name>.zst` when the listing
//! has a compressed copy.
//!
//! The channel's `launcher` section lists the installed program the same
//! way, as one file ([`LAUNCHER_FILE`]) at `launcher/<version>/`, so the
//! launcher can replace itself.
//!
//! Updating an install ([`crate::update`]) reads only these two sections
//! of the channel ([`Head`]), checked against the same project keys as the
//! plugin runtime.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::location::{Location, with_suffix};
use crate::signing::{self, PublicKey, SigKind, Signature};

/// The one file of the channel's `launcher` listing: the installed program.
pub const LAUNCHER_FILE: &str = "rekindle.exe";

/// Max size of one framework file.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
/// Max size of a fetched channel.
pub const MAX_CHANNEL_BYTES: u64 = 16 * 1024 * 1024;
/// Max size of a signature file.
pub const MAX_SIG_BYTES: u64 = 64 * 1024;

/// A `framework` or `launcher` section: one version's files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Framework {
    /// Semver.
    pub version: String,
    /// Where the files are, relative to the channel (`framework/<version>/`).
    pub base: String,
    pub files: Vec<FrameworkFile>,
}

/// One file of a [`Framework`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameworkFile {
    /// A plain file name: no directories.
    pub name: String,
    pub size: u64,
    pub sha256: String,
    /// The size of `<name>.zst`, a zstd copy hosted beside the file. The
    /// sha256 is still the uncompressed file's. Launchers that predate it
    /// ignore it and fetch the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zst: Option<u64>,
}

/// What [`Framework::fetch`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fetched {
    /// Files copied from an installed version.
    pub reused: usize,
    /// Files downloaded, and the bytes that took.
    pub downloaded: usize,
    pub bytes: u64,
}

/// The zstd level `compress_dir` uses: slow to write, as fast to read as any.
const ZSTD_LEVEL: i32 = 19;

/// File names are plain: letters, digits, `.`, `_`, `-`, not starting with
/// a dot. Nothing that could leave the version's directory.
pub fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

impl Framework {
    /// Check the listing itself: a semver version, and at least one file,
    /// each with a plain, unique name, a well-formed sha256 and a size
    /// within the limit.
    pub fn validate(&self) -> Result<()> {
        semver::Version::parse(&self.version)
            .with_context(|| format!("framework version `{}` is not semver", self.version))?;
        if self.files.is_empty() {
            bail!("framework {} lists no files", self.version);
        }
        let mut seen = BTreeSet::new();
        for f in &self.files {
            if !is_plain_name(&f.name) {
                bail!("framework file name `{}` is not a plain file name", f.name);
            }
            if !seen.insert(f.name.to_ascii_lowercase()) {
                bail!("framework lists {} twice", f.name);
            }
            if !signing::is_sha256_hex(&f.sha256) {
                bail!("framework file {} has a malformed sha256", f.name);
            }
            if f.size > MAX_FILE_BYTES || f.zst.is_some_and(|z| z > MAX_FILE_BYTES) {
                bail!("framework file {} is too large", f.name);
            }
        }
        Ok(())
    }

    /// Write `<name>.zst` beside every file in `dir` (flat), replacing any
    /// copy already there, for [`Framework::from_dir`] to list.
    pub fn compress_dir(dir: &Path) -> Result<()> {
        for e in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let path = e?.path();
            if !path.is_file() || path.extension().is_some_and(|x| x == "zst") {
                continue;
            }
            let zst = with_suffix(&path, ".zst");
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            let packed = zstd::bulk::compress(&bytes, ZSTD_LEVEL)
                .with_context(|| format!("compressing {}", path.display()))?;
            std::fs::write(&zst, packed).with_context(|| format!("writing {}", zst.display()))?;
        }
        Ok(())
    }

    /// Checks for the channel's `launcher` listing: [`Framework::validate`],
    /// and exactly one file, [`LAUNCHER_FILE`].
    pub fn validate_launcher(&self) -> Result<()> {
        self.validate()?;
        if self.files.len() != 1 || self.files[0].name != LAUNCHER_FILE {
            bail!("the launcher listing must be the one file {LAUNCHER_FILE}");
        }
        Ok(())
    }

    /// List every file in `dir` (flat) as version `version`, hosted at
    /// `framework/<version>/` next to the channel. A `<name>.zst` beside a
    /// file is listed as its compressed copy, not as a file of its own.
    pub fn from_dir(dir: &Path, version: &str) -> Result<Self> {
        Self::from_dir_at(dir, version, &format!("framework/{version}/"))
    }

    /// [`Framework::from_dir`], hosted at `base` next to the channel.
    pub fn from_dir_at(dir: &Path, version: &str, base: &str) -> Result<Self> {
        let mut files = Vec::new();
        for e in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let e = e?;
            if !e.file_type()?.is_file() {
                bail!(
                    "{} has a subdirectory ({}); framework directories are flat",
                    dir.display(),
                    e.path().display()
                );
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name
                .strip_suffix(".zst")
                .is_some_and(|base| dir.join(base).is_file())
            {
                continue;
            }
            let bytes = std::fs::read(e.path())
                .with_context(|| format!("reading {}", e.path().display()))?;
            let zst = std::fs::metadata(dir.join(format!("{name}.zst")))
                .ok()
                .map(|m| m.len());
            files.push(FrameworkFile {
                name,
                size: bytes.len() as u64,
                sha256: signing::sha256_hex(&bytes),
                zst,
            });
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        let f = Framework {
            version: version.into(),
            base: base.into(),
            files,
        };
        f.validate()?;
        Ok(f)
    }

    /// Whether `dir` holds exactly these files, byte for byte.
    pub fn check_dir(&self, dir: &Path) -> Result<()> {
        for f in &self.files {
            let path = dir.join(&f.name);
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            let actual = signing::sha256_hex(&bytes);
            if actual != f.sha256 {
                bail!(
                    "{} doesn't match framework {} (sha256 {actual}, listed {})",
                    path.display(),
                    self.version,
                    f.sha256
                );
            }
        }
        Ok(())
    }

    /// Put every file into `dest`, which must not exist yet: from the first
    /// of `reuse` (installed versions' directories) that holds it byte for
    /// byte, else downloaded from next to `channel`. The files are written
    /// to `<dest>.partial`, which is renamed once every file matched, so
    /// `dest` either holds the whole version or nothing.
    pub fn fetch(&self, channel: &Location, dest: &Path, reuse: &[PathBuf]) -> Result<Fetched> {
        self.validate()?;
        if dest.exists() {
            bail!("{} already exists", dest.display());
        }
        let partial = with_suffix(dest, ".partial");
        // Left over from an interrupted fetch, if it exists.
        let _ = std::fs::remove_dir_all(&partial);
        std::fs::create_dir_all(&partial)
            .with_context(|| format!("creating {}", partial.display()))?;
        let result = (|| {
            let base = channel.join(&self.base)?;
            let mut done = Fetched::default();
            for f in &self.files {
                let bytes = if let Some(bytes) = reuse.iter().find_map(|d| installed(d, f)) {
                    done.reused += 1;
                    bytes
                } else {
                    let (bytes, took) = download_file(&base, f)?;
                    done.downloaded += 1;
                    done.bytes += took;
                    bytes
                };
                std::fs::write(partial.join(&f.name), &bytes)
                    .with_context(|| format!("writing {}", f.name))?;
            }
            std::fs::rename(&partial, dest)
                .with_context(|| format!("moving {} into place", dest.display()))?;
            Ok(done)
        })();
        if result.is_err() {
            // Best effort: the next fetch removes it too.
            let _ = std::fs::remove_dir_all(&partial);
        }
        result
    }
}

/// One file from the directory `base`, checked against its listing, and the
/// bytes fetched for it. Its zstd copy when it has one, else (or if that
/// copy is missing or bad) the file.
fn download_file(base: &Location, f: &FrameworkFile) -> Result<(Vec<u8>, u64)> {
    // `Location::join` resolves a name against a path's parent directory,
    // and a path keeps no trailing slash, so a directory path is joined here.
    let at = |name: &str| -> Result<Location> {
        Ok(match base {
            Location::Path(p) => Location::Path(p.join(name)),
            Location::Url(_) => base.join(name)?,
        })
    };
    if let Some(size) = f.zst {
        let loc = at(&format!("{}.zst", f.name))?;
        let unpacked = loc
            .read(size)
            .ok()
            .filter(|z| z.len() as u64 == size)
            .and_then(|z| unpack(&z, f.size));
        if let Some(bytes) = unpacked.filter(|b| matches(b, f)) {
            return Ok((bytes, size));
        }
    }
    let loc = at(&f.name)?;
    let bytes = loc.read(f.size)?;
    if !matches(&bytes, f) {
        bail!(
            "{loc} doesn't match the channel (sha256 {}, listed {})",
            signing::sha256_hex(&bytes),
            f.sha256
        );
    }
    let took = bytes.len() as u64;
    Ok((bytes, took))
}

fn matches(bytes: &[u8], f: &FrameworkFile) -> bool {
    bytes.len() as u64 == f.size && signing::sha256_hex(bytes) == f.sha256
}

/// `f` from an installed version's directory, if it holds the same bytes.
fn installed(dir: &Path, f: &FrameworkFile) -> Option<Vec<u8>> {
    let path = dir.join(&f.name);
    if std::fs::metadata(&path).ok()?.len() != f.size {
        return None;
    }
    std::fs::read(&path).ok().filter(|b| matches(b, f))
}

/// Decompress a zstd copy, reading at most one byte past `size` so a copy
/// that unpacks larger than its listing stops there.
fn unpack(packed: &[u8], size: u64) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    zstd::stream::read::Decoder::new(packed)
        .ok()?
        .take(size + 1)
        .read_to_end(&mut out)
        .ok()?;
    Some(out)
}

/// The parts of `channel.json` that updating an install reads (the plugin
/// runtime reads the rest).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Head {
    pub format: u32,
    pub serial: u64,
    pub expires: u64,
    #[serde(default)]
    pub framework: Option<Framework>,
    /// The installed program, newest release.
    #[serde(default)]
    pub launcher: Option<Framework>,
}

/// Fetch `channel.json` and its `.sig` from `location`.
pub fn fetch_channel(location: &Location) -> Result<(Vec<u8>, Signature)> {
    let bytes = location.read(MAX_CHANNEL_BYTES)?;
    let sig_loc = location.with_suffix(".sig");
    let sig_bytes = sig_loc
        .read(MAX_SIG_BYTES)
        .with_context(|| format!("channel signature {sig_loc} is missing"))?;
    let sig = Signature::from_json(&sig_bytes).with_context(|| format!("parsing {sig_loc}"))?;
    Ok((bytes, sig))
}

/// Check a channel's signature against one of `keys` (the project keys)
/// and read its head.
pub fn verify_head(bytes: &[u8], sig: &Signature, keys: &[PublicKey]) -> Result<Head> {
    sig.check_bytes(SigKind::Channel, bytes)?;
    let key = keys
        .iter()
        .find(|k| k.key_id() == sig.key_id)
        .with_context(|| {
            format!(
                "channel is signed by key {}, which is not a project key",
                sig.key_id
            )
        })?;
    key.verify_channel(bytes, sig)?;
    let head: Head =
        serde_json::from_slice(bytes).context("channel.json is not a valid channel")?;
    if head.format != signing::FORMAT {
        bail!("unsupported channel format {}", head.format);
    }
    if let Some(f) = &head.framework {
        f.validate()?;
    }
    if let Some(l) = &head.launcher {
        l.validate_launcher()?;
    }
    Ok(head)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_in_their_directory() {
        for ok in ["rekindle_host.dll", "anchors.json", "layout-release.json"] {
            assert!(is_plain_name(ok), "{ok}");
        }
        for bad in [
            "", "../x.dll", "a/b.dll", "a\\b.dll", ".hidden", "C:x.dll", "x y.dll",
        ] {
            assert!(!is_plain_name(bad), "{bad}");
        }
    }

    #[test]
    fn download_is_all_or_nothing() {
        let dir = std::env::temp_dir().join(format!("rekindle-release-fw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.dll"), b"host").unwrap();
        std::fs::write(src.join("anchors.json"), b"{}").unwrap();
        let fw = Framework::from_dir(&src, "1.2.0").unwrap();
        assert_eq!(fw.files.len(), 2);

        // Hosted next to a channel: dist/channel.json, dist/framework/1.2.0/*.
        let hosted = dir.join("dist").join("framework").join("1.2.0");
        std::fs::create_dir_all(&hosted).unwrap();
        for f in ["a.dll", "anchors.json"] {
            std::fs::copy(src.join(f), hosted.join(f)).unwrap();
        }
        let channel = Location::Path(dir.join("dist").join("channel.json"));
        let dest = dir.join("versions").join("1.2.0");
        fw.fetch(&channel, &dest, &[]).unwrap();
        fw.check_dir(&dest).unwrap();

        // A tampered file: nothing is installed, no partial left behind.
        std::fs::write(hosted.join("a.dll"), b"evil").unwrap();
        let dest2 = dir.join("versions").join("1.2.1");
        assert!(fw.fetch(&channel, &dest2, &[]).is_err());
        assert!(!dest2.exists() && !with_suffix(&dest2, ".partial").exists());
        std::fs::write(dest.join("a.dll"), b"evil").unwrap();
        assert!(fw.check_dir(&dest).is_err());
    }

    /// `<dir>/dist/framework/<version>/` with `files`, compressed, and its listing.
    fn host(dir: &Path, version: &str, files: &[(&str, &[u8])]) -> (Framework, PathBuf) {
        let hosted = dir.join("dist").join("framework").join(version);
        std::fs::create_dir_all(&hosted).unwrap();
        for (name, bytes) in files {
            std::fs::write(hosted.join(name), bytes).unwrap();
        }
        Framework::compress_dir(&hosted).unwrap();
        (Framework::from_dir(&hosted, version).unwrap(), hosted)
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rekindle-release-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn compressed_copies_are_listed_and_fetched() {
        let dir = scratch("zst");
        let host_dll = b"host ".repeat(1000);
        let (fw, hosted) = host(
            &dir,
            "1.0.0",
            &[("a.dll", &host_dll), ("anchors.json", b"{}")],
        );
        assert_eq!(
            fw.files.len(),
            2,
            "the .zst copies aren't files of their own"
        );
        let dll = fw.files.iter().find(|f| f.name == "a.dll").unwrap();
        assert!(dll.zst.unwrap() < dll.size / 10, "{dll:?}");

        // Only the compressed copy is there: it's what gets fetched.
        std::fs::remove_file(hosted.join("a.dll")).unwrap();
        let channel = Location::Path(dir.join("dist").join("channel.json"));
        let got = fw.fetch(&channel, &dir.join("v1"), &[]).unwrap();
        assert_eq!((got.downloaded, got.reused), (2, 0));
        assert!(got.bytes < 200, "{got:?}");
        fw.check_dir(&dir.join("v1")).unwrap();

        // A bad copy falls back to the file; with no file either, it fails.
        std::fs::write(hosted.join("a.dll.zst"), b"not zstd").unwrap();
        assert!(fw.fetch(&channel, &dir.join("v2"), &[]).is_err());
        std::fs::write(hosted.join("a.dll"), &host_dll).unwrap();
        fw.fetch(&channel, &dir.join("v2"), &[]).unwrap();
        fw.check_dir(&dir.join("v2")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unchanged_files_come_from_the_installed_version() {
        let dir = scratch("reuse");
        let channel = Location::Path(dir.join("dist").join("channel.json"));
        let (v1, _) = host(
            &dir,
            "1.0.0",
            &[("a.dll", b"host 1"), ("anchors.json", b"{\"v\":1}")],
        );
        let installed = dir.join("versions").join("1.0.0");
        v1.fetch(&channel, &installed, &[]).unwrap();

        // 1.1.0 changes only the data, and its host isn't even hosted.
        let (v2, hosted) = host(
            &dir,
            "1.1.0",
            &[("a.dll", b"host 1"), ("anchors.json", b"{\"v\":2}")],
        );
        for f in ["a.dll", "a.dll.zst"] {
            std::fs::remove_file(hosted.join(f)).unwrap();
        }
        let got = v2
            .fetch(
                &channel,
                &dir.join("versions").join("1.1.0"),
                std::slice::from_ref(&installed),
            )
            .unwrap();
        assert_eq!((got.reused, got.downloaded), (1, 1));
        v2.check_dir(&dir.join("versions").join("1.1.0")).unwrap();

        // An installed file that changed on disk isn't reused.
        std::fs::write(installed.join("anchors.json"), b"{\"v\":2}").unwrap();
        std::fs::write(installed.join("a.dll"), b"host X").unwrap();
        assert!(
            v2.fetch(&channel, &dir.join("versions").join("1.1.1"), &[installed])
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_copy_that_unpacks_too_large_is_refused() {
        let packed = zstd::bulk::compress(&[0u8; 4096], 3).unwrap();
        assert_eq!(unpack(&packed, 4096).map(|b| b.len()), Some(4096));
        assert_eq!(
            unpack(&packed, 100).map(|b| b.len()),
            Some(101),
            "stops one byte past the listing"
        );
    }
}
