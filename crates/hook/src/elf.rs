//! Where an ELF image keeps the pointers a slot swap replaces: the GOT slot
//! of an imported function, and the live address of any slot.
//!
//! Addresses here are the image's own virtual addresses (its `p_vaddr`s),
//! which an Android or Linux shared object links at base 0. A slot in the
//! loaded image is at its load bias plus that address.

use anyhow::{Context, Result};
use object::elf::{
    R_AARCH64_GLOB_DAT, R_AARCH64_JUMP_SLOT, R_X86_64_GLOB_DAT, R_X86_64_JUMP_SLOT, RelocationType,
};
use object::{
    Object, ObjectSegment, ObjectSymbol, ObjectSymbolTable, RelocationFlags, RelocationTarget,
};

/// The GOT slots the dynamic linker fills with the address of the imported
/// `symbol`: its `JUMP_SLOT` (the PLT's) and `GLOB_DAT` (an address taken)
/// relocations, sorted. Empty if the image doesn't import it.
///
/// Relocations in Android's packed format (`APS2`) are not read. lld packs
/// only `.rela.dyn` there, and keeps `.rela.plt`, which holds every
/// `JUMP_SLOT`, as plain records.
pub fn got_slots(image: &[u8], symbol: &str) -> Result<Vec<u64>> {
    let obj = object::File::parse(image).context("parsing the ELF image")?;
    let mut out = Vec::new();
    let Some(relocations) = obj.dynamic_relocations() else {
        return Ok(out);
    };
    for (slot, r) in relocations {
        let RelocationFlags::Elf { r_type } = r.flags() else {
            continue;
        };
        if !is_got(r_type) {
            continue;
        }
        let RelocationTarget::Symbol(index) = r.target() else {
            continue;
        };
        if obj
            .dynamic_symbol_table()
            .and_then(|t| t.symbol_by_index(index).ok())
            .is_some_and(|s| s.name() == Ok(symbol))
        {
            out.push(slot);
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

fn is_got(r_type: RelocationType) -> bool {
    [
        R_AARCH64_JUMP_SLOT,
        R_AARCH64_GLOB_DAT,
        R_X86_64_JUMP_SLOT,
        R_X86_64_GLOB_DAT,
    ]
    .contains(&r_type)
}

/// The live address of the slot at the image's virtual address `vaddr`,
/// with the image mapped at `load_bias` (the address its `p_vaddr` 0 is
/// mapped at). `None` if it overflows.
pub fn slot_address(load_bias: usize, vaddr: u64) -> Option<usize> {
    load_bias.checked_add(usize::try_from(vaddr).ok()?)
}

/// The virtual address (`p_vaddr`) the byte at file `offset` is loaded at,
/// from the ELF's program headers: the `PT_LOAD` segment whose file range
/// holds the offset decides it. `None` if no loaded segment covers the
/// offset, or the image doesn't parse. The anchor sets give file offsets;
/// an image whose segments sit at their file offsets (as Android's host
/// does) maps each to the same number, but this reads the headers rather
/// than assume it.
pub fn offset_to_vaddr(image: &[u8], offset: u64) -> Option<u64> {
    let obj = object::File::parse(image).ok()?;
    for seg in obj.segments() {
        let (start, len) = seg.file_range();
        if offset >= start && offset < start + len {
            return (seg.address()).checked_add(offset - start);
        }
    }
    None
}

/// The GNU build ID (`NT_GNU_BUILD_ID`) in `notes`, the bytes of a
/// `PT_NOTE` segment. `None` if the segment holds no build ID.
///
/// The linker hashes the image into it, so two images with the same ID are
/// the same build. A patch the dynamic linker never sees (code rewritten in
/// place) keeps the ID.
pub fn gnu_build_id(notes: &[u8]) -> Option<&[u8]> {
    const NT_GNU_BUILD_ID: u32 = 3;
    let word = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(notes.get(at..at + 4)?.try_into().ok()?))
    };
    let pad = |n: usize| n.checked_add(3).map(|n| n & !3);
    let mut at = 0;
    while at + 12 <= notes.len() {
        let (namesz, descsz, kind) = (word(at)? as usize, word(at + 4)? as usize, word(at + 8)?);
        let name = at + 12;
        let desc = name.checked_add(pad(namesz)?)?;
        let end = desc.checked_add(pad(descsz)?)?;
        if kind == NT_GNU_BUILD_ID && notes.get(name..name + namesz)? == b"GNU\0" {
            return notes.get(desc..desc + descsz);
        }
        at = end;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(name: &[u8], kind: u32, desc: &[u8]) -> Vec<u8> {
        let mut n = Vec::new();
        n.extend_from_slice(&(name.len() as u32).to_le_bytes());
        n.extend_from_slice(&(desc.len() as u32).to_le_bytes());
        n.extend_from_slice(&kind.to_le_bytes());
        for part in [name, desc] {
            n.extend_from_slice(part);
            n.resize(n.len().next_multiple_of(4), 0);
        }
        n
    }

    #[test]
    fn the_build_id_is_read_past_other_notes() {
        let id = [0x9c, 0x88, 0xd1, 0x2e, 0xbb, 0x61, 0x90];
        let mut notes = note(b"Android\0", 1, &[0x18, 0, 0, 0, b'r', b'2']);
        notes.extend(note(b"GNU\0", 3, &id));
        assert_eq!(gnu_build_id(&notes), Some(&id[..]));
        // A note of another owner with the same type is not a build ID.
        assert_eq!(gnu_build_id(&note(b"Go\0\0", 3, &id)), None);
        assert_eq!(gnu_build_id(&notes[..notes.len() - 4]), None);
        assert_eq!(gnu_build_id(&[]), None);
    }

    #[test]
    fn slot_addresses_add_the_load_bias() {
        assert_eq!(
            slot_address(0x7000_0000_0000, 0x60efb98),
            Some(0x7000_060e_fb98)
        );
        assert_eq!(slot_address(usize::MAX, 8), None);
    }

    #[test]
    fn a_non_elf_image_is_an_error() {
        assert!(got_slots(b"MZ not an elf", "malloc").is_err());
    }

    /// On Linux a section's file offset maps to its own virtual address.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_file_offset_maps_to_the_vaddr_of_its_segment() {
        use object::ObjectSection;
        let exe = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let obj = object::File::parse(&*exe).unwrap();
        let text = obj.section_by_name(".text").unwrap();
        let (off, _) = text.file_range().unwrap();
        assert_eq!(offset_to_vaddr(&exe, off), Some(text.address()));
        assert_eq!(offset_to_vaddr(b"MZ not an elf", 0), None);
    }

    /// The test's own executable on Linux imports libc's `malloc` through
    /// its GOT.
    #[cfg(target_os = "linux")]
    #[test]
    fn this_executable_has_a_got_slot_for_an_import() {
        let exe = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        assert!(!got_slots(&exe, "malloc").unwrap().is_empty());
        assert!(got_slots(&exe, "no such import").unwrap().is_empty());
    }
}
