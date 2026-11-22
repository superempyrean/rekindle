//! The attach gate: the images rekindle may attach to, by the SHA-256 of
//! their file.
//!
//! An empty allowlist refuses every image. The gate keeps rekindle from
//! attaching to the wrong binary by mistake. It is not a security control:
//! anyone who can point rekindle at another binary can patch the check out.

use std::path::Path;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// The environment variable [`AllowList::from_env`] reads: SHA-256 digests
/// separated by `;`, `,` or whitespace, each with an optional `sha256:`
/// prefix.
pub const ALLOW_ENV: &str = "REKINDLE_ATTACH_ALLOW";

/// The identity of one build of an image: its SHA-256 and size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageFingerprint {
    digest: String,
    size: u64,
}

impl ImageFingerprint {
    /// The fingerprint of an image's bytes.
    pub fn of(bytes: &[u8]) -> Self {
        Self {
            digest: format!("{:x}", Sha256::digest(bytes)),
            size: bytes.len() as u64,
        }
    }

    /// The fingerprint of an image file.
    pub fn of_file(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Ok(Self::of(&bytes))
    }

    /// The digest in lowercase hex.
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// The image's size in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The first 12 digits of the digest, enough to tell builds apart in a
    /// log line.
    pub fn short(&self) -> &str {
        &self.digest[..12]
    }
}

/// The images rekindle may attach to. The default list is empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AllowList {
    digests: Vec<String>,
}

impl AllowList {
    /// Parse digests in the format of [`ALLOW_ENV`].
    pub fn parse(spec: &str) -> Self {
        let digests = spec
            .split(|c: char| c == ';' || c == ',' || c.is_whitespace())
            .filter(|token| !token.is_empty())
            .map(|token| token.trim_start_matches("sha256:").to_ascii_lowercase())
            .collect();
        Self { digests }
    }

    /// The list in [`ALLOW_ENV`], or an empty one if it isn't set.
    pub fn from_env() -> Self {
        std::env::var(ALLOW_ENV)
            .map(|spec| Self::parse(&spec))
            .unwrap_or_default()
    }

    /// Whether the list refuses every image.
    pub fn is_empty(&self) -> bool {
        self.digests.is_empty()
    }

    /// Whether the image's digest is on the list.
    pub fn contains(&self, fingerprint: &ImageFingerprint) -> bool {
        self.digests.contains(&fingerprint.digest)
    }
}

/// Check the image at `path` against `allow`, and return its fingerprint.
/// Fails if the list is empty, the file can't be read or its digest isn't
/// listed.
pub fn verify(path: &Path, allow: &AllowList) -> Result<ImageFingerprint> {
    if allow.is_empty() {
        bail!("refusing to attach: no allowed images configured in {ALLOW_ENV}");
    }
    let fingerprint = ImageFingerprint::of_file(path)?;
    if !allow.contains(&fingerprint) {
        bail!(
            "refusing to attach: {} (sha256 {}, {} bytes) is not allowlisted",
            path.display(),
            fingerprint.short(),
            fingerprint.size(),
        );
    }
    Ok(fingerprint)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint() -> ImageFingerprint {
        ImageFingerprint::of(b"a mock client build")
    }

    #[test]
    fn fingerprint_is_deterministic_and_distinguishing() {
        assert_eq!(fingerprint(), fingerprint());
        assert_ne!(fingerprint(), ImageFingerprint::of(b"a different build"));
        assert_eq!(fingerprint().digest().len(), 64);
        assert_eq!(fingerprint().short(), &fingerprint().digest()[..12]);
        assert_eq!(fingerprint().size(), 19);
    }

    #[test]
    fn empty_allowlist_refuses_before_reading_the_image() {
        let err = verify(Path::new("no such image"), &AllowList::default())
            .unwrap_err()
            .to_string();
        assert!(err.contains("no allowed images"), "{err}");
    }

    #[test]
    fn parses_separators_and_prefixes() {
        let allow = AllowList::parse("sha256:AAA;bbb,ccc ddd");
        let listed = |digest: &str| {
            allow.contains(&ImageFingerprint {
                digest: digest.into(),
                size: 0,
            })
        };
        assert!(["aaa", "bbb", "ccc", "ddd"].into_iter().all(listed));
        assert!(!listed("sha256:aaa"));
    }

    #[test]
    fn matching_image_is_verified_and_mismatch_is_refused() {
        let path = std::env::temp_dir().join(format!("rekindle-hook-gate-{}", std::process::id()));
        std::fs::write(&path, b"a mock client build").unwrap();

        let allow = AllowList::parse(fingerprint().digest());
        let verified = verify(&path, &allow).expect("allowlisted image should verify");
        assert_eq!(verified, fingerprint());

        let other = AllowList::parse(&"0".repeat(64));
        let err = verify(&path, &other).unwrap_err().to_string();
        assert!(err.contains("not allowlisted"), "{err}");
        std::fs::remove_file(path).unwrap();
    }
}
