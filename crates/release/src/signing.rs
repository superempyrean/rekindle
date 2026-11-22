//! Ed25519 keys and detached signatures for plugins and the release channel.
//!
//! A signature never covers a file directly. It covers a domain-separated
//! message, `domain || sha256(bytes)`, where the domain says what the bytes
//! are (`rekindle-plugin-sig-v1\0` or `rekindle-channel-sig-v1\0`). A plugin
//! signature can therefore never be replayed as a channel signature, or the
//! other way round, even when the same key makes both.
//!
//! Formats (all JSON, `format: 1`):
//! - `<stem>.secret.json`: the 32-byte Ed25519 seed. Secret.
//! - `<stem>.public.json`: the public half.
//! - `<file>.sig` ([`Signature`]): key id, sha256 of the exact signed bytes, and the signature.
//!
//! Key ids are the first 16 hex chars of `sha256(public key bytes)`.
//!
//! A plugin signed by one of the project's keys ([`first_party_keys`],
//! compiled in) is first-party. There is no user-managed trust store: users
//! don't pick keys (owner decision 2026-11-07); everything else is
//! third-party and needs developer mode.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::location::with_suffix;

/// Version of every JSON format in this module.
pub const FORMAT: u32 = 1;

const PLUGIN_DOMAIN: &[u8] = b"rekindle-plugin-sig-v1\0";
const CHANNEL_DOMAIN: &[u8] = b"rekindle-channel-sig-v1\0";

/// Length of a key id in hex characters.
const KEY_ID_LEN: usize = 16;

/// The project's signing keys, `(name, base64 Ed25519 public key)`. A plugin
/// signed by one of these is first-party. The secret halves are held by the
/// repo owner, never in the repo. To rotate, add the new key here, re-sign,
/// then drop the old one a release later.
pub const FIRST_PARTY_KEYS: &[(&str, &str)] = &[(
    "rekindle project",
    "dZh7tKmjYnGf2xFrFxcXt+UIR+Y7CD0oQHsCulJBUwk=",
)];

/// [`FIRST_PARTY_KEYS`], parsed.
pub fn first_party_keys() -> Vec<PublicKey> {
    FIRST_PARTY_KEYS
        .iter()
        .map(|(name, b64)| {
            PublicKey::from_base64(b64, *name).expect("FIRST_PARTY_KEYS holds valid keys")
        })
        .collect()
}

/// Lowercase hex sha256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        // Writing to a String can't fail.
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
    }
    Some(out)
}

/// Is `s` a well-formed lowercase sha256 hex digest?
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The key id of an Ed25519 public key.
pub fn key_id(key: &VerifyingKey) -> String {
    sha256_hex(key.as_bytes())[..KEY_ID_LEN].to_string()
}

/// `<path>.sig`, the conventional location of a detached signature.
pub fn sig_path_for(path: &Path) -> PathBuf {
    with_suffix(path, ".sig")
}

/// What a signature covers. Part of the signed message via the domain prefix,
/// and recorded in the file so a mix-up gives a readable error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SigKind {
    Plugin,
    Channel,
}

impl SigKind {
    fn domain(self) -> &'static [u8] {
        match self {
            SigKind::Plugin => PLUGIN_DOMAIN,
            SigKind::Channel => CHANNEL_DOMAIN,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            SigKind::Plugin => "plugin",
            SigKind::Channel => "channel",
        }
    }
}

fn message(kind: SigKind, digest: &[u8; 32]) -> Vec<u8> {
    let mut m = kind.domain().to_vec();
    m.extend_from_slice(digest);
    m
}

/// Why a signature doesn't verify.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SigError {
    #[error("unsupported signature format {0} (this host reads {FORMAT})")]
    Format(u32),
    #[error("this is a {found} signature, expected a {expected} signature", found = .found.as_str(), expected = .expected.as_str())]
    WrongKind { expected: SigKind, found: SigKind },
    #[error(
        "the file does not match its signature (sha256 is {actual}, signature covers {signed}); it was modified after signing"
    )]
    DigestMismatch { signed: String, actual: String },
    #[error("signature was made by key {signed_by}, not {key}")]
    KeyMismatch { signed_by: String, key: String },
    #[error("signature by key {0} is invalid")]
    BadSignature(String),
    #[error("malformed signature: {0}")]
    Malformed(String),
}

/// A detached signature file (`*.sig`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub format: u32,
    pub kind: SigKind,
    pub key_id: String,
    /// sha256 (hex) of the exact signed bytes.
    pub sha256: String,
    /// Base64 Ed25519 signature over `domain || sha256`.
    pub signature: String,
}

impl Signature {
    /// Read a `.sig` file.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_json(&bytes).with_context(|| format!("parsing signature {}", path.display()))
    }

    /// Parse a signature from the bytes of a `.sig` file.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }

    /// The signature as a `.sig` file's text.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("signature serializes") + "\n"
    }

    /// Write the signature to `path`.
    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, self.to_json()).with_context(|| format!("writing {}", path.display()))
    }

    /// Checks that need no key: format, kind, and that `bytes` are what was
    /// signed. A failure here means the signature is invalid for these bytes,
    /// whoever made it.
    pub fn check_bytes(&self, kind: SigKind, bytes: &[u8]) -> Result<(), SigError> {
        if self.format != FORMAT {
            return Err(SigError::Format(self.format));
        }
        if self.kind != kind {
            return Err(SigError::WrongKind {
                expected: kind,
                found: self.kind,
            });
        }
        let actual = sha256_hex(bytes);
        if actual != self.sha256 {
            return Err(SigError::DigestMismatch {
                signed: self.sha256.clone(),
                actual,
            });
        }
        Ok(())
    }
}

/// An Ed25519 public key with a display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicKey {
    key: VerifyingKey,
    pub name: String,
}

impl PublicKey {
    fn new(key: VerifyingKey, name: impl Into<String>) -> Self {
        Self {
            key,
            name: name.into(),
        }
    }

    /// This key's id.
    pub fn key_id(&self) -> String {
        key_id(&self.key)
    }

    /// The 32 key bytes, base64.
    pub fn to_base64(&self) -> String {
        B64.encode(self.key.as_bytes())
    }

    /// A key from its base64 bytes, as [`PublicKey::to_base64`] writes them.
    pub fn from_base64(b64: &str, name: impl Into<String>) -> Result<Self> {
        let raw = B64.decode(b64.trim()).context("public key is not base64")?;
        let raw: [u8; 32] = raw
            .try_into()
            .map_err(|_| anyhow::anyhow!("public key must be 32 bytes"))?;
        let key = VerifyingKey::from_bytes(&raw).context("not a valid Ed25519 public key")?;
        Ok(Self::new(key, name))
    }

    fn to_file(&self) -> PublicKeyFile {
        PublicKeyFile {
            format: FORMAT,
            key_id: self.key_id(),
            name: self.name.clone(),
            public_key: self.to_base64(),
        }
    }

    /// Read a `<stem>.public.json` file.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let f: PublicKeyFile = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing public key {}", path.display()))?;
        f.into_key()
    }

    /// Write the key to `path` as a `<stem>.public.json` file.
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.to_file())? + "\n";
        std::fs::write(path, json).with_context(|| format!("writing {}", path.display()))
    }

    fn verify(&self, kind: SigKind, bytes: &[u8], sig: &Signature) -> Result<(), SigError> {
        sig.check_bytes(kind, bytes)?;
        let id = self.key_id();
        if sig.key_id != id {
            return Err(SigError::KeyMismatch {
                signed_by: sig.key_id.clone(),
                key: id,
            });
        }
        let raw = B64
            .decode(sig.signature.trim())
            .map_err(|_| SigError::Malformed("signature is not base64".into()))?;
        let raw: [u8; 64] = raw
            .try_into()
            .map_err(|_| SigError::Malformed("signature must be 64 bytes".into()))?;
        let digest =
            unhex32(&sig.sha256).ok_or_else(|| SigError::Malformed("bad sha256".into()))?;
        self.key
            .verify_strict(
                &message(kind, &digest),
                &ed25519_dalek::Signature::from_bytes(&raw),
            )
            .map_err(|_| SigError::BadSignature(id))
    }

    /// Verify a detached plugin signature over the exact `.wasm` bytes.
    pub fn verify_plugin(&self, wasm: &[u8], sig: &Signature) -> Result<(), SigError> {
        self.verify(SigKind::Plugin, wasm, sig)
    }

    /// Verify a detached channel signature over the exact `channel.json` bytes.
    pub fn verify_channel(&self, channel: &[u8], sig: &Signature) -> Result<(), SigError> {
        self.verify(SigKind::Channel, channel, sig)
    }
}

/// `<stem>.public.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PublicKeyFile {
    format: u32,
    key_id: String,
    name: String,
    /// Base64 of the 32-byte Ed25519 public key.
    public_key: String,
}

impl PublicKeyFile {
    fn into_key(self) -> Result<PublicKey> {
        if self.format != FORMAT {
            bail!("unsupported key format {}", self.format);
        }
        let k = PublicKey::from_base64(&self.public_key, self.name)?;
        if k.key_id() != self.key_id {
            bail!(
                "key file says key id {} but the key's id is {}",
                self.key_id,
                k.key_id()
            );
        }
        Ok(k)
    }
}

/// `<stem>.secret.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SecretKeyFile {
    format: u32,
    warning: String,
    key_id: String,
    name: String,
    public_key: String,
    /// Base64 of the 32-byte Ed25519 seed.
    secret_key: String,
}

const SECRET_WARNING: &str = "SECRET signing key. Anyone with this file can sign plugins as you. Never commit, share, or upload it.";

/// A signing key pair.
pub struct Keypair {
    signing: SigningKey,
    pub name: String,
}

impl std::fmt::Debug for Keypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keypair")
            .field("key_id", &self.key_id())
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Keypair {
    /// A fresh key from the OS random source.
    pub fn generate(name: impl Into<String>) -> Result<Self> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| anyhow::anyhow!("OS random source failed: {e}"))?;
        Ok(Self::from_seed(seed, name))
    }

    /// The key whose Ed25519 seed is `seed`.
    pub fn from_seed(seed: [u8; 32], name: impl Into<String>) -> Self {
        Self {
            signing: SigningKey::from_bytes(&seed),
            name: name.into(),
        }
    }

    /// This key's id.
    pub fn key_id(&self) -> String {
        key_id(&self.signing.verifying_key())
    }

    /// The public half.
    pub fn public(&self) -> PublicKey {
        PublicKey::new(self.signing.verifying_key(), self.name.clone())
    }

    fn sign(&self, kind: SigKind, bytes: &[u8]) -> Signature {
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        let sig = self.signing.sign(&message(kind, &digest));
        Signature {
            format: FORMAT,
            kind,
            key_id: self.key_id(),
            sha256: hex(&digest),
            signature: B64.encode(sig.to_bytes()),
        }
    }

    /// Sign the exact bytes of a plugin's `.wasm`.
    pub fn sign_plugin(&self, wasm: &[u8]) -> Signature {
        self.sign(SigKind::Plugin, wasm)
    }

    /// Sign the exact bytes of a `channel.json`.
    pub fn sign_channel(&self, channel: &[u8]) -> Signature {
        self.sign(SigKind::Channel, channel)
    }

    fn to_secret_file(&self) -> SecretKeyFile {
        SecretKeyFile {
            format: FORMAT,
            warning: SECRET_WARNING.into(),
            key_id: self.key_id(),
            name: self.name.clone(),
            public_key: self.public().to_base64(),
            secret_key: B64.encode(self.signing.to_bytes()),
        }
    }

    /// Read a `<stem>.secret.json` file.
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let f: SecretKeyFile = serde_json::from_slice(&bytes)
            .with_context(|| format!("{} is not a rekindle secret key file", path.display()))?;
        if f.format != FORMAT {
            bail!("unsupported key format {}", f.format);
        }
        let seed = B64
            .decode(f.secret_key.trim())
            .context("secret key is not base64")?;
        let seed: [u8; 32] = seed
            .try_into()
            .map_err(|_| anyhow::anyhow!("secret key must be 32 bytes"))?;
        let kp = Self::from_seed(seed, f.name);
        if kp.key_id() != f.key_id {
            bail!(
                "{}: key id {} does not match the secret key ({})",
                path.display(),
                f.key_id,
                kp.key_id()
            );
        }
        Ok(kp)
    }

    /// Write `<stem>.secret.json` and `<stem>.public.json`, and return their
    /// paths. Refuses to overwrite an existing secret key.
    pub fn save(&self, stem: &Path) -> Result<(PathBuf, PathBuf)> {
        let secret = with_suffix(stem, ".secret.json");
        let public = with_suffix(stem, ".public.json");
        let json = serde_json::to_string_pretty(&self.to_secret_file())? + "\n";
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&secret)
            .with_context(|| {
                format!(
                    "creating {} (refusing to overwrite an existing key)",
                    secret.display()
                )
            })?;
        std::io::Write::write_all(&mut f, json.as_bytes())
            .with_context(|| format!("writing {}", secret.display()))?;
        self.public().save(&public)?;
        Ok((secret, public))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> Keypair {
        Keypair::from_seed([n; 32], format!("key {n}"))
    }

    #[test]
    fn sign_verify_roundtrip() {
        let k = key(1);
        let sig = k.sign_plugin(b"wasm bytes");
        assert_eq!(sig.key_id.len(), KEY_ID_LEN);
        k.public().verify_plugin(b"wasm bytes", &sig).unwrap();
        let sig = Signature::from_json(sig.to_json().as_bytes()).unwrap();
        k.public().verify_plugin(b"wasm bytes", &sig).unwrap();
    }

    #[test]
    fn tamper_is_detected() {
        let k = key(1);
        let mut wasm = b"some plugin bytes".to_vec();
        let sig = k.sign_plugin(&wasm);
        wasm[3] ^= 1;
        assert!(matches!(
            k.public().verify_plugin(&wasm, &sig),
            Err(SigError::DigestMismatch { .. })
        ));

        // Tampering with the sha256 field to match still fails the signature.
        let mut forged = sig.clone();
        forged.sha256 = sha256_hex(&wasm);
        assert_eq!(
            k.public().verify_plugin(&wasm, &forged),
            Err(SigError::BadSignature(k.key_id()))
        );

        // So does flipping a bit of the signature itself.
        let mut raw = B64.decode(&sig.signature).unwrap();
        raw[10] ^= 0x40;
        let flipped = Signature {
            signature: B64.encode(raw),
            ..sig
        };
        wasm[3] ^= 1;
        assert!(k.public().verify_plugin(&wasm, &flipped).is_err());
    }

    #[test]
    fn wrong_key_fails() {
        let sig = key(1).sign_plugin(b"x");
        assert!(matches!(
            key(2).public().verify_plugin(b"x", &sig),
            Err(SigError::KeyMismatch { .. })
        ));
        // Even if the key id is relabelled, the signature doesn't verify.
        let relabelled = Signature {
            key_id: key(2).key_id(),
            ..sig
        };
        assert!(matches!(
            key(2).public().verify_plugin(b"x", &relabelled),
            Err(SigError::BadSignature(_))
        ));
    }

    #[test]
    fn domain_separation() {
        let k = key(1);
        let sig = k.sign_plugin(b"same bytes");
        assert!(matches!(
            k.public().verify_channel(b"same bytes", &sig),
            Err(SigError::WrongKind { .. })
        ));
        // Relabelling the kind doesn't help: the signed message differs.
        let relabelled = Signature {
            kind: SigKind::Channel,
            ..sig
        };
        assert!(matches!(
            k.public().verify_channel(b"same bytes", &relabelled),
            Err(SigError::BadSignature(_))
        ));
        let isig = k.sign_channel(b"same bytes");
        k.public().verify_channel(b"same bytes", &isig).unwrap();
        let relabelled = Signature {
            kind: SigKind::Plugin,
            ..isig
        };
        assert!(
            k.public()
                .verify_plugin(b"same bytes", &relabelled)
                .is_err()
        );
    }

    #[test]
    fn key_files_roundtrip() {
        let dir = std::env::temp_dir().join(format!("rekindle-signing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let k = Keypair::generate("Alice").unwrap();
        let (secret, public) = k.save(&dir.join("alice")).unwrap();
        assert!(
            k.save(&dir.join("alice")).is_err(),
            "must not overwrite a secret key"
        );
        let loaded = Keypair::load(&secret).unwrap();
        assert_eq!(loaded.key_id(), k.key_id());
        let pk = PublicKey::load(&public).unwrap();
        assert_eq!(pk, k.public());
        assert_eq!(pk.name, "Alice");
        assert!(
            Keypair::load(&public).is_err(),
            "a public key file is not a secret key"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn first_party_keys_parse() {
        let keys = first_party_keys();
        assert_eq!(keys.len(), FIRST_PARTY_KEYS.len());
        assert!(keys.iter().all(|k| k.key_id().len() == KEY_ID_LEN));
    }
}
