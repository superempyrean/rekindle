//! The fonts and resources of the user's own APK, read at run time.
//!
//! A Play install is a base APK and config splits beside it: one for the
//! ABI and one for the screen density, which holds the density-specific
//! drawables. An APK is a zip. This reads each one's central directory once,
//! then inflates an entry on demand. Nothing it reads is written anywhere.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context as _, Result, bail};

use crate::axml;
use crate::res::Table;

struct Entry {
    method: u16,
    csize: u64,
    offset: u64,
}

struct Zip {
    path: PathBuf,
    file: Mutex<File>,
    entries: HashMap<String, Entry>,
}

pub struct Apk {
    /// The base first, then the splits.
    zips: Vec<Zip>,
    table: OnceLock<Option<Table>>,
    /// Splits beside the base that were left out, and why.
    pub skipped: Vec<(PathBuf, String)>,
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

impl Zip {
    fn open(path: &Path) -> Result<Zip> {
        let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let len = file.seek(SeekFrom::End(0))?;
        let tail_len = len.min(65_557);
        file.seek(SeekFrom::Start(len - tail_len))?;
        let mut tail = vec![0; tail_len as usize];
        file.read_exact(&mut tail)?;
        let Some(eocd) = (0..tail.len().saturating_sub(21))
            .rev()
            .find(|&i| u32_at(&tail, i) == 0x0605_4b50)
        else {
            bail!("{} is not a zip", path.display());
        };
        let (count, cd_size, cd_off) = (
            u16_at(&tail, eocd + 10),
            u64::from(u32_at(&tail, eocd + 12)),
            u64::from(u32_at(&tail, eocd + 16)),
        );
        file.seek(SeekFrom::Start(cd_off))?;
        let mut cd = vec![0; cd_size as usize];
        file.read_exact(&mut cd)?;
        let mut entries = HashMap::new();
        let mut p = 0;
        for _ in 0..count {
            if p + 46 > cd.len() || u32_at(&cd, p) != 0x0201_4b50 {
                bail!("{}: a bad central directory", path.display());
            }
            let method = u16_at(&cd, p + 10);
            let csize = u64::from(u32_at(&cd, p + 20));
            let (nlen, elen, clen) = (
                usize::from(u16_at(&cd, p + 28)),
                usize::from(u16_at(&cd, p + 30)),
                usize::from(u16_at(&cd, p + 32)),
            );
            let offset = u64::from(u32_at(&cd, p + 42));
            let name = String::from_utf8_lossy(&cd[p + 46..p + 46 + nlen]).into_owned();
            entries.insert(
                name,
                Entry {
                    method,
                    csize,
                    offset,
                },
            );
            p += 46 + nlen + elen + clen;
        }
        Ok(Zip {
            path: path.to_path_buf(),
            file: Mutex::new(file),
            entries,
        })
    }

    fn read(&self, name: &str) -> Option<Vec<u8>> {
        let e = self.entries.get(name)?;
        let mut file = self.file.lock().ok()?;
        let mut local = [0u8; 30];
        file.seek(SeekFrom::Start(e.offset)).ok()?;
        file.read_exact(&mut local).ok()?;
        if u32_at(&local, 0) != 0x0403_4b50 {
            return None;
        }
        let skip = u64::from(u16_at(&local, 26)) + u64::from(u16_at(&local, 28));
        file.seek(SeekFrom::Current(skip as i64)).ok()?;
        let mut data = vec![0; e.csize as usize];
        file.read_exact(&mut data).ok()?;
        match e.method {
            0 => Some(data),
            8 => {
                let mut out = Vec::new();
                flate2::read::DeflateDecoder::new(&data[..])
                    .read_to_end(&mut out)
                    .ok()?;
                Some(out)
            }
            _ => None,
        }
    }

    fn manifest(&self) -> Option<axml::Element> {
        self.read("AndroidManifest.xml")
            .and_then(|b| axml::parse(&b).ok())
    }

    /// The manifest's `versionCode`.
    fn version_code(&self) -> Option<i32> {
        match self.manifest()?.get("versionCode")? {
            axml::Val::Int(v) => Some(*v),
            _ => None,
        }
    }
}

/// The config splits Play installs beside `base`: `split_config.*.apk`
/// beside `base.apk`, as `pm path` lists them, or `<prefix>config.*.apk`
/// beside `<prefix>base.apk`.
fn sibling_splits(base: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (base.parent(), base.file_name().and_then(|n| n.to_str())) else {
        return vec![];
    };
    let prefix = match name.strip_suffix("base.apk") {
        Some("") => "split_".to_string(),
        Some(p) => p.to_string(),
        None => return vec![],
    };
    let want = format!("{prefix}config.");
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                n.starts_with(&want)
                    && Path::new(n)
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("apk"))
            })
        })
        .collect();
    out.sort();
    out
}

impl Apk {
    /// The base APK, the splits named, and the config splits beside it.
    ///
    /// Every density split found is read. Each one holds the buckets
    /// bundletool picked for its density (356207's `config.hdpi` is mostly
    /// `drawable-xhdpi`), and the merged table chooses per entry, as
    /// [`Table::best`] says, so the window's density picks the file a phone
    /// at that density would get. A split whose `versionCode` differs from
    /// the base's is left out, as Android refuses it: its resource ids are
    /// another build's.
    pub fn open(base: &Path, splits: &[PathBuf]) -> Result<Apk> {
        let mut paths: Vec<PathBuf> = splits.to_vec();
        for p in sibling_splits(base) {
            if !paths.contains(&p) {
                paths.push(p);
            }
        }
        let base = Zip::open(base)?;
        let version = base.version_code();
        let mut zips = vec![base];
        let mut skipped = vec![];
        for p in paths {
            let z = Zip::open(&p)?;
            match (version, z.version_code()) {
                (Some(b), Some(s)) if b != s => {
                    skipped.push((p, format!("versionCode {s}, the base's is {b}")));
                }
                _ => zips.push(z),
            }
        }
        Ok(Apk {
            zips,
            table: OnceLock::new(),
            skipped,
        })
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.zips
            .iter()
            .flat_map(|z| z.entries.keys().map(String::as_str))
    }

    /// An entry, from the first archive that has it.
    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        self.zips.iter().find_map(|z| z.read(name))
    }

    /// Each archive's path and its split name (`None` for the base), from
    /// its manifest.
    pub fn splits(&self) -> Vec<(PathBuf, Option<String>)> {
        self.zips
            .iter()
            .map(|z| {
                let split = z
                    .manifest()
                    .and_then(|m| m.attr("split").map(str::to_string));
                (z.path.clone(), split)
            })
            .collect()
    }

    /// The base manifest's `requiredSplitTypes`, such as `base__density`.
    pub fn required_splits(&self) -> Vec<String> {
        self.zips
            .first()
            .and_then(Zip::manifest)
            .and_then(|m| m.attr("requiredSplitTypes").map(str::to_string))
            .map(|s| s.split(',').map(str::to_string).collect())
            .unwrap_or_default()
    }

    /// Every archive's `resources.arsc`, merged.
    pub fn table(&self) -> Option<&Table> {
        self.table
            .get_or_init(|| {
                let arscs: Vec<Vec<u8>> = self
                    .zips
                    .iter()
                    .filter_map(|z| z.read("resources.arsc"))
                    .collect();
                let aliases = self
                    .read("assets/phoenix_fat_client_aliases.txt")
                    .map(|b| String::from_utf8_lossy(&b).into_owned());
                match Table::parse(&arscs, aliases.as_deref()) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        crate::guest::log(&format!("[res] resources.arsc: {e:#}"));
                        None
                    }
                }
            })
            .as_ref()
    }

    /// The bundled fonts: gg sans and the Ginto faces.
    pub fn fonts(&self) -> Vec<Vec<u8>> {
        let mut names: Vec<&str> = self
            .names()
            .filter(|n| {
                n.starts_with("assets/fonts/")
                    && Path::new(n)
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("ttf"))
                    && (n.contains("ggsans") || n.contains("Ginto") || n.contains("ggmono"))
            })
            .collect();
        names.sort_unstable();
        names.dedup();
        names.into_iter().filter_map(|n| self.read(n)).collect()
    }
}
