//! A read-only view of the client image, by virtual address.
//!
//! Everything the resolver does is reading the file the client's loader
//! mapped: section bytes, `.pdata` function extents, and RIP-relative
//! references. Nothing here calls into the client.

use anyhow::{Context, Result, bail};
use rekindle_sigmatch::short::PeImage;

/// The image as a file: sections plus the function table.
pub struct Image<'a> {
    pe: PeImage<'a>,
    bytes: &'a [u8],
}

impl<'a> Image<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        let pe = PeImage::parse(bytes).context("parse the client image")?;
        Ok(Self { pe, bytes })
    }

    /// An image in its mapped layout, such as a live module, whose RVA 0 is
    /// at `base`. Its addresses are the live ones.
    pub fn mapped(bytes: &'a [u8], base: u64) -> Result<Self> {
        let mut pe = PeImage::parse(bytes).context("parse the mapped image")?;
        pe.image_base = base;
        for s in &mut pe.sections {
            s.raw_offset = s.rva;
            s.raw_size = s.virtual_size;
        }
        // The function table was read through the file's offsets.
        let le = |o: usize| {
            bytes
                .get(o..o + 4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        };
        let nt = le(0x3c).context("no PE header")? as usize;
        let dir = nt + 24 + 112 + 3 * 8;
        let (rva, size) = (
            le(dir).unwrap_or(0) as usize,
            le(dir + 4).unwrap_or(0) as usize,
        );
        pe.functions = (0..size / 12)
            .filter_map(|i| Some((le(rva + i * 12)?, le(rva + i * 12 + 4)?)))
            .filter(|&(b, e)| b != 0 && e > b)
            .collect();
        pe.functions.sort_unstable();
        pe.functions.dedup();
        Ok(Self { pe, bytes })
    }

    #[must_use]
    pub fn base(&self) -> u64 {
        self.pe.image_base
    }

    #[must_use]
    pub fn entry(&self, rva: u32) -> u64 {
        self.pe.image_base + u64::from(rva)
    }

    /// The bytes at a virtual address, if the whole range is mapped.
    #[must_use]
    pub fn read(&self, va: u64, len: usize) -> Option<&'a [u8]> {
        let rva = u32::try_from(va.checked_sub(self.pe.image_base)?).ok()?;
        let off = self.pe.rva_to_offset(rva)?;
        self.bytes.get(off..off + len)
    }

    /// The function extent containing `va`, as `(start, end)` virtual
    /// addresses.
    #[must_use]
    pub fn function(&self, va: u64) -> Option<(u64, u64)> {
        let rva = u32::try_from(va.checked_sub(self.pe.image_base)?).ok()?;
        let index = self
            .pe
            .functions
            .binary_search_by(|(start, _)| start.cmp(&rva))
            .unwrap_or_else(|i| i.saturating_sub(1));
        let &(start, end) = self.pe.functions.get(index)?;
        (start <= rva && rva < end).then(|| {
            (
                self.pe.image_base + u64::from(start),
                self.pe.image_base + u64::from(end),
            )
        })
    }

    /// The function extent containing `va`, or an error naming it.
    pub fn function_named(&self, what: &str, va: u64) -> Result<(u64, u64)> {
        self.function(va)
            .with_context(|| format!("{what} at {va:#x} is outside the function table"))
    }

    /// The executable sections, as `(start, end)` virtual addresses.
    #[must_use]
    pub fn text(&self) -> Vec<(u64, u64)> {
        self.pe
            .sections
            .iter()
            .filter(|s| s.executable)
            .map(|s| {
                (
                    self.pe.image_base + u64::from(s.rva),
                    self.pe.image_base + u64::from(s.rva) + u64::from(s.virtual_size),
                )
            })
            .collect()
    }

    /// The non-executable, initialized data sections.
    #[must_use]
    pub fn data(&self) -> Vec<(u64, u64)> {
        self.pe
            .sections
            .iter()
            .filter(|s| !s.executable && s.name != ".reloc" && s.virtual_size > 0)
            .map(|s| {
                (
                    self.pe.image_base + u64::from(s.rva),
                    self.pe.image_base + u64::from(s.rva) + u64::from(s.virtual_size),
                )
            })
            .collect()
    }

    /// Every VA where `needle` starts in an initialized section.
    #[must_use]
    pub fn find(&self, needle: &[u8]) -> Vec<u64> {
        let mut out = Vec::new();
        for section in &self.pe.sections {
            let size = match section.virtual_size {
                0 => section.raw_size,
                v => section.raw_size.min(v),
            } as usize;
            let start = section.raw_offset as usize;
            let Some(bytes) = self.bytes.get(start..start + size) else {
                continue;
            };
            let mut pos = 0;
            while let Some(at) = find_from(bytes, needle, pos) {
                out.push(self.pe.image_base + u64::from(section.rva) + at as u64);
                pos = at + 1;
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// A NUL-terminated UTF-8 string at `va`, if printable.
    #[must_use]
    pub fn string(&self, va: u64, max: usize) -> Option<&'a str> {
        let bytes = self.read(va, max)?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        let text = std::str::from_utf8(&bytes[..end]).ok()?;
        text.bytes()
            .all(|b| (0x20..0x7f).contains(&b))
            .then_some(text)
    }

    /// Every `lea r64, [rip + disp]` in executable code whose target lies in
    /// `[lo, hi)`. The eight REX.W LEA encodings are scanned directly; each
    /// is exactly seven bytes.
    #[must_use]
    pub fn leas_to(&self, lo: u64, hi: u64) -> Vec<u64> {
        const MODRM: [u8; 8] = [0x05, 0x0d, 0x15, 0x1d, 0x25, 0x2d, 0x35, 0x3d];
        let mut out = Vec::new();
        for (start, end) in self.text() {
            let len = (end - start) as usize;
            let Some(code) = self.read(start, len) else {
                continue;
            };
            let mut at = 0;
            while at + 7 <= code.len() {
                if (code[at] == 0x48 || code[at] == 0x4c)
                    && code[at + 1] == 0x8d
                    && MODRM.contains(&code[at + 2])
                {
                    let disp = i32::from_le_bytes(code[at + 3..at + 7].try_into().unwrap());
                    let target = (start + at as u64 + 7).wrapping_add_signed(i64::from(disp));
                    if (lo..hi).contains(&target) {
                        out.push(start + at as u64);
                    }
                }
                at += 1;
            }
        }
        out.sort_unstable();
        out
    }

    /// Every `lea r64, [rip + disp]` whose target is one of `targets`, as
    /// `(lea, target)`, from one pass over the code.
    #[must_use]
    pub fn leas_into(&self, targets: &[u64]) -> Vec<(u64, u64)> {
        const MODRM: [u8; 8] = [0x05, 0x0d, 0x15, 0x1d, 0x25, 0x2d, 0x35, 0x3d];
        let mut sorted = targets.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let mut out = Vec::new();
        for (start, end) in self.text() {
            let Some(code) = self.read(start, (end - start) as usize) else {
                continue;
            };
            for at in 0..code.len().saturating_sub(6) {
                if (code[at] == 0x48 || code[at] == 0x4c)
                    && code[at + 1] == 0x8d
                    && MODRM.contains(&code[at + 2])
                {
                    let disp = i32::from_le_bytes(code[at + 3..at + 7].try_into().unwrap());
                    let target = (start + at as u64 + 7).wrapping_add_signed(i64::from(disp));
                    if sorted.binary_search(&target).is_ok() {
                        out.push((start + at as u64, target));
                    }
                }
            }
        }
        out
    }

    /// Every direct `call rel32` to one of `targets`, as `(site, target)`,
    /// from one pass over the code.
    #[must_use]
    pub fn calls_into(&self, targets: &[u64]) -> Vec<(u64, u64)> {
        let mut sorted = targets.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let mut out = Vec::new();
        for (start, end) in self.text() {
            let Some(code) = self.read(start, (end - start) as usize) else {
                continue;
            };
            for at in 0..code.len().saturating_sub(4) {
                if code[at] != 0xe8 {
                    continue;
                }
                let rel = i32::from_le_bytes(code[at + 1..at + 5].try_into().unwrap());
                let target = (start + at as u64 + 5).wrapping_add_signed(i64::from(rel));
                if sorted.binary_search(&target).is_ok() {
                    out.push((start + at as u64, target));
                }
            }
        }
        out
    }

    /// A qword at `va`.
    #[must_use]
    pub fn qword(&self, va: u64) -> Option<u64> {
        Some(u64::from_le_bytes(self.read(va, 8)?.try_into().ok()?))
    }

    /// A u32 at `va`.
    #[must_use]
    pub fn dword(&self, va: u64) -> Option<u32> {
        Some(u32::from_le_bytes(self.read(va, 4)?.try_into().ok()?))
    }

    /// The section name containing `va`, for reports.
    #[must_use]
    pub fn section(&self, va: u64) -> Option<&str> {
        let rva = u32::try_from(va.checked_sub(self.pe.image_base)?).ok()?;
        self.pe
            .sections
            .iter()
            .find(|s| s.rva <= rva && rva < s.rva + s.virtual_size)
            .map(|s| s.name.as_str())
    }

    /// The `(data_va, end_va)` extents of an aligned pointer table in a data
    /// section: every qword in `[start, end)` that points into executable
    /// code.
    pub fn data_pointers(&self, start: u64, end: u64) -> Result<Vec<(u64, u64)>> {
        let len = usize::try_from(end - start).context("table too large")?;
        let bytes = self
            .read(start, len)
            .with_context(|| format!("table {start:#x}..{end:#x} is not mapped"))?;
        let text = self.text();
        let mut out = Vec::new();
        for (i, chunk) in bytes.chunks_exact(8).enumerate() {
            let value = u64::from_le_bytes(chunk.try_into().unwrap());
            if text.iter().any(|&(lo, hi)| (lo..hi).contains(&value)) {
                out.push((start + i as u64 * 8, value));
            }
        }
        Ok(out)
    }
}

fn find_from(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|at| from + at)
}

/// Fail unless `[values]` are all distinct.
pub fn one_of<T: std::fmt::Debug + Copy>(what: &str, values: &[T]) -> Result<T> {
    match values {
        [one] => Ok(*one),
        [] => bail!("{what}: nothing found"),
        many => bail!("{what}: {} candidates {many:?}", many.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_at_the_start_and_later() {
        let hay = b"abcabc";
        assert_eq!(find_from(hay, b"abc", 0), Some(0));
        assert_eq!(find_from(hay, b"abc", 1), Some(3));
        assert_eq!(find_from(hay, b"zzz", 0), None);
    }
}
