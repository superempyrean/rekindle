//! Read rekindle's bundled files straight from the installed APK.
//!
//! A one-tap install can't `adb push` a config dir, so the patcher bakes
//! rekindle's config under `assets/rekindle/` and leaves the client library
//! in `lib/<abi>/` (both stored, uncompressed). The APK is mapped into the
//! process, so its path is in `/proc/self/maps`; this module finds it, reads
//! the central directory, and copies out the entries the host needs:
//!
//! - [`seed_config`] mirrors `assets/rekindle/` into the config dir on first
//!   run (anchors, the plugin home, themes).
//! - [`extract_library`] copies the client library out so the anchors have a
//!   file to resolve against ([`crate::image`]), since a library mapped with
//!   `extractNativeLibs=false` has no extracted file of its own.
//!
//! The reader handles stored entries only, which is what the patcher and the
//! Android build tooling write for both; a wanted entry stored deflated is an
//! error rather than silently skipped.

use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Files baked in under this prefix are rekindle's config tree.
const ASSET_PREFIX: &str = "assets/rekindle/";

/// Mirror the `assets/rekindle/` tree from the APK into `dest`, returning how
/// many files were written. Directories are created as needed; an existing
/// file is overwritten. No APK, or no such entries, is `Ok(0)`.
pub(crate) fn seed_config(dest: &Path) -> Result<usize> {
    let mut written = 0;
    for apk in apk_paths() {
        let Ok(entries) = central_directory(&apk) else {
            continue;
        };
        for e in &entries {
            let Some(rel) = e.name.strip_prefix(ASSET_PREFIX) else {
                continue;
            };
            if rel.is_empty() || rel.ends_with('/') {
                continue;
            }
            if e.method != STORED {
                bail!("{} in {} is not stored", e.name, apk.display());
            }
            let out = dest.join(rel);
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let bytes = read_entry(&apk, e)?;
            std::fs::write(&out, bytes).with_context(|| format!("writing {}", out.display()))?;
            written += 1;
        }
        if written > 0 {
            break;
        }
    }
    Ok(written)
}

/// Copy the client library named `file` (found in any `lib/<abi>/` of the
/// APK) to `out`. Used when there is no adb-seeded copy to resolve anchors
/// against.
pub(crate) fn extract_library(file: &str, out: &Path) -> Result<()> {
    let want = format!("/{file}");
    for apk in apk_paths() {
        let Ok(entries) = central_directory(&apk) else {
            continue;
        };
        let Some(e) = entries
            .iter()
            .find(|e| e.name.starts_with("lib/") && e.name.ends_with(&want))
        else {
            continue;
        };
        if e.method != STORED {
            bail!("{} in {} is not stored", e.name, apk.display());
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = out.with_extension("tmp");
        let bytes = read_entry(&apk, e)?;
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, out)?;
        return Ok(());
    }
    bail!("{file} not found in any mapped APK");
}

/// The APKs mapped into this process (base and the config splits), from
/// `/proc/self/maps`: `/data/app/.../*.apk`, de-duplicated, base first.
// APK paths from the kernel are lowercase, so a plain extension match is right.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn apk_paths() -> Vec<PathBuf> {
    let text = std::fs::read_to_string("/proc/self/maps").unwrap_or_default();
    let mut seen = BTreeSet::new();
    let mut paths = Vec::new();
    for line in text.lines() {
        let Some(path) = line.split_whitespace().nth(5) else {
            continue;
        };
        if path.starts_with("/data/app/") && path.ends_with(".apk") && seen.insert(path.to_string())
        {
            paths.push(PathBuf::from(path));
        }
    }
    // base.apk holds the bundled assets; read it first.
    paths.sort_by_key(|p| p.file_name().is_none_or(|n| n != "base.apk"));
    paths
}

const STORED: u16 = 0;
const EOCD_SIG: u32 = 0x0605_4b50;
const CDH_SIG: u32 = 0x0201_4b50;
const LFH_SIG: u32 = 0x0403_4b50;

/// One central-directory entry: enough to locate and read its stored data.
struct CdEntry {
    name: String,
    method: u16,
    comp_size: u64,
    local_header: u64,
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Read a file's central directory. Reads the end for the EOCD record, then
/// the directory region it points at; the whole APK is never loaded.
fn central_directory(path: &Path) -> Result<Vec<CdEntry>> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    // The EOCD is the last 22 bytes plus up to 64 KiB of comment.
    let tail_len = len.min(22 + 0xffff);
    let mut tail = vec![0u8; tail_len as usize];
    f.seek(SeekFrom::Start(len - tail_len))?;
    f.read_exact(&mut tail)?;
    let eocd = (0..=tail.len().saturating_sub(22))
        .rev()
        .find(|&i| u32le(&tail, i) == EOCD_SIG)
        .context("no end-of-central-directory record")?;
    let cd_size = u64::from(u32le(&tail, eocd + 12));
    let cd_offset = u64::from(u32le(&tail, eocd + 16));
    let mut cd = vec![0u8; cd_size as usize];
    f.seek(SeekFrom::Start(cd_offset))?;
    f.read_exact(&mut cd)?;

    let mut entries = Vec::new();
    let mut i = 0usize;
    while i + 46 <= cd.len() {
        if u32le(&cd, i) != CDH_SIG {
            break;
        }
        let method = u16le(&cd, i + 10);
        let comp_size = u64::from(u32le(&cd, i + 20));
        let name_len = u16le(&cd, i + 28) as usize;
        let extra_len = u16le(&cd, i + 30) as usize;
        let comment_len = u16le(&cd, i + 32) as usize;
        let local_header = u64::from(u32le(&cd, i + 42));
        let name_start = i + 46;
        let name_end = name_start + name_len;
        if name_end > cd.len() {
            break;
        }
        let name = String::from_utf8_lossy(&cd[name_start..name_end]).into_owned();
        entries.push(CdEntry {
            name,
            method,
            comp_size,
            local_header,
        });
        i = name_end + extra_len + comment_len;
    }
    Ok(entries)
}

/// Read a stored entry's bytes, following its local header to the data.
fn read_entry(path: &Path, e: &CdEntry) -> Result<Vec<u8>> {
    let mut f = std::fs::File::open(path)?;
    let mut lfh = [0u8; 30];
    f.seek(SeekFrom::Start(e.local_header))?;
    f.read_exact(&mut lfh)?;
    if u32le(&lfh, 0) != LFH_SIG {
        bail!("{}: bad local header for {}", path.display(), e.name);
    }
    let name_len = u64::from(u16le(&lfh, 26));
    let extra_len = u64::from(u16le(&lfh, 28));
    let data = e.local_header + 30 + name_len + extra_len;
    let mut buf = vec![0u8; e.comp_size as usize];
    f.seek(SeekFrom::Start(data))?;
    f.read_exact(&mut buf)?;
    Ok(buf)
}
