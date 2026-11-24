//! `Storage` on disk for a live run, sealed with DPAPI.
//!
//! Every key the guest stores, secure or not, goes into one file that
//! `CryptProtectData` seals for the current Windows user. Nothing the guest
//! stores is ever written in the clear. The file is
//! `%LOCALAPPDATA%\phoenix-view\storage.dpapi`, outside the repository.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

/// The sealed file's default place.
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, PathBuf::from);
    base.join("phoenix-view").join("storage.dpapi")
}

/// The guest's storage, kept in memory and sealed to `path` on every change.
pub struct Sealed {
    path: PathBuf,
}

impl Sealed {
    /// Open the file at `path`, or start empty if there is none.
    pub fn open(path: &Path) -> Result<(Sealed, HashMap<String, Vec<u8>>)> {
        let map = match std::fs::read(path) {
            Ok(sealed) => decode(&unprotect(&sealed).with_context(|| {
                format!(
                    "unsealing {} (another Windows user's file?)",
                    path.display()
                )
            })?)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok((
            Sealed {
                path: path.to_path_buf(),
            },
            map,
        ))
    }

    /// Seal `map` and replace the file with it.
    pub fn save(&self, map: &HashMap<String, Vec<u8>>) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let sealed = protect(&encode(map))?;
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, sealed)?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("writing {}", self.path.display()))
    }
}

/// `u32` count, then per entry a `u32`-length key and a `u32`-length value,
/// little-endian, in key order.
fn encode(map: &HashMap<String, Vec<u8>>) -> Vec<u8> {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let mut out = Vec::new();
    out.extend_from_slice(&(keys.len() as u32).to_le_bytes());
    for k in keys {
        for part in [k.as_bytes(), &map[k]] {
            out.extend_from_slice(&(part.len() as u32).to_le_bytes());
            out.extend_from_slice(part);
        }
    }
    out
}

fn decode(mut b: &[u8]) -> Result<HashMap<String, Vec<u8>>> {
    fn take<'a>(b: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
        if b.len() < n {
            bail!("the storage file is truncated");
        }
        let (head, rest) = b.split_at(n);
        *b = rest;
        Ok(head)
    }
    fn len(b: &mut &[u8]) -> Result<usize> {
        Ok(u32::from_le_bytes(take(b, 4)?.try_into()?) as usize)
    }
    let n = len(&mut b)?;
    let mut map = HashMap::with_capacity(n.min(4096));
    for _ in 0..n {
        let kl = len(&mut b)?;
        let k = String::from_utf8(take(&mut b, kl)?.to_vec())?;
        let vl = len(&mut b)?;
        map.insert(k, take(&mut b, vl)?.to_vec());
    }
    Ok(map)
}

#[cfg(windows)]
fn protect(data: &[u8]) -> Result<Vec<u8>> {
    dpapi(data, true)
}

#[cfg(windows)]
fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
    dpapi(data, false)
}

#[cfg(windows)]
fn dpapi(data: &[u8], seal: bool) -> Result<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(data.len())?,
        pbData: data.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: `input` points at `data`, which outlives the call; DPAPI
    // allocates `output` with LocalAlloc, freed below after the copy.
    let ok = unsafe {
        if seal {
            CryptProtectData(
                &raw const input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        } else {
            CryptUnprotectData(
                &raw const input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        }
    };
    if ok == 0 {
        bail!(
            "{} failed: {}",
            if seal {
                "CryptProtectData"
            } else {
                "CryptUnprotectData"
            },
            std::io::Error::last_os_error()
        );
    }
    // SAFETY: DPAPI returned `cbData` bytes at `pbData`, which this frees once.
    let out = unsafe {
        let v = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData.cast());
        v
    };
    Ok(out)
}

#[cfg(not(windows))]
fn protect(_: &[u8]) -> Result<Vec<u8>> {
    bail!("sealed storage needs Windows DPAPI")
}

#[cfg(not(windows))]
fn unprotect(_: &[u8]) -> Result<Vec<u8>> {
    bail!("sealed storage needs Windows DPAPI")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seals_and_opens() {
        let dir = std::env::temp_dir().join(format!("phoenix-view-store-{}", std::process::id()));
        let path = dir.join("storage.dpapi");
        let mut map = HashMap::new();
        map.insert("pending:token".to_string(), b"not-a-token".to_vec());
        map.insert("AccessibilityStore".to_string(), vec![1, 2, 3]);
        let (s, empty) = Sealed::open(&path).unwrap();
        assert!(empty.is_empty());
        s.save(&map).unwrap();
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw.windows(11).any(|w| w == b"not-a-token"));
        let (_, back) = Sealed::open(&path).unwrap();
        assert_eq!(back, map);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
