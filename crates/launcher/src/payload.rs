//! The setup program is the launcher with rekindle's files appended:
//! one `.exe` to download and double-click.
//!
//! ```text
//! [launcher exe][file 0][file 1]…[index JSON][index length: u64 LE][MAGIC]
//! ```
//!
//! Windows runs the exe part and ignores the rest. The index lists each
//! file's name, offset (from the end of the exe part), size and sha256, so
//! a damaged download is caught before anything is installed. Installing
//! copies only the exe part as `rekindle.exe`.
//!
//! A setup program is code-signed after it is packed. Authenticode hashes
//! the appended data with the rest of the file, pads the file to 8 bytes
//! and appends its certificate table, which the exe's header then points
//! at:
//!
//! ```text
//! [launcher exe][files][index JSON][index length][MAGIC][padding][certificate table]
//! ```
//!
//! [`read`] finds the appended data in front of the certificate table. The
//! exe part of a signed setup still points at that table, which isn't in
//! the copy, so [`read`] returns it with the pointer cleared: an unsigned
//! `rekindle.exe`, not one with a broken signature.

use anyhow::{Context, Result, bail};
use rekindle_release::framework::{Framework, FrameworkFile, is_plain_name};
use rekindle_release::signing;
use serde::{Deserialize, Serialize};

const MAGIC: &[u8; 8] = b"RKSETUP1";

/// The bytes after the index: its length (`u64` LE), then [`MAGIC`].
const TAIL: usize = 16;

/// rekindle's files for one version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    pub version: String,
    pub files: Vec<(String, Vec<u8>)>,
}

#[derive(Serialize, Deserialize)]
struct Index {
    version: String,
    exe_len: u64,
    files: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    name: String,
    offset: u64,
    size: u64,
    sha256: String,
}

impl Bundle {
    /// The files as a framework listing (what the release channel lists).
    pub fn framework(&self) -> Framework {
        Framework {
            version: self.version.clone(),
            base: format!("framework/{}/", self.version),
            files: self
                .files
                .iter()
                .map(|(name, bytes)| FrameworkFile {
                    name: name.clone(),
                    size: bytes.len() as u64,
                    sha256: signing::sha256_hex(bytes),
                    zst: None,
                })
                .collect(),
        }
    }
}

/// Append `bundle` to the launcher `exe`.
pub fn pack(exe: &[u8], bundle: &Bundle) -> Result<Vec<u8>> {
    bundle.framework().validate()?;
    let mut out = exe.to_vec();
    let mut files = Vec::new();
    let mut offset = 0u64;
    for (name, bytes) in &bundle.files {
        files.push(Entry {
            name: name.clone(),
            offset,
            size: bytes.len() as u64,
            sha256: signing::sha256_hex(bytes),
        });
        out.extend_from_slice(bytes);
        offset += bytes.len() as u64;
    }
    let index = serde_json::to_vec(&Index {
        version: bundle.version.clone(),
        exe_len: exe.len() as u64,
        files,
    })?;
    // End on an 8-byte boundary, so signing adds no padding after MAGIC.
    let end = out.len() + index.len() + TAIL;
    out.resize(out.len() + (8 - end % 8) % 8, 0);
    out.extend_from_slice(&index);
    out.extend_from_slice(&(index.len() as u64).to_le_bytes());
    out.extend_from_slice(MAGIC);
    Ok(out)
}

/// Where a PE file's header records its checksum and its Authenticode
/// certificate table.
struct PeFields {
    checksum: usize,
    /// The security directory entry: two `u32`s, file offset then size.
    security: usize,
}

/// The [`PeFields`] of `exe`, or `None` if it isn't a PE file with them.
fn pe_fields(exe: &[u8]) -> Option<PeFields> {
    let u32_at = |at: usize| {
        exe.get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")) as usize)
    };
    let pe = u32_at(0x3c)?;
    if exe.get(..2)? != b"MZ" || exe.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    let optional = pe + 24;
    // The data directories follow the optional header's fixed part.
    let directories = match exe.get(optional..optional + 2)? {
        [0x0b, 0x01] => optional + 96,
        [0x0b, 0x02] => optional + 112,
        _ => return None,
    };
    // The security directory is the fifth.
    if u32_at(directories - 4)? < 5 {
        return None;
    }
    let security = directories + 4 * 8;
    exe.get(security..security + 8)?;
    Some(PeFields {
        checksum: optional + 64,
        security,
    })
}

/// Where the appended data ends: at the certificate table of a signed
/// setup (less the padding in front of it), else at the end of the file.
fn appended_end(setup: &[u8]) -> usize {
    let u32_at =
        |at: usize| u32::from_le_bytes(setup[at..at + 4].try_into().expect("4 bytes")) as usize;
    let Some(pe) = pe_fields(setup) else {
        return setup.len();
    };
    let (offset, size) = (u32_at(pe.security), u32_at(pe.security + 4));
    if offset == 0 || offset.checked_add(size) != Some(setup.len()) {
        return setup.len();
    }
    // MAGIC ends in a non-zero byte, so the padding is every zero before
    // the table, at most seven of them.
    let padding = setup[..offset]
        .iter()
        .rev()
        .take(7)
        .take_while(|b| **b == 0)
        .count();
    offset - padding
}

/// The launcher part and the bundle of a setup program, or `None` for a
/// plain launcher (nothing appended). The launcher part of a signed setup
/// is returned unsigned.
pub fn read(setup: &[u8]) -> Result<Option<(Vec<u8>, Bundle)>> {
    let end = appended_end(setup);
    let setup_len = setup.len();
    let setup = &setup[..end];
    let Some(tail) = setup.len().checked_sub(TAIL) else {
        return Ok(None);
    };
    if &setup[tail + 8..] != MAGIC {
        return Ok(None);
    }
    let index_len = u64::from_le_bytes(setup[tail..tail + 8].try_into().expect("8 bytes")) as usize;
    let index_start = tail
        .checked_sub(index_len)
        .context("the setup program is damaged (index)")?;
    let index: Index = serde_json::from_slice(&setup[index_start..tail])
        .context("the setup program is damaged (index)")?;
    let exe_len = index.exe_len as usize;
    if exe_len > index_start {
        bail!("the setup program is damaged (layout)");
    }
    let data = &setup[exe_len..index_start];
    let mut files = Vec::new();
    for e in index.files {
        if !is_plain_name(&e.name) {
            bail!("the setup program lists a bad file name `{}`", e.name);
        }
        let bytes = e
            .offset
            .checked_add(e.size)
            .and_then(|end| data.get(e.offset as usize..end as usize))
            .context("the setup program is damaged (truncated)")?;
        if signing::sha256_hex(bytes) != e.sha256 {
            bail!(
                "the setup program is damaged ({} doesn't match); download it again",
                e.name
            );
        }
        files.push((e.name, bytes.to_vec()));
    }
    let bundle = Bundle {
        version: index.version,
        files,
    };
    bundle.framework().validate()?;
    let mut exe = setup[..exe_len].to_vec();
    if end != setup_len {
        // Signed: the copy has no certificate table, and its checksum
        // covered one.
        if let Some(pe) = pe_fields(&exe) {
            exe[pe.security..pe.security + 8].fill(0);
            exe[pe.checksum..pe.checksum + 4].fill(0);
        }
    }
    Ok(Some((exe, bundle)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> Bundle {
        Bundle {
            version: "0.9.0".into(),
            files: vec![
                ("rekindle_host.dll".into(), b"MZ host".to_vec()),
                ("anchors.json".into(), b"{}".to_vec()),
            ],
        }
    }

    #[test]
    fn round_trips_and_keeps_the_exe_part_intact() {
        let exe = b"MZ launcher bytes".to_vec();
        let setup = pack(&exe, &bundle()).unwrap();
        let (stub, b) = read(&setup).unwrap().unwrap();
        assert_eq!(stub, exe);
        assert_eq!(b, bundle());
        assert!(
            read(&exe).unwrap().is_none(),
            "a plain launcher has no bundle"
        );
    }

    /// The smallest PE64 header [`pe_fields`] accepts, with no checksum and
    /// no certificate table.
    fn pe() -> Vec<u8> {
        let mut exe = vec![0u8; 0x200];
        exe[..2].copy_from_slice(b"MZ");
        exe[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        exe[0x80..0x84].copy_from_slice(b"PE\0\0");
        exe[0x98..0x9a].copy_from_slice(&[0x0b, 0x02]);
        exe[0x98 + 108..0x98 + 112].copy_from_slice(&16u32.to_le_bytes());
        exe.extend_from_slice(b"code");
        exe
    }

    /// What signing does to a file: pad to 8 bytes, append the certificate
    /// table, and record the table and a checksum in the header.
    fn sign(file: &mut Vec<u8>) {
        let f = pe_fields(file).unwrap();
        file.resize(file.len().next_multiple_of(8), 0);
        let table = [0xAAu8; 24];
        let offset = file.len() as u32;
        file.extend_from_slice(&table);
        file[f.security..f.security + 4].copy_from_slice(&offset.to_le_bytes());
        file[f.security + 4..f.security + 8].copy_from_slice(&(table.len() as u32).to_le_bytes());
        file[f.checksum..f.checksum + 4].copy_from_slice(&0x1234u32.to_le_bytes());
    }

    #[test]
    fn a_signed_setup_still_has_its_bundle_and_installs_an_unsigned_exe() {
        let exe = pe();
        let mut setup = pack(&exe, &bundle()).unwrap();
        assert_eq!(
            setup.len() % 8,
            0,
            "packed to an 8-byte boundary, so signing adds no padding"
        );
        sign(&mut setup);
        let (stub, b) = read(&setup).unwrap().unwrap();
        assert_eq!(b, bundle());
        assert_eq!(
            stub, exe,
            "the exe part no longer points at a certificate table"
        );
    }

    #[test]
    fn padding_in_front_of_the_certificate_table_is_skipped() {
        // A setup packed before packing aligned to 8 bytes.
        let exe = pe();
        let mut setup = pack(&exe, &bundle()).unwrap();
        let aligned = setup.len();
        setup.splice(exe.len()..exe.len(), [1, 2, 3]);
        let at = setup.len() - 16;
        let index_len = u64::from_le_bytes(setup[at..at + 8].try_into().unwrap());
        assert_ne!(setup.len(), aligned);
        sign(&mut setup);
        assert_ne!(
            &setup[setup.len() - 24 - 8..setup.len() - 24],
            MAGIC,
            "padding sits between MAGIC and the table"
        );
        // The three bytes shifted every file, so only the index is checked here.
        let end = appended_end(&setup);
        assert_eq!(&setup[end - 8..end], MAGIC);
        assert_eq!(
            u64::from_le_bytes(setup[end - 16..end - 8].try_into().unwrap()),
            index_len
        );
    }

    #[test]
    fn a_signed_plain_launcher_has_no_bundle() {
        let mut exe = pe();
        sign(&mut exe);
        assert!(read(&exe).unwrap().is_none());
    }

    #[test]
    fn damage_is_caught() {
        let exe = b"MZ launcher".to_vec();
        let mut setup = pack(&exe, &bundle()).unwrap();
        setup[exe.len() + 2] ^= 0xff;
        assert!(read(&setup).unwrap_err().to_string().contains("damaged"));
        let mut cut = pack(&exe, &bundle()).unwrap();
        cut.remove(exe.len());
        assert!(read(&cut).is_err(), "a byte missing from the files");
    }

    #[test]
    fn an_index_whose_offsets_overflow_is_damage() {
        let exe = b"MZ launcher".to_vec();
        let index = serde_json::to_vec(&Index {
            version: "0.9.0".into(),
            exe_len: exe.len() as u64,
            files: vec![Entry {
                name: "anchors.json".into(),
                offset: u64::MAX,
                size: 2,
                sha256: String::new(),
            }],
        })
        .unwrap();
        let mut setup = exe.clone();
        setup.extend_from_slice(&index);
        setup.extend_from_slice(&(index.len() as u64).to_le_bytes());
        setup.extend_from_slice(MAGIC);
        assert!(read(&setup).unwrap_err().to_string().contains("damaged"));
    }
}
