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
use object::{Object, ObjectSymbol, ObjectSymbolTable, RelocationFlags, RelocationTarget};

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

#[cfg(test)]
mod tests {
    use super::*;

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
