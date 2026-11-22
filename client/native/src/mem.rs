//! Access to the client's memory.
//!
//! Every address rekindle derives from a recovered layout (a field offset, a
//! `Vec` header's pointer, a vtable slot) is checked against committed,
//! accessible memory before it's dereferenced ([`readable`], [`writable`],
//! [`executable`]). A stale offset then reads as "not there" instead of
//! faulting inside the client, and the caller turns rekindle's client layer
//! off.
//!
//! This doesn't make a wrong layout safe: a wrong offset can still land on
//! mapped memory that means something else. That's what the operand anchors
//! are for (the host refuses a layout the binary disagrees with). This is
//! the backstop for what they don't cover.
//!
//! [`read`] and [`write`] are the accesses themselves. They take addresses,
//! not references, and don't need them aligned: the client's fields sit
//! wherever its layout puts them. Memory rekindle hands the client comes
//! from this DLL's global allocator ([`alloc`], [`push_vec`]), which is the
//! process heap the client allocates from and frees to.

use std::alloc::Layout;

use anyhow::{Context, Result};

/// Whether `[p, p + n)` is committed memory that can be read.
pub fn readable(p: usize, n: usize) -> bool {
    imp::accessible(p, n, false)
}

/// Whether `[p, p + n)` is committed memory that can be written.
pub fn writable(p: usize, n: usize) -> bool {
    imp::accessible(p, n, true)
}

/// Whether `p` is committed memory that can be executed: what a function
/// pointer read from a vtable must point at before rekindle calls it.
pub fn executable(p: usize) -> bool {
    imp::executable(p)
}

/// The `T` at `addr`, which needn't be aligned.
///
/// # Safety
/// `[addr, addr + size_of::<T>())` must be readable and hold a valid `T`
/// that nothing writes meanwhile.
pub unsafe fn read<T: Copy>(addr: usize) -> T {
    // SAFETY: the caller's contract; `read_unaligned` has no alignment
    // requirement.
    unsafe { std::ptr::read_unaligned(addr as *const T) }
}

/// Writes `value` at `addr`, which needn't be aligned, without dropping the
/// bytes it replaces.
///
/// # Safety
/// `[addr, addr + size_of::<T>())` must be writable, and nothing else may
/// read or write it meanwhile.
pub unsafe fn write<T>(addr: usize, value: T) {
    // SAFETY: the caller's contract; `write_unaligned` has no alignment
    // requirement.
    unsafe { std::ptr::write_unaligned(addr as *mut T, value) }
}

/// A new allocation of `size` bytes, aligned to `align`, from this DLL's
/// global allocator: memory the client may own and free. Zeroed if
/// `zeroed`. Aborts when the heap is exhausted, as `Box::new` does.
///
/// # Panics
/// If `size` is 0 or `align` is not a power of two (a bug in the caller).
pub fn alloc(size: usize, align: usize, zeroed: bool) -> *mut u8 {
    let layout = Layout::from_size_align(size, align).expect("a power-of-two alignment");
    assert!(size > 0, "an allocation of no bytes");
    // SAFETY: the layout has a non-zero size (asserted above).
    let p = unsafe {
        if zeroed {
            std::alloc::alloc_zeroed(layout)
        } else {
            std::alloc::alloc(layout)
        }
    };
    if p.is_null() {
        std::alloc::handle_alloc_error(layout);
    }
    p
}

/// Appends `elem` to a `Vec` in the client's memory whose `cap`, `ptr` and
/// `len` words sit at `header + words[0..3]`, growing its buffer as `Vec`
/// does (to twice its capacity, at least 4). Its elements are `elem.len()`
/// bytes, 8-aligned. Fails, changing nothing, if the grown buffer's size
/// would overflow.
///
/// # Safety
/// `header` must be a live, writable `Vec` of the client's with elements of
/// `elem.len()` bytes, at most 8-aligned, whose buffer came from the process
/// heap. Nothing else may touch it meanwhile, and `elem` must be a valid
/// element for the `Vec` to own.
pub unsafe fn push_vec(header: usize, words: [usize; 3], elem: &[u8]) -> Result<()> {
    let [cap_at, ptr_at, len_at] = words.map(|w| header + w);
    let stride = elem.len();
    // SAFETY: the caller's contract: a live `Vec` header.
    let (cap, ptr, len) = unsafe {
        (
            read::<usize>(cap_at),
            read::<usize>(ptr_at),
            read::<usize>(len_at),
        )
    };
    let ptr = if len == cap {
        let new_cap = cap.saturating_mul(2).max(4);
        let bytes = new_cap
            .checked_mul(stride)
            .filter(|b| isize::try_from(*b).is_ok())
            .with_context(|| format!("a Vec of capacity {cap} can't grow"))?;
        let grown = alloc(bytes, 8, false);
        // SAFETY: the old buffer holds `len` elements (the caller's
        // contract), the new one room for `new_cap > len`, and the two are
        // distinct allocations. The old one came from the process heap with
        // this layout, as `Vec` allocates it.
        unsafe {
            std::ptr::copy_nonoverlapping(ptr as *const u8, grown, len * stride);
            if cap > 0 {
                std::alloc::dealloc(
                    ptr as *mut u8,
                    Layout::from_size_align_unchecked(cap * stride, 8),
                );
            }
            write(cap_at, new_cap);
            write(ptr_at, grown as usize);
        }
        grown as usize
    } else {
        ptr
    };
    // SAFETY: `len < cap` now, so slot `len` is in the buffer, and the
    // header is the caller's live `Vec`.
    unsafe {
        std::ptr::copy_nonoverlapping(elem.as_ptr(), (ptr + len * stride) as *mut u8, stride);
        write(len_at, len + 1);
    }
    Ok(())
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    #[repr(C)]
    struct MemoryBasicInformation {
        base_address: *mut c_void,
        allocation_base: *mut c_void,
        allocation_protect: u32,
        partition_id: u16,
        region_size: usize,
        state: u32,
        protect: u32,
        kind: u32,
    }

    unsafe extern "system" {
        fn VirtualQuery(
            address: *const c_void,
            buffer: *mut MemoryBasicInformation,
            length: usize,
        ) -> usize;
    }

    const MEM_COMMIT: u32 = 0x1000;
    const PAGE_GUARD: u32 = 0x100;
    const PAGE_NOCACHE_WRITECOMBINE: u32 = 0x200 | 0x400;
    // PAGE_READONLY, _READWRITE, _WRITECOPY, _EXECUTE_READ, _EXECUTE_READWRITE, _EXECUTE_WRITECOPY.
    const READ: u32 = 0x02 | 0x04 | 0x08 | 0x20 | 0x40 | 0x80;
    const WRITE: u32 = 0x04 | 0x08 | 0x40 | 0x80;
    // PAGE_EXECUTE, _EXECUTE_READ, _EXECUTE_READWRITE, _EXECUTE_WRITECOPY.
    const EXECUTE: u32 = 0x10 | 0x20 | 0x40 | 0x80;

    /// The region of memory `p` is in, if it's in one.
    fn query(p: usize) -> Option<MemoryBasicInformation> {
        let mut info = std::mem::MaybeUninit::<MemoryBasicInformation>::uninit();
        // SAFETY: VirtualQuery writes at most `length` bytes to `info`, and
        // any address is a valid query.
        let got = unsafe {
            VirtualQuery(
                p as *const c_void,
                info.as_mut_ptr(),
                std::mem::size_of::<MemoryBasicInformation>(),
            )
        };
        // SAFETY: a non-zero return filled `info`.
        (got != 0).then(|| unsafe { info.assume_init() })
    }

    pub(super) fn executable(p: usize) -> bool {
        p != 0
            && query(p).is_some_and(|info| {
                info.state == MEM_COMMIT
                    && info.protect & PAGE_GUARD == 0
                    && info.protect & EXECUTE != 0
            })
    }

    /// Walks every region the range touches; each must be committed, not a
    /// guard page, and allow the access.
    pub(super) fn accessible(p: usize, n: usize, write: bool) -> bool {
        if p == 0 {
            return false;
        }
        let Some(end) = p.checked_add(n.max(1)) else {
            return false;
        };
        let allowed = if write { WRITE } else { READ };
        let mut at = p;
        while at < end {
            let Some(info) = query(at) else {
                return false;
            };
            let protect = info.protect & !PAGE_NOCACHE_WRITECOMBINE;
            if info.state != MEM_COMMIT || protect & PAGE_GUARD != 0 || protect & allowed == 0 {
                return false;
            }
            let region_end = (info.base_address as usize).saturating_add(info.region_size);
            if region_end <= at {
                return false;
            }
            at = region_end;
        }
        true
    }
}

#[cfg(target_os = "linux")]
mod imp {
    /// Reads one byte of every page the range touches through
    /// `process_vm_readv`, which fails instead of faulting.
    fn readable(p: usize, n: usize) -> bool {
        let Some(end) = p.checked_add(n.max(1)) else {
            return false;
        };
        let mut at = p;
        while at < end {
            let mut byte = 0u8;
            let local = libc::iovec {
                iov_base: std::ptr::from_mut(&mut byte).cast(),
                iov_len: 1,
            };
            let remote = libc::iovec {
                iov_base: at as *mut libc::c_void,
                iov_len: 1,
            };
            // SAFETY: the kernel writes at most one byte, into `byte`, and
            // checks the remote side itself.
            if unsafe {
                libc::process_vm_readv(libc::getpid(), &raw const local, 1, &raw const remote, 1, 0)
            } != 1
            {
                return false;
            }
            at = (at | 0xfff) + 1;
        }
        true
    }

    /// Whether every mapping `[p, end)` touches is mapped with `perm`.
    fn mapped_with(p: usize, end: usize, perm: u8) -> bool {
        let Ok(maps) = std::fs::read_to_string("/proc/self/maps") else {
            return false;
        };
        let mut at = p;
        for line in maps.lines() {
            let mut f = line.split_whitespace();
            let (Some(range), Some(perms)) = (f.next(), f.next()) else {
                return false;
            };
            let Some((s, e)) = range.split_once('-') else {
                return false;
            };
            let (Ok(s), Ok(e)) = (usize::from_str_radix(s, 16), usize::from_str_radix(e, 16))
            else {
                return false;
            };
            if e <= at {
                continue;
            }
            if s > at || !perms.as_bytes().contains(&perm) {
                return false;
            }
            at = e;
            if at >= end {
                return true;
            }
        }
        false
    }

    pub(super) fn accessible(p: usize, n: usize, write: bool) -> bool {
        if p == 0 {
            return false;
        }
        let Some(end) = p.checked_add(n.max(1)) else {
            return false;
        };
        readable(p, n) && (!write || mapped_with(p, end, b'w'))
    }

    pub(super) fn executable(p: usize) -> bool {
        p != 0 && mapped_with(p, p + 1, b'x')
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    /// No query off Windows and Linux (the client runs only there): only
    /// null fails.
    pub(super) fn accessible(p: usize, _n: usize, _write: bool) -> bool {
        p != 0
    }

    pub(super) fn executable(p: usize) -> bool {
        p != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_and_heap_are_readable_and_writable() {
        let local = [0u8; 64];
        assert!(readable(local.as_ptr() as usize, local.len()));
        let heap = vec![0u8; 1 << 20];
        assert!(writable(heap.as_ptr() as usize, heap.len()));
    }

    #[cfg(windows)]
    #[test]
    fn null_low_and_read_only_memory_are_refused() {
        assert!(!readable(0, 8));
        assert!(!readable(0x10, 8), "the first page is never mapped");
        assert!(!readable(usize::MAX - 4, 8), "a range that wraps");
        // A string literal lives in .rdata: readable, not writable.
        let lit: &'static str = "rekindle mem test";
        assert!(readable(lit.as_ptr() as usize, lit.len()));
        assert!(!writable(lit.as_ptr() as usize, lit.len()));
    }

    #[cfg(windows)]
    #[test]
    fn code_is_executable_and_data_is_not() {
        assert!(executable(
            code_is_executable_and_data_is_not as *const () as usize
        ));
        let heap = Box::new([0u8; 64]);
        assert!(!executable(heap.as_ptr() as usize));
        assert!(!executable(0));
    }

    #[cfg(windows)]
    #[test]
    fn a_range_past_its_allocation_end_is_refused_when_unmapped() {
        unsafe extern "system" {
            fn VirtualAlloc(
                a: *mut std::ffi::c_void,
                n: usize,
                kind: u32,
                protect: u32,
            ) -> *mut std::ffi::c_void;
            fn VirtualFree(a: *mut std::ffi::c_void, n: usize, kind: u32) -> i32;
        }
        const MEM_COMMIT: u32 = 0x1000;
        const MEM_RESERVE: u32 = 0x2000;
        const MEM_RELEASE: u32 = 0x8000;
        const PAGE_READWRITE: u32 = 0x04;
        // Reserve two pages and commit one: the second is reserved, not
        // committed, so a range spanning both is refused.
        // SAFETY: reserving fresh address space touches nothing in use.
        let p = unsafe { VirtualAlloc(std::ptr::null_mut(), 0x2000, MEM_RESERVE, PAGE_READWRITE) };
        assert!(!p.is_null());
        // SAFETY: commits the first page of the reservation just made.
        let committed = unsafe { VirtualAlloc(p, 0x1000, MEM_COMMIT, PAGE_READWRITE) };
        assert!(!committed.is_null());
        assert!(writable(p as usize, 0x1000));
        assert!(!readable(p as usize, 0x1001));
        assert!(!readable(p as usize + 0x1000, 8));
        // SAFETY: releases the reservation, which nothing else uses.
        unsafe { VirtualFree(p, 0, MEM_RELEASE) };
    }

    #[test]
    fn reads_and_writes_need_no_alignment() {
        let mut buf = [0u8; 16];
        let at = buf.as_mut_ptr() as usize + 3;
        // SAFETY: `[at, at + 8)` lies inside `buf`, which nothing else uses.
        let back = unsafe {
            write(at, 0x1122_3344_5566_7788u64);
            read::<u64>(at)
        };
        assert_eq!(back, 0x1122_3344_5566_7788);
        assert_eq!(buf[3], 0x88);
    }

    /// A `Vec` header laid out `[cap, ptr, len]`, as rekindle sees the
    /// client's.
    fn header(v: Vec<[u64; 2]>) -> [usize; 3] {
        let mut v = std::mem::ManuallyDrop::new(v);
        [v.capacity(), v.as_mut_ptr() as usize, v.len()]
    }

    #[test]
    fn push_vec_appends_in_place_and_grows_when_full() {
        let mut v = Vec::with_capacity(2);
        v.push([1u64, 2]);
        let mut h = header(v);
        let at = h.as_mut_ptr() as usize;
        let elem = |a: u64, b: u64| {
            let mut e = [0u8; 16];
            e[..8].copy_from_slice(&a.to_ne_bytes());
            e[8..].copy_from_slice(&b.to_ne_bytes());
            e
        };
        // SAFETY: `h` is the header of a live Vec<[u64; 2]> from the global
        // allocator, used by nothing else.
        unsafe {
            push_vec(at, [0, 8, 16], &elem(3, 4)).unwrap();
            assert_eq!((h[0], h[2]), (2, 2), "room for one more: no growth");
            push_vec(at, [0, 8, 16], &elem(5, 6)).unwrap();
            assert_eq!((h[0], h[2]), (4, 3), "full: doubled");
            let v = Vec::from_raw_parts(h[1] as *mut [u64; 2], h[2], h[0]);
            assert_eq!(v, [[1, 2], [3, 4], [5, 6]]);
        }
    }

    #[test]
    fn push_vec_refuses_a_capacity_it_cant_double() {
        let mut h = [usize::MAX / 4, 8, usize::MAX / 4];
        // SAFETY: a full Vec's header that is never dereferenced: growing it
        // fails before anything is read through `ptr`.
        let pushed = unsafe { push_vec(h.as_mut_ptr() as usize, [0, 8, 16], &[0; 16]) };
        assert!(pushed.is_err());
        assert_eq!(h, [usize::MAX / 4, 8, usize::MAX / 4]);
    }
}
