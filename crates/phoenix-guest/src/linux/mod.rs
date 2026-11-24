//! Linux `x86_64` with glibc: most imports resolve to glibc. The shims cover
//! what differs between bionic and glibc, and the network, which is
//! refused. The stack guard at `fs:0x28` is where glibc keeps it, and the
//! library's thread locals go through its own emutls on glibc's pthread
//! keys.
#![expect(
    clippy::undocumented_unsafe_blocks,
    reason = "every shim dereferences the pointers the library passes, under the contract of the bionic function it stands in for"
)]

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::fmt::Write as _;
use std::sync::OnceLock;

use anyhow::{Result, bail};

use crate::elf::{PF_R, PF_W, PF_X};
use crate::loader::{caller, import_stats, in_module, module, where_};
use crate::log::glog;
use crate::valist::{VaList, returns_f128, variadic};
use crate::{Faults, Options};

unsafe extern "C" {
    static mut stdin: *mut libc::FILE;
    static mut stdout: *mut libc::FILE;
    static mut stderr: *mut libc::FILE;
    fn vfprintf(f: *mut libc::FILE, fmt: *const c_char, ap: *mut VaList) -> c_int;
    fn vsnprintf(buf: *mut c_char, n: usize, fmt: *const c_char, ap: *mut VaList) -> c_int;
    fn fputwc(c: u32, f: *mut libc::FILE) -> u32;
    fn getwc(f: *mut libc::FILE) -> u32;
    fn ungetwc(c: u32, f: *mut libc::FILE) -> u32;
    fn __xpg_strerror_r(e: c_int, buf: *mut c_char, n: usize) -> c_int;
}

pub(crate) fn tid() -> u64 {
    unsafe { libc::gettid() as u64 }
}

pub(crate) fn alloc_rw(len: usize) -> Result<*mut u8> {
    let p = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    if p == libc::MAP_FAILED {
        bail!(
            "mmap of {len:#x} bytes failed: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(p.cast())
}

pub(crate) fn protect(addr: usize, len: usize, pf: u32) {
    let mut prot = 0;
    if pf & PF_R != 0 {
        prot |= libc::PROT_READ;
    }
    if pf & PF_W != 0 {
        prot |= libc::PROT_WRITE;
    }
    if pf & PF_X != 0 {
        prot |= libc::PROT_EXEC;
    }
    if unsafe { libc::mprotect(addr as *mut c_void, len, prot) } != 0 {
        glog!(
            "[load] mprotect {addr:#x}: {}\n",
            std::io::Error::last_os_error()
        );
    }
}

pub(crate) fn flush_icache() {}

/// Calls a function of the library with up to four integer arguments.
pub(crate) fn guest_call(f: usize, a: [u64; 4]) -> u64 {
    let f: extern "sysv64" fn(u64, u64, u64, u64) -> u64 = unsafe { std::mem::transmute(f) };
    f(a[0], a[1], a[2], a[3])
}

/// Calls a function of the library with up to six integer and eight `f32`
/// arguments, which fill separate registers.
pub(crate) fn guest_call_f(f: usize, a: [u64; 6], x: [f32; 8]) -> u64 {
    type F = extern "sysv64" fn(
        u64,
        u64,
        u64,
        u64,
        u64,
        u64,
        f32,
        f32,
        f32,
        f32,
        f32,
        f32,
        f32,
        f32,
    ) -> u64;
    let f: F = unsafe { std::mem::transmute(f) };
    f(
        a[0], a[1], a[2], a[3], a[4], a[5], x[0], x[1], x[2], x[3], x[4], x[5], x[6], x[7],
    )
}

// ---------- faults ----------

extern "C" fn on_fault(sig: c_int, si: *mut libc::siginfo_t, uc: *mut c_void) {
    let uc = unsafe { &*uc.cast::<libc::ucontext_t>() };
    let rip = uc.uc_mcontext.gregs[libc::REG_RIP as usize] as usize;
    let rsp = uc.uc_mcontext.gregs[libc::REG_RSP as usize] as usize;
    let addr = unsafe { (*si).si_addr() } as usize;
    let mut s = format!(
        "[fault] signal {sig} addr={addr:#x} rip={rip:#x} ({}) rsp={rsp:#x} tid={}\n",
        where_(rip),
        tid()
    );
    s.push_str("[fault] stack words that point into the module:");
    for i in 0..256 {
        let w = unsafe { *((rsp + i * 8) as *const usize) };
        if in_module(w) {
            let _ = write!(s, " {}", where_(w));
        }
    }
    s.push('\n');
    crate::log::try_write(format_args!("{s}"));
    crate::loader::try_import_stats();
    unsafe { libc::_exit(128 + sig) };
}

fn install_fault_handlers() {
    unsafe {
        let size = 1 << 16;
        let ss = libc::stack_t {
            ss_sp: Box::leak(vec![0u8; size].into_boxed_slice())
                .as_mut_ptr()
                .cast(),
            ss_flags: 0,
            ss_size: size,
        };
        libc::sigaltstack(&raw const ss, std::ptr::null_mut());
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_fault as *const () as usize;
        sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
        for sig in [
            libc::SIGSEGV,
            libc::SIGBUS,
            libc::SIGILL,
            libc::SIGFPE,
            libc::SIGABRT,
        ] {
            libc::sigaction(sig, &raw const sa, std::ptr::null_mut());
        }
    }
}

static ENV: OnceLock<Vec<(String, CString)>> = OnceLock::new();

pub(crate) fn init(opts: &Options) {
    let _ = ENV.set(
        opts.env
            .iter()
            .map(|(k, v)| (k.clone(), CString::new(v.as_str()).unwrap_or_default()))
            .collect(),
    );
    if opts.faults == Faults::All {
        install_fault_handlers();
    }
}

/// Ends the process at once.
pub(crate) fn die(code: i32) -> ! {
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    unsafe {
        libc::fflush(std::ptr::null_mut());
        libc::_exit(code)
    }
}

// ---------- stdio ----------

/// bionic's `__sF[3]`: `FILE` is 152 bytes on LP64. Pointers into it map
/// to glibc's standard streams.
static FAKE_SF: [u8; 3 * 152] = [0; 3 * 152];

fn fake_sf() -> usize {
    (&raw const FAKE_SF) as usize
}

fn tf(f: *mut libc::FILE) -> *mut libc::FILE {
    let (a, base) = (f as usize, fake_sf());
    if (base..base + 3 * 152).contains(&a) {
        unsafe {
            match (a - base) / 152 {
                0 => stdin,
                1 => stdout,
                _ => stderr,
            }
        }
    } else {
        f
    }
}

extern "sysv64" fn s_vfprintf(f: *mut libc::FILE, fmt: *const c_char, ap: *mut VaList) -> c_int {
    unsafe { vfprintf(tf(f), fmt, ap) }
}
variadic!(s_fprintf, 2, "rdx", s_vfprintf);

extern "sysv64" fn s_fwrite(p: *const c_void, s: usize, n: usize, f: *mut libc::FILE) -> usize {
    unsafe { libc::fwrite(p, s, n, tf(f)) }
}
extern "sysv64" fn s_fread(p: *mut c_void, s: usize, n: usize, f: *mut libc::FILE) -> usize {
    unsafe { libc::fread(p, s, n, tf(f)) }
}
extern "sysv64" fn s_fputc(c: c_int, f: *mut libc::FILE) -> c_int {
    unsafe { libc::fputc(c, tf(f)) }
}
extern "sysv64" fn s_fputwc(c: u32, f: *mut libc::FILE) -> u32 {
    unsafe { fputwc(c, tf(f)) }
}
extern "sysv64" fn s_fflush(f: *mut libc::FILE) -> c_int {
    unsafe { libc::fflush(if f.is_null() { f } else { tf(f) }) }
}
extern "sysv64" fn s_getc(f: *mut libc::FILE) -> c_int {
    unsafe { libc::fgetc(tf(f)) }
}
extern "sysv64" fn s_ungetc(c: c_int, f: *mut libc::FILE) -> c_int {
    unsafe { libc::ungetc(c, tf(f)) }
}
extern "sysv64" fn s_getwc(f: *mut libc::FILE) -> u32 {
    unsafe { getwc(tf(f)) }
}
extern "sysv64" fn s_ungetwc(c: u32, f: *mut libc::FILE) -> u32 {
    unsafe { ungetwc(c, tf(f)) }
}

// ---------- bionic-only ----------

fn lossy(s: *const c_char) -> String {
    if s.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned()
    }
}

extern "sysv64" fn s_android_log_write(
    prio: c_int,
    tag: *const c_char,
    text: *const c_char,
) -> c_int {
    glog!("[alog {prio} {}] {}\n", lossy(tag), lossy(text));
    1
}
extern "sysv64" fn s_android_set_abort_message(msg: *const c_char) {
    glog!("[abort-message] {}\n", lossy(msg));
}
extern "sysv64" fn s_system_property_get(name: *const c_char, value: *mut c_char) -> c_int {
    glog!("[prop] {} -> \"\" from {}\n", lossy(name), caller());
    unsafe { *value = 0 };
    0
}
extern "sysv64" fn s_assert2(
    file: *const c_char,
    line: c_int,
    f: *const c_char,
    msg: *const c_char,
) {
    glog!(
        "[assert] {}:{line} {}: {}\n",
        lossy(file),
        lossy(f),
        lossy(msg)
    );
    unsafe { libc::abort() };
}
extern "sysv64" fn s_strlen_chk(s: *const c_char, _n: usize) -> usize {
    unsafe { libc::strlen(s) }
}
extern "sysv64" fn s_write_chk(fd: c_int, b: *const c_void, c: usize, _bs: usize) -> isize {
    unsafe { libc::write(fd, b, c) }
}
extern "sysv64" fn s_strerror_r(e: c_int, buf: *mut c_char, n: usize) -> c_int {
    unsafe { __xpg_strerror_r(e, buf, n) }
}

/// `sysconf`, with bionic's `_SC_*` numbering.
extern "sysv64" fn s_sysconf(name: c_int) -> i64 {
    let r = match name {
        0x06 => unsafe { libc::sysconf(libc::_SC_CLK_TCK) },
        0x0b => unsafe { libc::sysconf(libc::_SC_OPEN_MAX) },
        0x27 | 0x28 => unsafe { libc::sysconf(libc::_SC_PAGESIZE) },
        0x60 => unsafe { libc::sysconf(libc::_SC_NPROCESSORS_CONF) },
        0x61 => unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) },
        0x62 => unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) },
        0x63 => unsafe { libc::sysconf(libc::_SC_AVPHYS_PAGES) },
        _ => {
            unsafe { *libc::__errno_location() = libc::EINVAL };
            -1
        }
    };
    glog!("[sysconf] bionic {name:#x} -> {r} from {}\n", caller());
    r
}

extern "sysv64" fn strtold_bits(s: *const c_char, end: *mut *mut c_char, _l: usize) -> u128 {
    crate::valist::f64_to_f128(unsafe { libc::strtod(s, end) })
}
returns_f128!(s_strtold_l, strtold_bits);

// ---------- the dynamic linker ----------

/// Reports the library first, so its own unwinder finds its
/// `.eh_frame_hdr`, then glibc's modules.
extern "sysv64" fn s_dl_iterate_phdr(
    cb: Option<unsafe extern "C" fn(*mut libc::dl_phdr_info, usize, *mut c_void) -> c_int>,
    data: *mut c_void,
) -> c_int {
    if let (Some(m), Some(f)) = (module(), cb) {
        let mut info: libc::dl_phdr_info = unsafe { std::mem::zeroed() };
        info.dlpi_addr = m.base as u64;
        info.dlpi_name = m.name.as_ptr();
        info.dlpi_phdr = m.phdr as *const libc::Elf64_Phdr;
        info.dlpi_phnum = m.phnum;
        info.dlpi_adds = 1;
        let r = unsafe { f(&raw mut info, size_of::<libc::dl_phdr_info>(), data) };
        if r != 0 {
            return r;
        }
    }
    unsafe { libc::dl_iterate_phdr(cb, data) }
}

extern "sysv64" fn s_dladdr(addr: *const c_void, info: *mut libc::Dl_info) -> c_int {
    if let Some(m) = module().filter(|_| in_module(addr as usize)) {
        unsafe {
            info.write(std::mem::zeroed());
            (*info).dli_fname = m.name.as_ptr();
            (*info).dli_fbase = m.base as *mut c_void;
        }
        return 1;
    }
    unsafe { libc::dladdr(addr, info) }
}

extern "sysv64" fn s_dlopen(name: *const c_char, flags: c_int) -> usize {
    let n = if name.is_null() {
        "(null)".into()
    } else {
        lossy(name)
    };
    glog!("[dlopen] {n} flags={flags:#x} from {} -> NULL\n", caller());
    0
}

extern "sysv64" fn s_dlsym(h: usize, name: *const c_char) -> usize {
    let n = lossy(name);
    let mut p = shim(&n).map_or(0, |s| s.addr);
    if p == 0 && h == 0 {
        p = fallback(&n).unwrap_or(0);
    }
    glog!("[dlsym] {h:#x} {n} -> {p:#x} from {}\n", caller());
    p
}

extern "sysv64" fn s_dlerror() -> *const c_char {
    c"rekindle loader: not available".as_ptr()
}

extern "sysv64" fn s_dlclose(_h: usize) -> c_int {
    0
}

// ---------- the network: refused ----------

fn deny() -> c_int {
    unsafe { *libc::__errno_location() = libc::EACCES };
    -1
}

extern "sysv64" fn s_socket(d: c_int, t: c_int, p: c_int) -> c_int {
    glog!("[net] socket({d}, {t:#x}, {p}) DENIED from {}\n", caller());
    deny()
}
extern "sysv64" fn s_getaddrinfo(
    n: *const c_char,
    s: *const c_char,
    _h: *const c_void,
    res: *mut usize,
) -> c_int {
    glog!(
        "[net] getaddrinfo({}, {}) DENIED from {}\n",
        lossy(n),
        lossy(s),
        caller()
    );
    unsafe { *res = 0 };
    4 // bionic's EAI_FAIL
}
extern "sysv64" fn s_freeaddrinfo(_a: usize) {}
extern "sysv64" fn s_gai_strerror(_e: c_int) -> *const c_char {
    c"denied by the loader".as_ptr()
}
extern "sysv64" fn s_android_setsocknetwork(_net: u64, _fd: c_int) -> c_int {
    deny()
}
extern "sysv64" fn s_syscall(n: i64, a: i64, b: i64, c: i64, d: i64, e: i64, f: i64) -> i64 {
    if n == libc::SYS_socket || n == libc::SYS_connect {
        glog!("[net] syscall {n} DENIED from {}\n", caller());
        return i64::from(deny());
    }
    unsafe { libc::syscall(n, a, b, c, d, e, f) }
}

// ---------- the process ----------

extern "sysv64" fn s_abort() {
    glog!("[abort] from {}\n", caller());
    import_stats();
    unsafe { libc::abort() };
}
extern "sysv64" fn s_exit(c: c_int) {
    glog!("[exit] {c} from {}\n", caller());
    import_stats();
    die(c);
}
extern "sysv64" fn s_exit_now(c: c_int) {
    glog!("[_exit] {c} from {}\n", caller());
    die(c);
}
extern "sysv64" fn s_pthread_create(
    t: *mut libc::pthread_t,
    a: *const libc::pthread_attr_t,
    f: extern "C" fn(*mut c_void) -> *mut c_void,
    arg: *mut c_void,
) -> c_int {
    let r = unsafe { libc::pthread_create(t, a, f, arg) };
    glog!(
        "[thread] create start={} arg={arg:?} -> {r}\n",
        where_(f as usize)
    );
    r
}
extern "sysv64" fn s_fopen(p: *const c_char, m: *const c_char) -> *mut libc::FILE {
    let f = unsafe { libc::fopen(p, m) };
    glog!(
        "[file] fopen({}, {}) -> {f:?} from {}\n",
        lossy(p),
        lossy(m),
        caller()
    );
    f
}
extern "sysv64" fn s_open(p: *const c_char, fl: c_int, mode: c_int) -> c_int {
    let r = unsafe { libc::open(p, fl, mode) };
    glog!(
        "[file] open({}, {fl:#x}) -> {r} from {}\n",
        lossy(p),
        caller()
    );
    r
}
extern "sysv64" fn s_openat(dfd: c_int, p: *const c_char, fl: c_int, mode: c_int) -> c_int {
    let r = unsafe { libc::openat(dfd, p, fl, mode) };
    glog!(
        "[file] openat({dfd}, {}, {fl:#x}) -> {r} from {}\n",
        lossy(p),
        caller()
    );
    r
}
extern "sysv64" fn s_open_2(p: *const c_char, fl: c_int) -> c_int {
    s_open(p, fl, 0)
}

/// The library's environment: only what the options give it.
extern "sysv64" fn s_getenv(n: *const c_char) -> *const c_char {
    let name = lossy(n);
    let v = ENV
        .get()
        .and_then(|e| e.iter().find(|(k, _)| *k == name))
        .map(|(_, v)| v);
    glog!(
        "[env] getenv({name}) -> {}\n",
        v.map_or_else(|| "(null)".into(), |v| v.to_string_lossy().into_owned())
    );
    v.map_or(std::ptr::null(), |v| v.as_ptr())
}

extern "sysv64" fn vsyslog_imp(pri: c_int, fmt: *const c_char, ap: *mut VaList) {
    let mut buf = vec![0u8; 4096];
    unsafe { vsnprintf(buf.as_mut_ptr().cast(), buf.len(), fmt, ap) };
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    glog!("[syslog {pri}] {}\n", String::from_utf8_lossy(&buf[..end]));
}
variadic!(s_syslog, 2, "rdx", vsyslog_imp);
extern "sysv64" fn s_openlog(_i: *const c_char, _o: c_int, _f: c_int) {}
extern "sysv64" fn s_closelog() {}

// ---------- the table ----------

/// A shim for an import: a function, or a variable (`data`).
#[derive(Clone, Copy)]
pub(crate) struct Shim {
    pub(crate) addr: usize,
    pub(crate) data: bool,
}

macro_rules! s {
    ($($name:ident),* $(,)?) => {
        vec![$((&stringify!($name)[2..], $name as *const () as usize, false)),*]
    };
}

fn table() -> &'static [(&'static str, usize, bool)] {
    static T: OnceLock<Vec<(&'static str, usize, bool)>> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = vec![
            ("__sF", fake_sf(), true),
            ("stderr", (&raw const stderr) as usize, true),
            (
                "__errno",
                libc::__errno_location as *const () as usize,
                false,
            ),
            (
                "__android_log_write",
                s_android_log_write as *const () as usize,
                false,
            ),
            (
                "__system_property_get",
                s_system_property_get as *const () as usize,
                false,
            ),
            ("__assert2", s_assert2 as *const () as usize, false),
            ("__strlen_chk", s_strlen_chk as *const () as usize, false),
            ("__write_chk", s_write_chk as *const () as usize, false),
            ("__open_2", s_open_2 as *const () as usize, false),
            ("_exit", s_exit_now as *const () as usize, false),
        ];
        t.extend(s![
            s_fprintf,
            s_vfprintf,
            s_fwrite,
            s_fread,
            s_fputc,
            s_fputwc,
            s_fflush,
            s_getc,
            s_ungetc,
            s_getwc,
            s_ungetwc,
            s_android_set_abort_message,
            s_strerror_r,
            s_sysconf,
            s_strtold_l,
            s_dl_iterate_phdr,
            s_dladdr,
            s_dlopen,
            s_dlsym,
            s_dlerror,
            s_dlclose,
            s_socket,
            s_getaddrinfo,
            s_freeaddrinfo,
            s_gai_strerror,
            s_android_setsocknetwork,
            s_syscall,
            s_abort,
            s_exit,
            s_pthread_create,
            s_fopen,
            s_open,
            s_openat,
            s_getenv,
            s_syslog,
            s_openlog,
            s_closelog,
        ]);
        t
    })
}

pub(crate) fn shim(name: &str) -> Option<Shim> {
    table()
        .iter()
        .find(|(n, ..)| *n == name)
        .map(|&(_, addr, data)| Shim { addr, data })
}

/// Never taken from glibc: anything that could open a connection by another
/// name, and the library's optional hooks, which stay NULL as on Android.
const DENIED: &[&str] = &["connect", "sdallocx"];

/// glibc's symbol for an import there is no shim for.
pub(crate) fn fallback(name: &str) -> Option<usize> {
    if DENIED.contains(&name) {
        return None;
    }
    let c = CString::new(name).ok()?;
    let p = unsafe { libc::dlsym(libc::RTLD_DEFAULT, c.as_ptr()) };
    (!p.is_null()).then_some(p as usize)
}
