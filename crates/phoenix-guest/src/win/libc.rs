//! Memory, strings, numbers, formatting, maths, locales and wide
//! characters, with bionic's types: `long` is 64 bits and `wchar_t` 32.
#![expect(
    clippy::undocumented_unsafe_blocks,
    reason = "every shim dereferences the pointers the library passes, under the contract of the bionic function it stands in for"
)]

use std::cell::UnsafeCell;
use std::ffi::{CStr, c_char, c_void};
use std::ptr::null_mut;

use windows_sys::Win32::System::Memory::{
    GetProcessHeap, HeapAlloc, HeapFree, MEM_COMMIT, MEM_DECOMMIT, MEM_RELEASE, MEM_RESERVE,
    MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE,
    VirtualAlloc, VirtualFree, VirtualQuery,
};

use super::{errno, set_errno, ts};
use crate::errno::{EILSEQ, EINVAL, ENOMEM, ERANGE};
use crate::loader::caller;
use crate::log::glog;
use crate::num::{strtod, strtoint};
use crate::valist::{SysvArgs, VaList, f64_to_f128, returns_f128, variadic};

type Sl = *mut VaList;

pub(super) fn cbytes<'a>(s: *const c_char) -> &'a [u8] {
    if s.is_null() {
        b""
    } else {
        unsafe { CStr::from_ptr(s) }.to_bytes()
    }
}

pub(super) fn cstr_lossy(s: *const c_char) -> String {
    if s.is_null() {
        "(null)".into()
    } else {
        String::from_utf8_lossy(cbytes(s)).into_owned()
    }
}

// ---------- memory ----------

/// Every block carries {block start, size} in the 16 bytes before it, so
/// `free` and `realloc` take `malloc`'s and `posix_memalign`'s alike.
fn xalloc(n: usize, align: usize) -> *mut c_void {
    let align = align.max(16);
    if n > 1 << 46 {
        return null_mut();
    }
    let real = unsafe { HeapAlloc(GetProcessHeap(), 0, n + align + 16) };
    if real.is_null() {
        return null_mut();
    }
    let p = (real as usize + 16 + align - 1) & !(align - 1);
    unsafe {
        (p as *mut u64).sub(2).write(real as u64);
        (p as *mut u64).sub(1).write(n as u64);
    }
    p as *mut c_void
}

pub(super) extern "sysv64" fn s_malloc(n: usize) -> *mut c_void {
    let p = xalloc(n, 16);
    if p.is_null() {
        set_errno(ENOMEM);
    }
    p
}

pub(super) extern "sysv64" fn s_free(p: *mut c_void) {
    if !p.is_null() {
        unsafe {
            HeapFree(
                GetProcessHeap(),
                0,
                (p as *const u64).sub(2).read() as *const c_void,
            )
        };
    }
}

extern "sysv64" fn s_calloc(a: usize, b: usize) -> *mut c_void {
    let Some(n) = a.checked_mul(b) else {
        set_errno(ENOMEM);
        return null_mut();
    };
    let p = s_malloc(n);
    if !p.is_null() && n > 0 {
        unsafe { p.cast::<u8>().write_bytes(0, n) };
    }
    p
}

extern "sysv64" fn s_realloc(old: *mut c_void, n: usize) -> *mut c_void {
    if old.is_null() {
        return s_malloc(n);
    }
    if n == 0 {
        s_free(old);
        return null_mut();
    }
    let on = unsafe { (old as *const u64).sub(1).read() } as usize;
    let p = s_malloc(n);
    if p.is_null() {
        return p;
    }
    unsafe { std::ptr::copy_nonoverlapping(old.cast::<u8>(), p.cast::<u8>(), on.min(n)) };
    s_free(old);
    p
}

extern "sysv64" fn s_posix_memalign(out: *mut *mut c_void, align: usize, n: usize) -> i32 {
    if align == 0 || !align.is_power_of_two() || !align.is_multiple_of(8) {
        return EINVAL;
    }
    let p = xalloc(n, align);
    if p.is_null() {
        return ENOMEM;
    }
    unsafe { *out = p };
    0
}

/// A copy in the library's heap, for it to free.
pub(super) fn guest_strdup(s: &[u8]) -> *mut c_char {
    let p = s_malloc(s.len() + 1).cast::<u8>();
    if !p.is_null() {
        unsafe {
            std::ptr::copy_nonoverlapping(s.as_ptr(), p, s.len());
            *p.add(s.len()) = 0;
        }
    }
    p.cast()
}

const MAP_FIXED: i32 = 0x10;
const MAP_ANONYMOUS: i32 = 0x20;

extern "sysv64" fn s_mmap(
    addr: *mut c_void,
    len: usize,
    prot: i32,
    flags: i32,
    fd: i32,
    off: i64,
) -> *mut c_void {
    let failed = usize::MAX as *mut c_void;
    if flags & MAP_ANONYMOUS == 0 || flags & MAP_FIXED != 0 {
        glog!(
            "[mmap] {addr:?} len={len:#x} prot={prot} flags={flags:#x} fd={fd} off={off} unsupported from {}\n",
            caller()
        );
        set_errno(ENOMEM);
        return failed;
    }
    let wp = if prot == 0 {
        PAGE_NOACCESS
    } else if prot & 4 != 0 {
        PAGE_EXECUTE_READWRITE
    } else if prot & 2 != 0 {
        PAGE_READWRITE
    } else {
        PAGE_READONLY
    };
    let kind = if prot == 0 {
        MEM_RESERVE
    } else {
        MEM_RESERVE | MEM_COMMIT
    };
    let p = unsafe { VirtualAlloc(std::ptr::null(), len, kind, wp) };
    if p.is_null() {
        set_errno(ENOMEM);
        return failed;
    }
    p
}

extern "sysv64" fn s_munmap(addr: *mut c_void, len: usize) -> i32 {
    let mut mi: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let sz = size_of::<MEMORY_BASIC_INFORMATION>();
    if unsafe { VirtualQuery(addr, &raw mut mi, sz) } == 0 {
        set_errno(EINVAL);
        return -1;
    }
    if mi.AllocationBase == addr {
        let mut total = 0;
        while unsafe { VirtualQuery(addr.cast::<u8>().add(total).cast(), &raw mut mi, sz) } != 0
            && mi.AllocationBase == addr
        {
            total += mi.RegionSize;
        }
        if len >= total {
            unsafe { VirtualFree(addr, 0, MEM_RELEASE) };
            return 0;
        }
    }
    unsafe { VirtualFree(addr, len, MEM_DECOMMIT) };
    0
}

extern "sysv64" fn s_madvise(_a: *mut c_void, _n: usize, _adv: i32) -> i32 {
    0
}

// ---------- strings ----------

extern "sysv64" fn s_memchr(s: *const u8, c: i32, n: usize) -> *const u8 {
    for i in 0..n {
        if unsafe { *s.add(i) } == c as u8 {
            return unsafe { s.add(i) };
        }
    }
    std::ptr::null()
}

extern "sysv64" fn s_memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    for i in 0..n {
        let (x, y) = unsafe { (*a.add(i), *b.add(i)) };
        if x != y {
            return i32::from(x) - i32::from(y);
        }
    }
    0
}

extern "sysv64" fn s_memcpy(d: *mut u8, s: *const u8, n: usize) -> *mut u8 {
    if n > 0 {
        unsafe { std::ptr::copy(s, d, n) };
    }
    d
}

extern "sysv64" fn s_memset(d: *mut u8, c: i32, n: usize) -> *mut u8 {
    if n > 0 {
        unsafe { d.write_bytes(c as u8, n) };
    }
    d
}

extern "sysv64" fn s_memcpy_chk(d: *mut u8, s: *const u8, n: usize, _dl: usize) -> *mut u8 {
    s_memcpy(d, s, n)
}

extern "sysv64" fn s_memset_chk(d: *mut u8, c: i32, n: usize, _dl: usize) -> *mut u8 {
    s_memset(d, c, n)
}

extern "sysv64" fn s_strchr(s: *const u8, c: i32) -> *const u8 {
    let mut p = s;
    loop {
        let b = unsafe { *p };
        if b == c as u8 {
            return p;
        }
        if b == 0 {
            return std::ptr::null();
        }
        p = unsafe { p.add(1) };
    }
}

extern "sysv64" fn s_strrchr(s: *const u8, c: i32) -> *const u8 {
    let mut found = std::ptr::null();
    let mut p = s;
    loop {
        let b = unsafe { *p };
        if b == c as u8 {
            found = p;
        }
        if b == 0 {
            return found;
        }
        p = unsafe { p.add(1) };
    }
}

extern "sysv64" fn s_strncmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    for i in 0..n {
        let (x, y) = unsafe { (*a.add(i), *b.add(i)) };
        if x != y || x == 0 {
            return i32::from(x) - i32::from(y);
        }
    }
    0
}

extern "sysv64" fn s_strcmp(a: *const u8, b: *const u8) -> i32 {
    s_strncmp(a, b, usize::MAX)
}

extern "sysv64" fn s_strlen(s: *const c_char) -> usize {
    cbytes(s).len()
}

extern "sysv64" fn s_strlen_chk(s: *const c_char, _n: usize) -> usize {
    cbytes(s).len()
}

extern "sysv64" fn s_wcslen(s: *const u32) -> usize {
    let mut n = 0;
    while unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    n
}

extern "sysv64" fn s_wmemchr(s: *const u32, c: u32, n: usize) -> *const u32 {
    for i in 0..n {
        if unsafe { *s.add(i) } == c {
            return unsafe { s.add(i) };
        }
    }
    std::ptr::null()
}

// ---------- numbers ----------

fn set_end(end: *mut *const c_char, s: *const c_char, used: usize) {
    if !end.is_null() {
        unsafe { *end = s.add(used) };
    }
}

fn int(s: *const c_char, end: *mut *const c_char, base: i32, signed: bool) -> u64 {
    let r = strtoint(cbytes(s), base as u32, signed);
    if r.overflow {
        set_errno(ERANGE);
    }
    set_end(end, s, r.used);
    r.value
}

extern "sysv64" fn s_strtol(s: *const c_char, e: *mut *const c_char, b: i32) -> i64 {
    int(s, e, b, true) as i64
}

extern "sysv64" fn s_strtoul(s: *const c_char, e: *mut *const c_char, b: i32) -> u64 {
    int(s, e, b, false)
}

extern "sysv64" fn s_strtoll_l(s: *const c_char, e: *mut *const c_char, b: i32, _l: usize) -> i64 {
    int(s, e, b, true) as i64
}

extern "sysv64" fn s_strtoull_l(s: *const c_char, e: *mut *const c_char, b: i32, _l: usize) -> u64 {
    int(s, e, b, false)
}

extern "sysv64" fn s_strtod(s: *const c_char, e: *mut *const c_char) -> f64 {
    let (v, n) = strtod(cbytes(s));
    set_end(e, s, n);
    v
}

extern "sysv64" fn s_strtof(s: *const c_char, e: *mut *const c_char) -> f32 {
    s_strtod(s, e) as f32
}

extern "sysv64" fn strtold_bits(s: *const c_char, e: *mut *const c_char, _l: usize) -> u128 {
    f64_to_f128(s_strtod(s, e))
}
returns_f128!(s_strtold_l, strtold_bits);

// ---------- sorting ----------

type Cmp = extern "sysv64" fn(*const c_void, *const c_void) -> i32;

extern "sysv64" fn s_bsearch(
    key: *const c_void,
    base: *const u8,
    n: usize,
    sz: usize,
    cmp: Cmp,
) -> *const u8 {
    let (mut lo, mut hi) = (0, n);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let p = unsafe { base.add(mid * sz) };
        match cmp(key, p.cast()) {
            0 => return p,
            c if c < 0 => hi = mid,
            _ => lo = mid + 1,
        }
    }
    std::ptr::null()
}

/// A merge sort of indices, which stays well-defined under any comparator.
fn merge_sort(idx: &mut [usize], tmp: &mut [usize], less_eq: &dyn Fn(usize, usize) -> bool) {
    let n = idx.len();
    if n < 2 {
        return;
    }
    let mid = n / 2;
    merge_sort(&mut idx[..mid], &mut tmp[..mid], less_eq);
    merge_sort(&mut idx[mid..], &mut tmp[mid..], less_eq);
    let (mut i, mut j) = (0, mid);
    for slot in tmp.iter_mut().take(n) {
        if j >= n || (i < mid && less_eq(idx[i], idx[j])) {
            *slot = idx[i];
            i += 1;
        } else {
            *slot = idx[j];
            j += 1;
        }
    }
    idx.copy_from_slice(&tmp[..n]);
}

extern "sysv64" fn s_qsort(base: *mut u8, n: usize, sz: usize, cmp: Cmp) {
    if n < 2 || sz == 0 {
        return;
    }
    let at = |i: usize| unsafe { base.add(i * sz) }.cast_const().cast::<c_void>();
    let mut idx: Vec<usize> = (0..n).collect();
    let mut tmp = vec![0; n];
    merge_sort(&mut idx, &mut tmp, &|a, b| cmp(at(a), at(b)) <= 0);
    let copy = unsafe { std::slice::from_raw_parts(base, n * sz) }.to_vec();
    for (k, &i) in idx.iter().enumerate() {
        unsafe { std::ptr::copy_nonoverlapping(copy.as_ptr().add(i * sz), base.add(k * sz), sz) };
    }
}

// ---------- errors ----------

thread_local! {
    static ERRBUF: UnsafeCell<[u8; 64]> = const { UnsafeCell::new([0; 64]) };
}

extern "sysv64" fn s_strerror(e: i32) -> *const c_char {
    let m = crate::errno::message(e);
    ERRBUF.with(|b| {
        let b = unsafe { &mut *b.get() };
        let n = m.len().min(63);
        b[..n].copy_from_slice(&m.as_bytes()[..n]);
        b[n] = 0;
        b.as_ptr().cast()
    })
}

extern "sysv64" fn s_strerror_r(e: i32, buf: *mut u8, n: usize) -> i32 {
    let m = crate::errno::message(e);
    if m.len() + 1 > n {
        return ERANGE;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(m.as_ptr(), buf, m.len());
        *buf.add(m.len()) = 0;
    }
    0
}

// ---------- printf and scanf ----------

pub(super) fn vformat(fmt: *const c_char, ap: Sl) -> Vec<u8> {
    let mut out = Vec::new();
    crate::printf::format(&mut out, cbytes(fmt), &mut SysvArgs(ap), errno());
    out
}

extern "sysv64" fn s_vsnprintf(buf: *mut u8, n: usize, fmt: *const c_char, ap: Sl) -> i32 {
    let out = vformat(fmt, ap);
    if n > 0 {
        let c = out.len().min(n - 1);
        unsafe {
            std::ptr::copy_nonoverlapping(out.as_ptr(), buf, c);
            *buf.add(c) = 0;
        }
    }
    out.len() as i32
}
variadic!(s_snprintf, 3, "rcx", s_vsnprintf);

extern "sysv64" fn s_vsnprintf_chk(
    buf: *mut u8,
    n: usize,
    _flags: i32,
    _sl: usize,
    fmt: *const c_char,
    ap: Sl,
) -> i32 {
    s_vsnprintf(buf, n, fmt, ap)
}

extern "sysv64" fn s_vasprintf(out: *mut *mut c_char, fmt: *const c_char, ap: Sl) -> i32 {
    let b = vformat(fmt, ap);
    unsafe { *out = guest_strdup(&b) };
    b.len() as i32
}

extern "sysv64" fn s_vsscanf(s: *const c_char, fmt: *const c_char, ap: Sl) -> i32 {
    let r = crate::scanf::scan(cbytes(s), cbytes(fmt), &mut SysvArgs(ap));
    r.unwrap_or_else(|| {
        glog!(
            "[scanf] unsupported format, refused
"
        );
        -1
    })
}
variadic!(s_sscanf, 2, "rdx", s_vsscanf);

// ---------- maths ----------

extern "sysv64" fn s_acos(x: f64) -> f64 {
    x.acos()
}
extern "sysv64" fn s_acosf(x: f32) -> f32 {
    x.acos()
}
extern "sysv64" fn s_atanf(x: f32) -> f32 {
    x.atan()
}
extern "sysv64" fn s_cos(x: f64) -> f64 {
    x.cos()
}
extern "sysv64" fn s_cosf(x: f32) -> f32 {
    x.cos()
}
extern "sysv64" fn s_exp(x: f64) -> f64 {
    x.exp()
}
extern "sysv64" fn s_exp2(x: f64) -> f64 {
    x.exp2()
}
extern "sysv64" fn s_exp2f(x: f32) -> f32 {
    x.exp2()
}
extern "sysv64" fn s_expf(x: f32) -> f32 {
    x.exp()
}
extern "sysv64" fn s_expm1(x: f64) -> f64 {
    x.exp_m1()
}
extern "sysv64" fn s_log(x: f64) -> f64 {
    x.ln()
}
extern "sysv64" fn s_log10(x: f64) -> f64 {
    x.log10()
}
extern "sysv64" fn s_log10f(x: f32) -> f32 {
    x.log10()
}
extern "sysv64" fn s_log2(x: f64) -> f64 {
    x.log2()
}
extern "sysv64" fn s_log2f(x: f32) -> f32 {
    x.log2()
}
extern "sysv64" fn s_logf(x: f32) -> f32 {
    x.ln()
}
extern "sysv64" fn s_sin(x: f64) -> f64 {
    x.sin()
}
extern "sysv64" fn s_sinf(x: f32) -> f32 {
    x.sin()
}
extern "sysv64" fn s_tan(x: f64) -> f64 {
    x.tan()
}
extern "sysv64" fn s_tanf(x: f32) -> f32 {
    x.tan()
}
extern "sysv64" fn s_tanhf(x: f32) -> f32 {
    x.tanh()
}
extern "sysv64" fn s_atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}
extern "sysv64" fn s_atan2f(y: f32, x: f32) -> f32 {
    y.atan2(x)
}
extern "sysv64" fn s_hypot(x: f64, y: f64) -> f64 {
    x.hypot(y)
}
extern "sysv64" fn s_hypotf(x: f32, y: f32) -> f32 {
    x.hypot(y)
}
extern "sysv64" fn s_pow(x: f64, y: f64) -> f64 {
    x.powf(y)
}
extern "sysv64" fn s_powf(x: f32, y: f32) -> f32 {
    x.powf(y)
}
extern "sysv64" fn s_sincos(x: f64, s: *mut f64, c: *mut f64) {
    unsafe {
        *s = x.sin();
        *c = x.cos();
    }
}
extern "sysv64" fn s_sincosf(x: f32, s: *mut f32, c: *mut f32) {
    unsafe {
        *s = x.sin();
        *c = x.cos();
    }
}

// ---------- locales ----------

/// A locale: whether it is UTF-8, and its name.
struct Locale {
    utf8: bool,
}

const GLOBAL_LOCALE: usize = usize::MAX;

extern "sysv64" fn s_newlocale(mask: i32, name: *const c_char, _base: usize) -> *mut c_void {
    let n = cstr_lossy(name);
    let l = Box::into_raw(Box::new(Locale {
        utf8: n.contains("UTF-8") || n.contains("utf8"),
    }));
    glog!("[locale] newlocale({mask:#x}, {n}) -> {l:?}\n");
    l.cast()
}

extern "sysv64" fn s_freelocale(l: *mut c_void) {
    if !l.is_null() && l as usize != GLOBAL_LOCALE {
        drop(unsafe { Box::from_raw(l.cast::<Locale>()) });
    }
}

extern "sysv64" fn s_uselocale(l: usize) -> usize {
    let t = ts();
    unsafe {
        let old = if (*t).locale == 0 {
            GLOBAL_LOCALE
        } else {
            (*t).locale
        };
        if l != 0 {
            (*t).locale = l;
        }
        old
    }
}

extern "sysv64" fn s_ctype_get_mb_cur_max() -> usize {
    let l = unsafe { (*ts()).locale };
    if l != 0 && l != GLOBAL_LOCALE && unsafe { (*(l as *const Locale)).utf8 } {
        4
    } else {
        1
    }
}

/// bionic's `struct lconv`, the C locale's values.
#[repr(C)]
struct Lconv {
    strs: [*const c_char; 10],
    chars: [u8; 14],
}

struct SyncLconv(Lconv);
// SAFETY: read-only after construction.
unsafe impl Sync for SyncLconv {}

extern "sysv64" fn s_localeconv() -> *const c_void {
    static LC: SyncLconv = SyncLconv(Lconv {
        strs: [
            c".".as_ptr(),
            c"".as_ptr(),
            c"".as_ptr(),
            c"".as_ptr(),
            c"".as_ptr(),
            c"".as_ptr(),
            c"".as_ptr(),
            c"".as_ptr(),
            c"".as_ptr(),
            c"".as_ptr(),
        ],
        chars: [127; 14],
    });
    (&raw const LC.0).cast()
}

extern "sysv64" fn s_iswlower_l(c: u32, _l: usize) -> i32 {
    i32::from(char::from_u32(c).is_some_and(char::is_lowercase))
}

// ---------- multibyte ----------

const PARTIAL: usize = usize::MAX - 1;
const INVALID: usize = usize::MAX;

/// Decodes one UTF-8 character, as bionic does in every locale. Returns
/// its length (0 for NUL), `PARTIAL` or `INVALID`.
fn utf8_get(out: &mut u32, s: &[u8]) -> usize {
    let Some(&b) = s.first() else {
        return PARTIAL;
    };
    if b < 0x80 {
        *out = u32::from(b);
        return usize::from(b != 0);
    }
    let len = match b {
        0xf0..=0xf4 => 4,
        0xe0..=0xef => 3,
        0xc0..=0xdf => 2,
        _ => return INVALID,
    };
    if s.len() < len {
        return PARTIAL;
    }
    let mut c = u32::from(b & (0x7f >> len));
    for &x in &s[1..len] {
        if x & 0xc0 != 0x80 {
            return INVALID;
        }
        c = c << 6 | u32::from(x & 63);
    }
    *out = c;
    len
}

extern "sysv64" fn s_mbrtowc(pwc: *mut u32, s: *const u8, n: usize, _ps: usize) -> usize {
    if s.is_null() {
        return 0;
    }
    let mut c = 0;
    let r = utf8_get(&mut c, unsafe { std::slice::from_raw_parts(s, n) });
    if r == INVALID {
        set_errno(EILSEQ);
    } else if r != PARTIAL && !pwc.is_null() {
        unsafe { *pwc = c };
    }
    r
}

extern "sysv64" fn s_mbrlen(s: *const u8, n: usize, ps: usize) -> usize {
    s_mbrtowc(null_mut(), s, n, ps)
}

extern "sysv64" fn s_mbtowc(pwc: *mut u32, s: *const u8, n: usize) -> i32 {
    if s.is_null() {
        return 0;
    }
    let r = s_mbrtowc(pwc, s, n, 0);
    if r >= PARTIAL { -1 } else { r as i32 }
}

extern "sysv64" fn s_wcrtomb(s: *mut u8, wc: u32, _ps: usize) -> usize {
    if s.is_null() {
        return 1;
    }
    if wc > 0x10ffff || (0xd800..0xe000).contains(&wc) {
        set_errno(EILSEQ);
        return INVALID;
    }
    let mut b = Vec::new();
    crate::printf::utf8_push(&mut b, wc);
    unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), s, b.len()) };
    b.len()
}

extern "sysv64" fn s_mbsnrtowcs(
    dst: *mut u32,
    src: *mut *const u8,
    nms: usize,
    len: usize,
    _ps: usize,
) -> usize {
    let s = unsafe { *src };
    let (mut i, mut used) = (0, 0);
    while dst.is_null() || i < len {
        let mut c = 0;
        // The input is NUL-terminated within `nms`, which may be huge.
        let avail = nms - used;
        let window = unsafe { std::slice::from_raw_parts(s.add(used), avail.min(4)) };
        let r = utf8_get(&mut c, window);
        if r == INVALID {
            set_errno(EILSEQ);
            return r;
        }
        if r == PARTIAL {
            break;
        }
        if r == 0 {
            if !dst.is_null() {
                unsafe {
                    *dst.add(i) = 0;
                    *src = std::ptr::null();
                }
            }
            return i;
        }
        if !dst.is_null() {
            unsafe { *dst.add(i) = c };
        }
        i += 1;
        used += r;
    }
    if !dst.is_null() {
        unsafe { *src = s.add(used) };
    }
    i
}

extern "sysv64" fn s_mbsrtowcs(dst: *mut u32, src: *mut *const u8, len: usize, ps: usize) -> usize {
    let s = unsafe { *src };
    // Bound the input by its terminator, so no read passes it.
    let n = cbytes(s.cast()).len() + 1;
    s_mbsnrtowcs(dst, src, n, len, ps)
}

extern "sysv64" fn s_wcsnrtombs(
    dst: *mut u8,
    src: *mut *const u32,
    nwc: usize,
    len: usize,
    _ps: usize,
) -> usize {
    let s = unsafe { *src };
    let mut o = 0;
    for i in 0..nwc {
        let c = unsafe { *s.add(i) };
        if c == 0 {
            if !dst.is_null() {
                unsafe {
                    if o < len {
                        *dst.add(o) = 0;
                    }
                    *src = std::ptr::null();
                }
            }
            return o;
        }
        if c > 0x10ffff {
            set_errno(EILSEQ);
            return INVALID;
        }
        let mut b = Vec::new();
        crate::printf::utf8_push(&mut b, c);
        if !dst.is_null() {
            if o + b.len() > len {
                unsafe { *src = s.add(i) };
                return o;
            }
            unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), dst.add(o), b.len()) };
        }
        o += b.len();
    }
    if !dst.is_null() {
        unsafe { *src = s.add(nwc) };
    }
    o
}

macro_rules! s {
    ($($name:ident),* $(,)?) => {
        vec![$((&stringify!($name)[2..], $name as *const () as usize, false)),*]
    };
}

pub(super) fn table() -> Vec<(&'static str, usize, bool)> {
    let mut t = s![
        s_malloc,
        s_free,
        s_calloc,
        s_realloc,
        s_posix_memalign,
        s_mmap,
        s_munmap,
        s_madvise,
        s_memchr,
        s_memcmp,
        s_memcpy,
        s_memset,
        s_strchr,
        s_strrchr,
        s_strcmp,
        s_strncmp,
        s_strlen,
        s_wcslen,
        s_wmemchr,
        s_strtol,
        s_strtoul,
        s_strtoll_l,
        s_strtoull_l,
        s_strtod,
        s_strtof,
        s_strtold_l,
        s_bsearch,
        s_qsort,
        s_strerror,
        s_strerror_r,
        s_vsnprintf,
        s_snprintf,
        s_vasprintf,
        s_vsscanf,
        s_sscanf,
        s_acos,
        s_acosf,
        s_atanf,
        s_cos,
        s_cosf,
        s_exp,
        s_exp2,
        s_exp2f,
        s_expf,
        s_expm1,
        s_log,
        s_log10,
        s_log10f,
        s_log2,
        s_log2f,
        s_logf,
        s_sin,
        s_sinf,
        s_tan,
        s_tanf,
        s_tanhf,
        s_atan2,
        s_atan2f,
        s_hypot,
        s_hypotf,
        s_pow,
        s_powf,
        s_sincos,
        s_sincosf,
        s_newlocale,
        s_freelocale,
        s_uselocale,
        s_localeconv,
        s_iswlower_l,
        s_mbrtowc,
        s_mbrlen,
        s_mbtowc,
        s_wcrtomb,
        s_mbsnrtowcs,
        s_mbsrtowcs,
        s_wcsnrtombs,
    ];
    t.extend([
        ("memmove", s_memcpy as *const () as usize, false),
        ("__memcpy_chk", s_memcpy_chk as *const () as usize, false),
        ("__memset_chk", s_memset_chk as *const () as usize, false),
        ("__strlen_chk", s_strlen_chk as *const () as usize, false),
        ("strtoll", s_strtol as *const () as usize, false),
        ("strtoull", s_strtoul as *const () as usize, false),
        (
            "__vsnprintf_chk",
            s_vsnprintf_chk as *const () as usize,
            false,
        ),
        (
            "__ctype_get_mb_cur_max",
            s_ctype_get_mb_cur_max as *const () as usize,
            false,
        ),
    ]);
    t
}
