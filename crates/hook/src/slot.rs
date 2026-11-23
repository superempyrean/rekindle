//! Reversible pointer swaps: a vtable method or a GOT entry replaced with a
//! hook, with no code patched.
//!
//! A call through a `dyn Trait` loads the method from the vtable, and a call
//! to an imported function loads it from the GOT. Both slots are
//! pointer-sized words in data that the dynamic linker wrote and then made
//! read-only (`PT_GNU_RELRO`). [`SlotSwap::install`] makes the slot's page
//! writable, stores the hook with one atomic write and puts the page's
//! protection back. A thread that loads the slot meanwhile sees the old
//! pointer or the new one, never half of each.
//!
//! [`SlotSwap`] keeps the pointer it replaced and writes it back when it is
//! dropped, unless something else replaced the hook in the meantime.
//!
//! Slot swaps work on Windows, Linux and Android, on any architecture. The
//! [`elf`](crate::elf) module finds GOT slots in an ELF file.

use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, bail};

/// A pointer in a vtable or GOT slot, replaced with a hook.
#[derive(Debug)]
pub struct SlotSwap {
    slot: usize,
    original: usize,
    replacement: usize,
    restored: bool,
}

impl SlotSwap {
    /// Store `replacement` in the pointer slot at `slot` and keep the
    /// pointer it held.
    ///
    /// Fails, leaving the slot untouched, if `slot` is not pointer-aligned or
    /// its page's protection can't be changed.
    ///
    /// # Safety
    ///
    /// - `slot` is a mapped pointer slot in this process that holds a
    ///   function pointer, and `replacement` is a function with the same ABI.
    /// - The returned [`SlotSwap`] is not restored or dropped while a thread
    ///   may still be inside `replacement`, if that would break it.
    pub unsafe fn install(slot: usize, replacement: usize) -> Result<Self> {
        if !slot.is_multiple_of(align_of::<usize>()) {
            bail!("slot {slot:#x} is not pointer-aligned");
        }
        // SAFETY: the caller guarantees the slot is mapped and holds a
        // pointer, and `write_slot` writes one aligned word.
        let original = unsafe { write_slot(slot, None, replacement)? };
        Ok(Self {
            slot,
            original,
            replacement,
            restored: false,
        })
    }

    /// The slot's address.
    pub fn slot(&self) -> usize {
        self.slot
    }

    /// The pointer the slot held before the swap: the function the hook
    /// calls to pass a call on.
    pub fn original(&self) -> usize {
        self.original
    }

    /// Put the original pointer back. Fails, writing nothing, if the slot no
    /// longer holds the hook: another swap replaced it, and writing the
    /// original over that one would drop it.
    pub fn restore(&mut self) -> Result<()> {
        if self.restored {
            return Ok(());
        }
        // SAFETY: `install` checked the slot, which stays mapped for as long
        // as the image that holds it, and the caller of `install` accepted
        // that the hook may be removed.
        unsafe { write_slot(self.slot, Some(self.replacement), self.original)? };
        self.restored = true;
        Ok(())
    }
}

impl Drop for SlotSwap {
    fn drop(&mut self) {
        // A failure leaves the hook in place, which the drop can't report.
        let _ = self.restore();
    }
}

/// Write `new` into the aligned pointer slot at `slot`, if it holds
/// `expect` (or anything, for `None`), and return what it held. The page is
/// made writable for the write and given its protection back after it.
///
/// # Safety
///
/// `slot` is a mapped, aligned pointer slot of this process.
unsafe fn write_slot(slot: usize, expect: Option<usize>, new: usize) -> Result<usize> {
    let page = sys::page_size();
    let (start, len) = page_span(slot, size_of::<usize>(), page);
    // SAFETY: forwarded from the caller; the span covers the slot.
    let old = unsafe { sys::make_writable(start, len)? };
    // SAFETY: the slot is aligned, mapped and now writable. Every other
    // access to a vtable or GOT slot is a plain aligned load, which an
    // atomic store doesn't tear.
    let cell = unsafe { AtomicUsize::from_ptr(slot as *mut usize) };
    let result = match expect {
        None => Ok(cell.swap(new, Ordering::AcqRel)),
        Some(e) => cell
            .compare_exchange(e, new, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|held| anyhow::anyhow!("slot {slot:#x} holds {held:#x}, not the hook {e:#x}")),
    };
    sys::flush(slot, size_of::<usize>());
    // SAFETY: the pages are the ones made writable above, and `old` is the
    // protection they had.
    unsafe { sys::restore(start, len, old)? };
    result
}

/// The page-aligned span `(start, len)` covering `len` bytes at `addr`.
pub(crate) fn page_span(addr: usize, len: usize, page: usize) -> (usize, usize) {
    let start = addr & !(page - 1);
    let end = (addr + len).next_multiple_of(page);
    (start, end - start)
}

/// A page's protection as `/proc/self/maps` shows it.
#[cfg(any(test, target_os = "linux", target_os = "android"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Prot {
    pub(crate) read: bool,
    pub(crate) write: bool,
    pub(crate) exec: bool,
}

/// The protection of the mapping that holds `addr`, from the text of
/// `/proc/self/maps`.
#[cfg(any(test, target_os = "linux", target_os = "android"))]
pub(crate) fn prot_in_maps(maps: &str, addr: usize) -> Option<Prot> {
    maps.lines().find_map(|line| {
        let mut f = line.split_whitespace();
        let (start, end) = f.next()?.split_once('-')?;
        let (start, end) = (
            usize::from_str_radix(start, 16).ok()?,
            usize::from_str_radix(end, 16).ok()?,
        );
        let perms = f.next()?.as_bytes();
        ((start..end).contains(&addr) && perms.len() >= 3).then(|| Prot {
            read: perms[0] == b'r',
            write: perms[1] == b'w',
            exec: perms[2] == b'x',
        })
    })
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod sys {
    use anyhow::{Context, Result, bail};

    use super::{Prot, prot_in_maps};

    pub(super) fn page_size() -> usize {
        // SAFETY: sysconf reads a constant of the system.
        unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
    }

    fn bits(p: Prot) -> libc::c_int {
        let mut bits = libc::PROT_NONE;
        if p.read {
            bits |= libc::PROT_READ;
        }
        if p.write {
            bits |= libc::PROT_WRITE;
        }
        if p.exec {
            bits |= libc::PROT_EXEC;
        }
        bits
    }

    /// # Safety
    ///
    /// `[start, start + len)` are mapped pages of this process.
    pub(super) unsafe fn make_writable(start: usize, len: usize) -> Result<Prot> {
        // mprotect doesn't return the old protection, so it's read from the
        // mappings to be put back afterwards.
        let maps = std::fs::read_to_string("/proc/self/maps").context("reading /proc/self/maps")?;
        let old =
            prot_in_maps(&maps, start).with_context(|| format!("no mapping holds {start:#x}"))?;
        let rw = Prot {
            read: true,
            write: true,
            ..old
        };
        // SAFETY: forwarded from the caller; adding write access breaks no
        // reader.
        if unsafe { libc::mprotect(start as *mut libc::c_void, len, bits(rw)) } != 0 {
            bail!(
                "mprotect({start:#x}, {len:#x}) failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(old)
    }

    /// # Safety
    ///
    /// As for [`make_writable`], with `old` what it returned.
    pub(super) unsafe fn restore(start: usize, len: usize, old: Prot) -> Result<()> {
        // SAFETY: forwarded from the caller.
        if unsafe { libc::mprotect(start as *mut libc::c_void, len, bits(old)) } != 0 {
            bail!(
                "mprotect({start:#x}, {len:#x}) failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }

    pub(super) fn flush(_addr: usize, _len: usize) {
        // A slot is data: loads see the store without an instruction-cache
        // flush.
    }
}

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;

    use anyhow::{Result, bail};

    #[repr(C)]
    struct SystemInfo {
        processor_architecture: u16,
        reserved: u16,
        page_size: u32,
        rest: [usize; 6],
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn VirtualProtect(address: *mut c_void, size: usize, new: u32, old: *mut u32) -> i32;
        fn GetSystemInfo(info: *mut SystemInfo);
    }

    const PAGE_READWRITE: u32 = 0x04;
    const PAGE_EXECUTE_READWRITE: u32 = 0x40;
    const EXECUTE_ANY: u32 = 0xF0;

    pub(super) fn page_size() -> usize {
        let mut info = std::mem::MaybeUninit::<SystemInfo>::zeroed();
        // SAFETY: GetSystemInfo fills the struct it is given.
        unsafe { GetSystemInfo(info.as_mut_ptr()) };
        // SAFETY: zeroed, then filled by GetSystemInfo.
        unsafe { info.assume_init() }.page_size as usize
    }

    /// # Safety
    ///
    /// `[start, start + len)` are mapped pages of this process.
    pub(super) unsafe fn make_writable(start: usize, len: usize) -> Result<u32> {
        let mut old = 0u32;
        // Query first, so an executable page stays executable.
        // SAFETY: forwarded from the caller; `old` is a valid out pointer.
        if unsafe { VirtualProtect(start as *mut c_void, len, PAGE_READWRITE, &raw mut old) } == 0 {
            bail!("VirtualProtect({start:#x}) failed");
        }
        if old & EXECUTE_ANY != 0 {
            let mut ignored = 0u32;
            // SAFETY: as above.
            unsafe {
                VirtualProtect(
                    start as *mut c_void,
                    len,
                    PAGE_EXECUTE_READWRITE,
                    &raw mut ignored,
                )
            };
        }
        Ok(old)
    }

    /// # Safety
    ///
    /// As for [`make_writable`], with `old` what it returned.
    pub(super) unsafe fn restore(start: usize, len: usize, old: u32) -> Result<()> {
        let mut ignored = 0u32;
        // SAFETY: forwarded from the caller.
        if unsafe { VirtualProtect(start as *mut c_void, len, old, &raw mut ignored) } == 0 {
            bail!("VirtualProtect({start:#x}) failed");
        }
        Ok(())
    }

    pub(super) fn flush(_addr: usize, _len: usize) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_span_covers_a_slot_that_straddles_no_boundary() {
        assert_eq!(page_span(0x60efb98, 8, 0x1000), (0x60ef000, 0x1000));
        assert_eq!(page_span(0x1ffc, 8, 0x1000), (0x1000, 0x2000));
        assert_eq!(page_span(0x4000, 8, 0x4000), (0x4000, 0x4000));
    }

    #[test]
    fn protection_comes_from_the_mapping_that_holds_the_address() {
        let maps = "\
7f0000000000-7f0000001000 r-xp 00000000 fd:01 12 /system/lib64/libc.so
7f0000001000-7f0000003000 r--p 00001000 fd:01 12 /system/lib64/libc.so
7f0000003000-7f0000004000 rw-p 00003000 fd:01 12 /system/lib64/libc.so
";
        let at = |a| prot_in_maps(maps, a);
        let p = |read, write, exec| Some(Prot { read, write, exec });
        assert_eq!(at(0x7f0000000800), p(true, false, true));
        assert_eq!(at(0x7f0000002ff8), p(true, false, false));
        assert_eq!(at(0x7f0000003000), p(true, true, false));
        assert_eq!(at(0x7f0000004000), None);
    }

    extern "C" fn original() -> u32 {
        1
    }

    extern "C" fn hooked() -> u32 {
        2
    }

    /// A vtable-like table in a read-only page of its own.
    struct ReadOnlyTable {
        page: usize,
        len: usize,
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    impl ReadOnlyTable {
        fn new(words: &[usize]) -> Self {
            let len = sys::page_size();
            // SAFETY: a fresh anonymous mapping, written before it's made
            // read-only.
            unsafe {
                let p = libc::mmap(
                    std::ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                );
                assert_ne!(p, libc::MAP_FAILED);
                std::ptr::copy_nonoverlapping(words.as_ptr(), p.cast(), words.len());
                assert_eq!(libc::mprotect(p, len, libc::PROT_READ), 0);
                Self {
                    page: p as usize,
                    len,
                }
            }
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    impl Drop for ReadOnlyTable {
        fn drop(&mut self) {
            // SAFETY: the mapping `new` made.
            unsafe { libc::munmap(self.page as *mut libc::c_void, self.len) };
        }
    }

    #[cfg(windows)]
    impl ReadOnlyTable {
        fn new(words: &[usize]) -> Self {
            use std::ffi::c_void;
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn VirtualAlloc(a: *mut c_void, s: usize, k: u32, p: u32) -> *mut c_void;
                fn VirtualProtect(a: *mut c_void, s: usize, n: u32, o: *mut u32) -> i32;
            }
            let len = sys::page_size();
            // SAFETY: a fresh allocation, written before it's made read-only.
            unsafe {
                let p = VirtualAlloc(std::ptr::null_mut(), len, 0x3000, 0x04);
                assert!(!p.is_null());
                std::ptr::copy_nonoverlapping(words.as_ptr(), p.cast(), words.len());
                let mut old = 0;
                assert_ne!(VirtualProtect(p, len, 0x02, &raw mut old), 0);
                Self {
                    page: p as usize,
                    len,
                }
            }
        }
    }

    #[cfg(windows)]
    impl Drop for ReadOnlyTable {
        fn drop(&mut self) {
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn VirtualFree(a: *mut std::ffi::c_void, s: usize, k: u32) -> i32;
            }
            let _ = self.len;
            // SAFETY: the allocation `new` made.
            unsafe { VirtualFree(self.page as *mut _, 0, 0x8000) };
        }
    }

    fn call(slot: usize) -> u32 {
        // SAFETY: the test's slots hold `extern "C" fn() -> u32` pointers.
        let f: extern "C" fn() -> u32 = unsafe { std::mem::transmute(*(slot as *const usize)) };
        f()
    }

    #[test]
    fn a_swapped_read_only_slot_calls_the_hook_and_restores_on_drop() {
        let table = ReadOnlyTable::new(&[0xdead, 64, 8, original as *const () as usize]);
        let slot = table.page + 3 * size_of::<usize>();
        assert_eq!(call(slot), 1);
        {
            // SAFETY: the slot holds a function of `hooked`'s type.
            let swap = unsafe { SlotSwap::install(slot, hooked as *const () as usize) }.unwrap();
            assert_eq!(swap.original(), original as *const () as usize);
            assert_eq!(call(slot), 2);
        }
        assert_eq!(call(slot), 1);
        // The page is read-only again: the size word is unchanged and the
        // protection is what the table set.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            let maps = std::fs::read_to_string("/proc/self/maps").unwrap();
            let p = prot_in_maps(&maps, slot).unwrap();
            assert!(p.read && !p.write);
        }
    }

    #[test]
    fn restore_refuses_a_slot_another_swap_replaced() {
        let table = ReadOnlyTable::new(&[original as *const () as usize]);
        // SAFETY: the slot holds a function of `hooked`'s type.
        let mut first =
            unsafe { SlotSwap::install(table.page, hooked as *const () as usize) }.unwrap();
        // SAFETY: as above; the second swap hooks over the first.
        let second =
            unsafe { SlotSwap::install(table.page, original as *const () as usize) }.unwrap();
        assert_eq!(second.original(), hooked as *const () as usize);
        let err = first.restore().unwrap_err().to_string();
        assert!(err.contains("not the hook"), "{err}");
        drop(second);
        // The second swap put the first's hook back, and the first can now
        // restore.
        first.restore().unwrap();
        assert_eq!(call(table.page), 1);
    }

    #[test]
    fn an_unaligned_slot_is_refused() {
        // SAFETY: refused before any access.
        let err = unsafe { SlotSwap::install(0x1003, 0) }
            .unwrap_err()
            .to_string();
        assert!(err.contains("not pointer-aligned"), "{err}");
    }
}
