//! Panic locations: the `(file, line, column)` records rustc emits for every
//! `#[track_caller]` call, index expression and panic in the binary.
//!
//! A `core::panic::Location` is `{ file: &str, line: u32, col: u32 }`, 24
//! bytes in a data section: a pointer to the file name, its length, then the
//! line and column. They survive release builds, fat LTO and symbol
//! stripping, since the panic machinery needs them at run time.
//!
//! A PE and a non-PIE ELF store the pointer as an absolute address. A PIE
//! ELF or a shared object stores 0 there and keeps the pointer in a relative
//! relocation, read through [`crate::relocs`].

use std::collections::BTreeSet;

use object::{Object, ObjectSection, SectionKind};

/// One `core::panic::Location` record.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PanicLocation {
    /// The source path as the build saw it, e.g.
    /// `C:\Users\..\zed-a70e2ad075855582\c84af5b\crates\gpui\src\window.rs`.
    pub file: String,
    pub line: u32,
    /// 1-based, in characters.
    pub col: u32,
}

/// Every distinct panic location in `bytes`, sorted. Empty if the container
/// can't be parsed.
pub fn extract(bytes: &[u8]) -> Vec<PanicLocation> {
    let out: BTreeSet<PanicLocation> = records(bytes).into_iter().map(|(_, l)| l).collect();
    out.into_iter().collect()
}

/// Every panic location record in `bytes` with its address: the code that
/// panics there loads that address. Empty if the container can't be parsed.
pub fn records(bytes: &[u8]) -> Vec<(u64, PanicLocation)> {
    let Ok(obj) = object::File::parse(bytes) else {
        return Vec::new();
    };
    // Address-sorted (address, contents) of every section with file contents,
    // to follow a pointer to its string.
    let mut mapped: Vec<(u64, &[u8])> = obj
        .sections()
        .filter_map(|s| Some((s.address(), s.data().ok()?)))
        .filter(|(_, d)| !d.is_empty())
        .collect();
    mapped.sort_by_key(|(a, _)| *a);
    let relative = crate::relocs::relative(&obj);
    let target = |va: u64, len: usize| -> Option<&[u8]> {
        let i = mapped.partition_point(|(a, _)| *a <= va).checked_sub(1)?;
        let (start, data) = mapped[i];
        let off = usize::try_from(va - start).ok()?;
        data.get(off..off.checked_add(len)?)
    };

    let mut out = Vec::new();
    for sec in obj.sections() {
        if !matches!(
            sec.kind(),
            SectionKind::ReadOnlyData | SectionKind::Data | SectionKind::ReadOnlyDataWithRel
        ) {
            continue;
        }
        let Ok(data) = sec.data() else { continue };
        for (i, rec) in data.as_chunks::<8>().0.windows(3).enumerate() {
            let va = sec.address() + 8 * i as u64;
            let stored = u64::from_le_bytes(rec[0]);
            let ptr = relative.get(&va).copied().unwrap_or(stored);
            let len = u64::from_le_bytes(rec[1]);
            if !(4..4096).contains(&len) {
                continue;
            }
            let Some(name) = target(ptr, len as usize) else {
                continue;
            };
            if !name.ends_with(b".rs") || name.contains(&0) {
                continue;
            }
            let Ok(file) = std::str::from_utf8(name) else {
                continue;
            };
            // The line, then the column: two u32s in one little-endian word.
            let word = u64::from_le_bytes(rec[2]);
            let (line, col) = (word as u32, (word >> 32) as u32);
            if (1..1_000_000).contains(&line) && (1..10_000).contains(&col) {
                out.push((
                    va,
                    PanicLocation {
                        file: file.to_string(),
                        line,
                        col,
                    },
                ));
            }
        }
    }
    out
}

/// Whether column `col` of the source line `text` is where a panic location
/// points: a call (`.unwrap()`, `.borrow_mut()`, any `#[track_caller]` fn),
/// a macro (`panic!`, `assert!`), an index expression (`[`, or `self` where
/// the indexed place starts with it), or an async block or closure
/// (`async`, `move`, `|`, `{`). In a different revision of the file the same
/// coordinates usually land somewhere else.
pub fn is_site(text: &str, col: u32) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let Some(i) = (col as usize).checked_sub(1) else {
        return false;
    };
    let Some(&c) = chars.get(i) else { return false };
    if matches!(c, '[' | '{' | '|') {
        return true;
    }
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    if !(c.is_alphabetic() || c == '_') || (i > 0 && ident(chars[i - 1])) {
        return false;
    }
    let mut j = i;
    while j < chars.len() && ident(chars[j]) {
        j += 1;
    }
    let word: String = chars[i..j].iter().collect();
    if matches!(word.as_str(), "async" | "move" | "self") {
        return true;
    }
    // A turbofish: `spawn::<T>(`.
    if chars[j..].starts_with(&[':', ':', '<']) {
        let mut depth = 0;
        while j < chars.len() {
            match chars[j] {
                '<' => depth += 1,
                '>' => {
                    depth -= 1;
                    if depth == 0 {
                        j += 1;
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
    }
    matches!(chars.get(j), Some('(' | '!'))
}

#[cfg(test)]
mod tests {
    use super::is_site;

    #[test]
    fn sites() {
        let spawn = "    cx.spawn::<u8>(async move |cx| {});";
        assert!(is_site("    let a = self.cell.borrow_mut();", 23));
        assert!(is_site("    x.nodes[i].kind;", 12));
        assert!(is_site("    self.indices[n] = 1;", 5));
        assert!(is_site("    panic!(\"no\");", 5));
        assert!(is_site(spawn, 8));
        assert!(is_site(spawn, 20));
        assert!(!is_site("    let a = self.cell.borrow_mut();", 25)); // inside the name
        assert!(!is_site("    let b = 1;", 9)); // a binding, not a call
        assert!(!is_site("    let b = 1;", 1)); // indentation
        assert!(!is_site("    x.nodes[i].kind;", 5)); // a place that isn't `self`
        assert!(!is_site("}", 4)); // past the end
        assert!(!is_site("}", 0)); // columns start at 1
    }
}
