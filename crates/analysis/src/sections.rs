//! Map file offsets to the binary section they fall in.
//!
//! A recovered string is only useful as a durable *anchor* if we know where it
//! lives: `.rodata`/`.rdata` holds the read-only constants (panic paths, format
//! strings) we key on, and an offset into a named section survives far better
//! across rebuilds than a raw file offset does. This wraps the `object` crate
//! so the rest of the tool can ask "what section is byte N in?" without caring
//! whether the container is ELF, PE, or Mach-O.

use anyhow::Result;
use object::{Object, ObjectSection};
use serde::Serialize;

/// One section's placement in the file, in ascending `file_start` order.
#[derive(Debug, Clone, Serialize)]
pub struct SectionSpan {
    pub name: String,
    /// Byte offset of the section's data in the file.
    pub file_start: u64,
    /// Length of that data in the file (0 for `.bss`-like sections).
    pub file_len: u64,
}

#[derive(Debug, Default)]
pub struct SectionMap {
    spans: Vec<SectionSpan>,
}

impl SectionMap {
    /// Parse the container and record every section that occupies file bytes.
    /// Returns an empty map (not an error) for formats `object` can't read, so
    /// section attribution degrades to "unknown" rather than failing the run.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let Ok(file) = object::File::parse(bytes) else {
            return Ok(Self::default());
        };
        let mut spans: Vec<SectionSpan> = file
            .sections()
            .filter_map(|s| {
                let (file_start, file_len) = s.file_range()?;
                let name = s.name().unwrap_or("<unnamed>").to_string();
                Some(SectionSpan {
                    name,
                    file_start,
                    file_len,
                })
            })
            .collect();
        spans.sort_by_key(|s| s.file_start);
        Ok(Self { spans })
    }

    #[cfg(test)]
    fn from_spans(mut spans: Vec<SectionSpan>) -> Self {
        spans.sort_by_key(|s| s.file_start);
        Self { spans }
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    pub fn spans(&self) -> &[SectionSpan] {
        &self.spans
    }

    /// The section containing file byte `offset`, if any. Linear over a handful
    /// of sections; callers resolve a bounded number of anchors, not every
    /// string, so this stays cheap without an interval tree.
    pub fn locate(&self, offset: usize) -> Option<&str> {
        let off = offset as u64;
        self.spans
            .iter()
            .find(|s| off >= s.file_start && off < s.file_start + s.file_len)
            .map(|s| s.name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(name: &str, start: u64, len: u64) -> SectionSpan {
        SectionSpan {
            name: name.into(),
            file_start: start,
            file_len: len,
        }
    }

    #[test]
    fn locates_offset_in_the_right_section() {
        let m = SectionMap::from_spans(vec![
            span(".text", 0x1000, 0x2000),
            span(".rodata", 0x3000, 0x1000),
        ]);
        assert_eq!(m.locate(0x1000), Some(".text"));
        assert_eq!(m.locate(0x2fff), Some(".text"));
        assert_eq!(m.locate(0x3000), Some(".rodata"));
        assert_eq!(m.locate(0x3fff), Some(".rodata"));
    }

    #[test]
    fn offsets_outside_every_section_are_unknown() {
        let m = SectionMap::from_spans(vec![span(".rodata", 0x3000, 0x1000)]);
        assert_eq!(m.locate(0x2fff), None); // just before
        assert_eq!(m.locate(0x4000), None); // just past the end (exclusive)
    }

    #[test]
    fn empty_map_locates_nothing() {
        let m = SectionMap::default();
        assert!(m.is_empty());
        assert_eq!(m.locate(0), None);
    }
}
