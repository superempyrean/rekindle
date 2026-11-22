//! The platform-specific part of a detour: executable memory, page
//! protection and instruction-cache coherence.

use std::ptr::NonNull;

use anyhow::{Context, Result, bail};

/// The protection [`protect`] sets on code pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Protection {
    ReadExecute,
    ReadWriteExecute,
}

/// Readable, writable and executable memory this module allocated, freed on
/// drop.
#[derive(Debug)]
pub(super) struct Block {
    ptr: NonNull<u8>,
    size: usize,
}

impl Block {
    /// `size` bytes anywhere in the address space.
    pub(super) fn alloc(size: usize) -> Result<Self> {
        let ptr = sys::alloc(None, size)
            .with_context(|| format!("allocating {size} bytes of executable memory"))?;
        Ok(Self { ptr, size })
    }

    /// `size` bytes that a `jmp rel32` at `target` can reach.
    pub(super) fn alloc_near(target: usize, size: usize) -> Result<Self> {
        // Windows places allocations on 64 KiB boundaries. Searching no
        // further than 0x7000_0000 keeps the whole block inside rel32's 2 GiB.
        const STEP: usize = 0x1_0000;
        const MAX_DELTA: usize = 0x7000_0000;
        let base = target & !(STEP - 1);
        for delta in (0..=MAX_DELTA).step_by(STEP) {
            let above = base.checked_add(delta);
            let below = base.checked_sub(delta).filter(|_| delta != 0);
            for hint in [above, below].into_iter().flatten().filter(|&h| h != 0) {
                let Some(ptr) = sys::alloc(Some(hint), size) else {
                    continue;
                };
                let block = Self { ptr, size };
                if super::x64::near_jmp(target, block.addr()).is_some() {
                    return Ok(block);
                }
            }
        }
        bail!("no free executable memory within rel32 of {target:#x}")
    }

    pub(super) fn addr(&self) -> usize {
        self.ptr.as_ptr() as usize
    }

    /// Copy `bytes` into the block at `at`, and flush the instruction cache
    /// over them.
    ///
    /// # Panics
    ///
    /// If `at + bytes.len()` exceeds the block's size.
    pub(super) fn write(&mut self, at: usize, bytes: &[u8]) {
        // SAFETY: `ptr` points to `size` bytes this block owns, mapped
        // readable and writable, and `&mut self` makes this the only access.
        let block = unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.size) };
        block[at..at + bytes.len()].copy_from_slice(bytes);
        flush(self.ptr.as_ptr().wrapping_add(at), bytes.len());
    }
}

impl Drop for Block {
    fn drop(&mut self) {
        // SAFETY: `ptr` and `size` describe an allocation `sys::alloc` made,
        // and the block is its only owner.
        unsafe { sys::free(self.ptr, self.size) };
    }
}

/// Set the protection of the pages covering `len` bytes at `addr`.
///
/// # Safety
///
/// The pages are mapped in this process, and no code relies on their
/// protection while it changes (in particular, nothing writes to them after
/// they become read-only).
pub(super) unsafe fn protect(addr: *mut u8, len: usize, protection: Protection) -> Result<()> {
    // SAFETY: forwarded from the caller.
    unsafe { sys::protect(addr, len, protection) }
}

/// Make the instruction cache see code written over `len` bytes at `addr`.
pub(super) fn flush(addr: *const u8, len: usize) {
    sys::flush(addr, len);
}

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use anyhow::{Result, bail};

    use super::Protection;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn VirtualAlloc(address: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
        fn VirtualFree(address: *mut c_void, size: usize, kind: u32) -> i32;
        fn VirtualProtect(address: *mut c_void, size: usize, new: u32, old: *mut u32) -> i32;
        fn FlushInstructionCache(process: *mut c_void, base: *const c_void, size: usize) -> i32;
    }

    const MEM_COMMIT: u32 = 0x1000;
    const MEM_RESERVE: u32 = 0x2000;
    const MEM_RELEASE: u32 = 0x8000;
    const PAGE_EXECUTE_READ: u32 = 0x20;
    const PAGE_EXECUTE_READWRITE: u32 = 0x40;

    /// `size` bytes of read-write-execute memory, at `hint` or anywhere.
    pub(super) fn alloc(hint: Option<usize>, size: usize) -> Option<NonNull<u8>> {
        let address = hint.map_or(std::ptr::null_mut(), |h| h as *mut c_void);
        // SAFETY: VirtualAlloc reserves fresh pages, or fails when `address`
        // is taken; it never touches memory already in use.
        let ptr = unsafe {
            VirtualAlloc(
                address,
                size,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_EXECUTE_READWRITE,
            )
        };
        NonNull::new(ptr.cast())
    }

    /// # Safety
    ///
    /// `ptr` is an allocation from [`alloc`] that nothing uses any more.
    pub(super) unsafe fn free(ptr: NonNull<u8>, _size: usize) {
        // A failure leaves the pages allocated, which costs memory and
        // nothing else. MEM_RELEASE frees the whole allocation, and takes a
        // size of 0.
        // SAFETY: forwarded from the caller.
        unsafe { VirtualFree(ptr.as_ptr().cast(), 0, MEM_RELEASE) };
    }

    /// # Safety
    ///
    /// As for [`super::protect`].
    pub(super) unsafe fn protect(addr: *mut u8, len: usize, protection: Protection) -> Result<()> {
        let new = match protection {
            Protection::ReadExecute => PAGE_EXECUTE_READ,
            Protection::ReadWriteExecute => PAGE_EXECUTE_READWRITE,
        };
        let mut old = 0u32;
        // SAFETY: the caller guarantees the pages are mapped and that
        // changing their protection breaks nothing; `old` is a valid out
        // pointer.
        let ok = unsafe { VirtualProtect(addr.cast(), len, new, &raw mut old) };
        if ok == 0 {
            bail!("VirtualProtect failed for {addr:p}");
        }
        Ok(())
    }

    pub(super) fn flush(addr: *const u8, len: usize) {
        // The result is ignored: x64 keeps the instruction cache coherent
        // with stores, so the call is a formality.
        // SAFETY: FlushInstructionCache reads no memory, and the
        // current-process pseudo handle is always valid.
        unsafe { FlushInstructionCache(GetCurrentProcess(), addr.cast(), len) };
    }
}

#[cfg(target_os = "linux")]
mod sys {
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use anyhow::{Result, bail};

    use super::Protection;

    /// `size` bytes of read-write-execute memory, at `hint` or anywhere.
    pub(super) fn alloc(hint: Option<usize>, size: usize) -> Option<NonNull<u8>> {
        // MAP_FIXED_NOREPLACE fails rather than replace a mapping at `hint`.
        // Kernels before 4.17 ignore it and treat `hint` as a hint, so the
        // block may land elsewhere; `Block::alloc_near` checks where it did.
        let (address, fixed) = match hint {
            Some(h) => (h as *mut c_void, libc::MAP_FIXED_NOREPLACE),
            None => (std::ptr::null_mut(), 0),
        };
        // SAFETY: an anonymous private mapping that replaces nothing
        // touches no memory already in use.
        let ptr = unsafe {
            libc::mmap(
                address,
                size,
                libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | fixed,
                -1,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return None;
        }
        NonNull::new(ptr.cast())
    }

    /// # Safety
    ///
    /// `ptr` and `size` are an allocation from [`alloc`] that nothing uses
    /// any more.
    pub(super) unsafe fn free(ptr: NonNull<u8>, size: usize) {
        // A failure leaves the pages mapped, which costs memory and nothing
        // else.
        // SAFETY: forwarded from the caller.
        unsafe { libc::munmap(ptr.as_ptr().cast(), size) };
    }

    /// # Safety
    ///
    /// As for [`super::protect`].
    pub(super) unsafe fn protect(addr: *mut u8, len: usize, protection: Protection) -> Result<()> {
        let prot = match protection {
            Protection::ReadExecute => libc::PROT_READ | libc::PROT_EXEC,
            Protection::ReadWriteExecute => libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
        };
        // mprotect takes whole pages, so widen the range to page boundaries.
        // SAFETY: sysconf reads a constant of the system.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        let start = (addr as usize) & !(page - 1);
        let end = (addr as usize + len).next_multiple_of(page);
        // SAFETY: the caller guarantees the pages are mapped and that
        // changing their protection breaks nothing.
        let rc = unsafe { libc::mprotect(start as *mut c_void, end - start, prot) };
        if rc != 0 {
            bail!("mprotect failed for {addr:p}");
        }
        Ok(())
    }

    pub(super) fn flush(_addr: *const u8, _len: usize) {
        // x64 keeps the instruction cache coherent with stores.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_allocated_near_a_function_is_in_rel32_reach_of_it() {
        let target =
            a_block_allocated_near_a_function_is_in_rel32_reach_of_it as *const () as usize;
        let mut block = Block::alloc_near(target, 16).unwrap();
        assert!(super::super::x64::near_jmp(target, block.addr()).is_some());
        block.write(12, &[0xC3; 4]);
    }
}
