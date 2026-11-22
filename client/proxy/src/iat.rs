//! The client image's import address table: finding the slots of one
//! imported function, and pointing a slot somewhere else.

use std::ffi::{CStr, c_char};

use crate::sys;

/// The base address of the process's exe image.
pub(crate) fn exe_base() -> Option<usize> {
    // SAFETY: a null name asks for the exe's own module, and touches no memory.
    let base = unsafe { sys::GetModuleHandleW(std::ptr::null()) } as usize;
    (base != 0).then_some(base)
}

/// The import address table slots in the image at `base` for `dll!name`:
/// one per import descriptor naming that DLL. By-name imports only.
///
/// # Safety
///
/// `base` is the base address of a PE32+ image the loader has mapped.
pub(crate) unsafe fn slots(base: usize, dll: &str, name: &str) -> Vec<*mut usize> {
    let mut out = Vec::new();
    // SAFETY: `e_lfanew`, the offset of the NT headers, is at 0x3c of the
    // DOS header, at `base`.
    let nt = base + unsafe { u32_at(base + 0x3c) };
    // SAFETY: the import directory's RVA is data directory 1 of the PE32+
    // optional header, which follows the 24-byte file header.
    let imports = unsafe { u32_at(nt + 24 + 112 + 8) };
    if imports == 0 {
        return out;
    }
    let mut desc = base + imports;
    loop {
        // SAFETY: the import directory is an array of 20-byte descriptors,
        // ended by one whose name is zero; `desc` is one of them.
        let (lookup, dll_name, iat) =
            unsafe { (u32_at(desc), u32_at(desc + 12), u32_at(desc + 16)) };
        if dll_name == 0 {
            break;
        }
        // SAFETY: a descriptor's name is the RVA of a NUL-terminated string.
        let this_dll = unsafe { c_str(base + dll_name) };
        if this_dll.eq_ignore_ascii_case(dll) {
            let names = base + if lookup != 0 { lookup } else { iat };
            for i in 0.. {
                // SAFETY: the lookup table is an array of 8-byte entries,
                // ended by a zero one.
                let entry = unsafe { ((names + 8 * i) as *const u64).read_unaligned() };
                if entry == 0 {
                    break;
                }
                // The high bit marks an import by ordinal, which has no name.
                if entry & (1 << 63) != 0 {
                    continue;
                }
                // SAFETY: a by-name entry is the RVA of a 2-byte hint and
                // then the NUL-terminated name.
                let import = unsafe { c_str(base + entry as usize + 2) };
                if import == name {
                    out.push((base + iat + 8 * i) as *mut usize);
                }
            }
        }
        desc += 20;
    }
    out
}

/// Point the import slot `slot` at `value`.
///
/// # Safety
///
/// `slot` is an import address table slot of a mapped image, as [`slots`]
/// returns them.
pub(crate) unsafe fn write(slot: *mut usize, value: usize) -> Result<(), &'static str> {
    let mut old = 0u32;
    // SAFETY: `slot` is 8 bytes of the image, and `old` receives the
    // protection they had.
    let unprotected =
        unsafe { sys::VirtualProtect(slot.cast(), 8, sys::PAGE_READWRITE, &raw mut old) };
    if unprotected == 0 {
        return Err("VirtualProtect failed on an import slot");
    }
    // SAFETY: the slot is an aligned pointer-sized entry, writable now.
    unsafe { slot.write(value) };
    // SAFETY: as above; this restores the slot's protection. If it fails, the
    // slot stays writable, which nothing relies on.
    unsafe { sys::VirtualProtect(slot.cast(), 8, old, &raw mut old) };
    Ok(())
}

/// The `u32` at `at`, widened.
///
/// # Safety
///
/// `at` is readable for 4 bytes.
unsafe fn u32_at(at: usize) -> usize {
    // SAFETY: the caller's contract.
    unsafe { (at as *const u32).read_unaligned() as usize }
}

/// The NUL-terminated string at `at`, lossily as UTF-8.
///
/// # Safety
///
/// `at` points at a NUL-terminated string.
unsafe fn c_str(at: usize) -> String {
    // SAFETY: the caller's contract.
    unsafe { CStr::from_ptr(at as *const c_char) }
        .to_string_lossy()
        .into_owned()
}
