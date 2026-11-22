//! Turn a compiled object (or an `.rlib` of them) into per-function signatures.
//!
//! We compile a candidate crate ourselves, so its objects keep their symbols
//! and, crucially, their relocation tables. A relocation marks the exact
//! bytes the linker will overwrite with a final address (a call target, a data
//! or static reference). Those are the only bytes that can differ between our
//! object and the same code linked into the target, so we wildcard precisely
//! them and keep everything else fixed. No disassembly or heuristics: the
//! toolchain tells us what is address-dependent.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use object::read::archive::ArchiveFile;
use object::{Object, ObjectSection, ObjectSymbol, SymbolKind};

use crate::sig::Signature;

/// One function's signature, from a relocatable object.
#[derive(Debug, Clone)]
pub struct FnSig {
    /// Mangled symbol name of the function in our reference object.
    pub symbol: String,
    pub sig: Signature,
}

/// Signatures for every sized function symbol in one relocatable object.
/// `min_significant` drops signatures with too few fixed bytes to be specific.
pub fn from_object(bytes: &[u8], min_significant: usize) -> Result<Vec<FnSig>> {
    let obj = object::File::parse(bytes).context("parsing object")?;
    // COFF (the objects an MSVC-target rustc emits) records no symbol size, so
    // `Symbol::size()` is 0 there. Recover each function's extent from the next
    // text-symbol boundary in the same section, which also handles the common
    // one-function-per-COMDAT-section layout (next boundary = section end).
    let mut bounds: BTreeMap<usize, Vec<u64>> = BTreeMap::new();
    for sym in obj.symbols() {
        if sym.kind() == SymbolKind::Text
            && let Some(idx) = sym.section_index()
        {
            bounds.entry(idx.0).or_default().push(sym.address());
        }
    }
    for v in bounds.values_mut() {
        v.sort_unstable();
        v.dedup();
    }
    // Each section's relocated fields as (offset, width), sorted by offset.
    let mut relocs: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    for section in obj.sections() {
        let mut fields: Vec<(usize, usize)> = section
            .relocations()
            .map(|(off, reloc)| {
                // `size()` is in bits and 0 when the format doesn't record
                // it; the dominant case on x86-64 (PC-relative call/lea) is
                // a 4-byte field, so fall back to that.
                let width = match reloc.size() {
                    0 => 4,
                    bits => (bits as usize).div_ceil(8),
                };
                (off as usize, width)
            })
            .collect();
        fields.sort_unstable();
        relocs.insert(section.index().0, fields);
    }

    let mut out = Vec::new();

    for sym in obj.symbols() {
        if sym.kind() != SymbolKind::Text {
            continue;
        }
        let Some(sec_idx) = sym.section_index() else {
            continue;
        };
        let section = obj.section_by_index(sec_idx)?;
        let data = section.data()?;

        // In a relocatable object, symbol and relocation offsets are relative
        // to the start of their section.
        let start = sym.address() as usize;
        let size = if sym.size() > 0 {
            sym.size() as usize
        } else {
            // Next distinct text-symbol address in this section, else the end.
            let next = bounds
                .get(&sec_idx.0)
                .and_then(|v| v.iter().find(|&&a| a > sym.address()).copied())
                .map_or(data.len(), |a| a as usize);
            next.saturating_sub(start)
        };
        if size == 0 {
            continue;
        }
        let Some(end) = start.checked_add(size) else {
            continue;
        };
        if end > data.len() {
            continue;
        }

        let mut masked: Vec<Option<u8>> = data[start..end].iter().map(|&b| Some(b)).collect();

        let fields = relocs.get(&sec_idx.0).map_or(&[][..], Vec::as_slice);
        let first = fields.partition_point(|&(off, _)| off < start);
        for &(rel_off, width) in fields[first..].iter().take_while(|&&(off, _)| off < end) {
            let lo = rel_off - start;
            let hi = (lo + width).min(masked.len());
            for m in &mut masked[lo..hi] {
                *m = None;
            }
        }

        let sig = Signature::new(masked);
        if sig.is_empty() || sig.significant() < min_significant {
            continue;
        }
        let symbol = sym.name().unwrap_or("<unnamed>").to_string();
        out.push(FnSig { symbol, sig });
    }

    Ok(out)
}

/// Signatures from an `.rlib` (an `ar` archive of objects plus metadata).
/// Members that aren't parseable objects (notably `lib.rmeta`) are skipped.
pub fn from_rlib(bytes: &[u8], min_significant: usize) -> Result<Vec<FnSig>> {
    let archive = ArchiveFile::parse(bytes).context("parsing rlib archive")?;
    let mut out = Vec::new();
    for member in archive.members() {
        let member = member?;
        let data = member.data(bytes)?;
        // A member that isn't an object file (lib.rmeta) has no functions.
        if let Ok(sigs) = from_object(data, min_significant) {
            out.extend(sigs);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use object::write;
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationEncoding, RelocationFlags,
        RelocationKind, SymbolFlags, SymbolScope,
    };

    /// Build a tiny relocatable ELF with one 8-byte function whose bytes 3..7
    /// are covered by a 32-bit PC-relative relocation, so extraction must
    /// wildcard exactly those four bytes and keep the other four fixed.
    fn object_with_one_reloc() -> Vec<u8> {
        let mut obj =
            write::Object::new(BinaryFormat::Elf, Architecture::X86_64, Endianness::Little);
        let text = obj.section_id(write::StandardSection::Text);
        let code = [0x55u8, 0x48, 0xe8, 0x00, 0x00, 0x00, 0x00, 0xc3];
        let off = obj.append_section_data(text, &code, 1);

        obj.add_symbol(write::Symbol {
            name: b"my_func".to_vec(),
            value: off,
            size: code.len() as u64,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: write::SymbolSection::Section(text),
            flags: SymbolFlags::None,
        });

        // A target symbol the call refers to; undefined is fine for this test.
        let callee = obj.add_symbol(write::Symbol {
            name: b"callee".to_vec(),
            value: 0,
            size: 0,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: write::SymbolSection::Undefined,
            flags: SymbolFlags::None,
        });
        obj.add_relocation(
            text,
            write::Relocation {
                offset: off + 3,
                symbol: callee,
                addend: -4,
                flags: RelocationFlags::Generic {
                    kind: RelocationKind::Relative,
                    encoding: RelocationEncoding::Generic,
                    size: 32,
                },
            },
        )
        .unwrap();

        obj.write().unwrap()
    }

    #[test]
    fn masks_relocation_bytes_only() {
        let bytes = object_with_one_reloc();
        let sigs = from_object(&bytes, 1).unwrap();
        let f = sigs
            .iter()
            .find(|f| f.symbol == "my_func")
            .expect("my_func present");
        // 0x55 0x48 0xe8 ?? ?? ?? ?? 0xc3
        assert_eq!(f.sig.to_pattern(), "55 48 e8 ?? ?? ?? ?? c3");
        assert_eq!(f.sig.significant(), 4);
    }

    #[test]
    fn min_significant_filters() {
        let bytes = object_with_one_reloc();
        // The only function has 4 fixed bytes; asking for 5 drops it.
        assert!(from_object(&bytes, 5).unwrap().is_empty());
    }

    /// Two functions packed in one `.text` with no recorded symbol size (the
    /// COFF case) must have their extents recovered from each other's address.
    #[test]
    fn derives_size_from_next_boundary() {
        let mut obj =
            write::Object::new(BinaryFormat::Elf, Architecture::X86_64, Endianness::Little);
        let text = obj.section_id(write::StandardSection::Text);
        // first: 4 bytes, second: 3 bytes.
        let code = [0x11u8, 0x22, 0x33, 0x44, 0xaa, 0xbb, 0xc3];
        let base = obj.append_section_data(text, &code, 1);
        for (name, at) in [
            (b"first".as_slice(), base),
            (b"second".as_slice(), base + 4),
        ] {
            obj.add_symbol(write::Symbol {
                name: name.to_vec(),
                value: at,
                size: 0, // force boundary derivation
                kind: SymbolKind::Text,
                scope: SymbolScope::Linkage,
                weak: false,
                section: write::SymbolSection::Section(text),
                flags: SymbolFlags::None,
            });
        }
        let bytes = obj.write().unwrap();
        let sigs = from_object(&bytes, 1).unwrap();
        let first = sigs.iter().find(|f| f.symbol == "first").unwrap();
        let second = sigs.iter().find(|f| f.symbol == "second").unwrap();
        assert_eq!(first.sig.to_pattern(), "11 22 33 44");
        assert_eq!(second.sig.to_pattern(), "aa bb c3");
    }
}
