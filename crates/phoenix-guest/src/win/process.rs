//! Time, the environment, the process, `syscall`, the dynamic linker, and
//! the network, which is refused.
#![expect(
    clippy::undocumented_unsafe_blocks,
    reason = "every shim dereferences the pointers the library passes, under the contract of the bionic function it stands in for"
)]

use std::ffi::{c_char, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_READONLY,
    PAGE_READWRITE, VirtualQuery,
};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThread, GetProcessTimes, GetThreadTimes,
    SetThreadDescription,
};

use super::libc::{cbytes, cstr_lossy, vformat};
use super::thread::{Timespec, cpus, sys_futex};
use super::{die, set_errno, ts};
use crate::errno::{EACCES, EINVAL, ENOSYS};
use crate::loader::{caller, import_stats, in_module, module};
use crate::log::glog;
use crate::valist::{VaList, variadic};

pub(crate) fn random_bytes(b: &mut [u8]) {
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
    };
    unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            b.as_mut_ptr(),
            b.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
}

/// Whether `len` bytes at `at` can be read, for the fault handler.
pub(super) fn readable(at: usize, len: usize) -> bool {
    let mut mi: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        VirtualQuery(
            at as *const c_void,
            &raw mut mi,
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    } == 0
    {
        return false;
    }
    let ok = mi.State == MEM_COMMIT
        && mi.Protect
            & (PAGE_READWRITE | PAGE_READONLY | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE)
            != 0;
    ok && at + len <= mi.BaseAddress as usize + mi.RegionSize
}

// ---------- time ----------

static QPF: OnceLock<i64> = OnceLock::new();

pub(super) fn init_clock() {
    QPF.get_or_init(|| {
        let mut f = 0;
        unsafe { QueryPerformanceFrequency(&raw mut f) };
        f
    });
}

pub(super) fn now_mono_ns() -> i64 {
    let mut c = 0;
    unsafe { QueryPerformanceCounter(&raw mut c) };
    let f = *QPF.get().unwrap_or(&10_000_000);
    (c / f) * 1_000_000_000 + (c % f) * 1_000_000_000 / f
}

pub(super) fn now_real_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}

fn ft(f: FILETIME) -> i64 {
    (i64::from(f.dwHighDateTime) << 32) | i64::from(f.dwLowDateTime)
}

/// Kernel and user time of the process, or of this thread, in 100 ns units.
fn cpu_times(thread: bool) -> (i64, i64) {
    let z = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut c, mut e, mut k, mut u) = (z, z, z, z);
    unsafe {
        if thread {
            GetThreadTimes(
                GetCurrentThread(),
                &raw mut c,
                &raw mut e,
                &raw mut k,
                &raw mut u,
            );
        } else {
            GetProcessTimes(
                GetCurrentProcess(),
                &raw mut c,
                &raw mut e,
                &raw mut k,
                &raw mut u,
            );
        }
    }
    (ft(k), ft(u))
}

fn put_ns(t: *mut Timespec, ns: i64) {
    unsafe {
        (*t).sec = ns.div_euclid(1_000_000_000);
        (*t).nsec = ns.rem_euclid(1_000_000_000);
    }
}

extern "sysv64" fn s_clock_gettime(clk: i32, t: *mut Timespec) -> i32 {
    match clk {
        0 | 5 | 8 => put_ns(t, now_real_ns()),
        1 | 4 | 6 | 7 | 9 => put_ns(t, now_mono_ns()),
        2 | 3 => {
            let (k, u) = cpu_times(clk == 3);
            put_ns(t, (k + u) * 100);
        }
        _ => {
            set_errno(EINVAL);
            return -1;
        }
    }
    0
}

extern "sysv64" fn s_clock_nanosleep(
    clk: i32,
    flags: i32,
    req: *const Timespec,
    _rem: *mut Timespec,
) -> i32 {
    let mut ns = unsafe { (*req).sec * 1_000_000_000 + (*req).nsec };
    if flags & 1 != 0 {
        let mut now = Timespec { sec: 0, nsec: 0 };
        s_clock_gettime(clk, &raw mut now);
        ns -= now.sec * 1_000_000_000 + now.nsec;
    }
    if ns > 0 {
        std::thread::sleep(Duration::from_nanos(ns as u64));
    }
    0
}

extern "sysv64" fn s_time(t: *mut i64) -> i64 {
    let s = now_real_ns() / 1_000_000_000;
    if !t.is_null() {
        unsafe { *t = s };
    }
    s
}

/// bionic's `struct rusage`: two timevals, then fourteen longs.
#[repr(C)]
struct Rusage {
    utime: [i64; 2],
    stime: [i64; 2],
    rest: [i64; 14],
}

extern "sysv64" fn s_getrusage(who: i32, r: *mut Rusage) -> i32 {
    let (k, u) = cpu_times(who == 1);
    let tv = |t: i64| [t / 10_000_000, t % 10_000_000 / 10];
    unsafe {
        r.write(Rusage {
            utime: tv(u),
            stime: tv(k),
            rest: [0; 14],
        });
    }
    0
}

/// bionic's `struct tm`.
#[repr(C)]
struct Tm {
    sec: i32,
    min: i32,
    hour: i32,
    mday: i32,
    mon: i32,
    year: i32,
    wday: i32,
    yday: i32,
    isdst: i32,
    gmtoff: i64,
    zone: *const c_char,
}

const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `strftime` in the C locale.
fn strftime(fmt: &[u8], t: &Tm) -> Vec<u8> {
    let mut o = String::new();
    let day = DAYS.get(t.wday as usize).copied().unwrap_or("?");
    let mon = MONTHS.get(t.mon as usize).copied().unwrap_or("?");
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' || i + 1 == fmt.len() {
            o.push(fmt[i] as char);
            i += 1;
            continue;
        }
        i += 1;
        while matches!(fmt.get(i), Some(b'E' | b'O')) {
            i += 1;
        }
        let Some(&c) = fmt.get(i) else { break };
        i += 1;
        let h12 = if t.hour % 12 == 0 { 12 } else { t.hour % 12 };
        let piece = match c {
            b'a' => day[..3].to_string(),
            b'A' => day.to_string(),
            b'b' | b'h' => mon[..3].to_string(),
            b'B' => mon.to_string(),
            b'c' => format!(
                "{} {} {:2} {:02}:{:02}:{:02} {}",
                &day[..3],
                &mon[..3],
                t.mday,
                t.hour,
                t.min,
                t.sec,
                t.year + 1900
            ),
            b'C' => format!("{:02}", (t.year + 1900) / 100),
            b'd' => format!("{:02}", t.mday),
            b'D' | b'x' => format!(
                "{:02}/{:02}/{:02}",
                t.mon + 1,
                t.mday,
                (t.year + 1900) % 100
            ),
            b'e' => format!("{:2}", t.mday),
            b'F' => format!("{}-{:02}-{:02}", t.year + 1900, t.mon + 1, t.mday),
            b'H' => format!("{:02}", t.hour),
            b'I' => format!("{h12:02}"),
            b'j' => format!("{:03}", t.yday + 1),
            b'k' => format!("{:2}", t.hour),
            b'l' => format!("{h12:2}"),
            b'm' => format!("{:02}", t.mon + 1),
            b'M' => format!("{:02}", t.min),
            b'n' => "
"
            .into(),
            b'p' => if t.hour < 12 { "AM" } else { "PM" }.into(),
            b'R' => format!("{:02}:{:02}", t.hour, t.min),
            b'S' => format!("{:02}", t.sec),
            b't' => "	".into(),
            b'T' | b'X' => format!("{:02}:{:02}:{:02}", t.hour, t.min, t.sec),
            b'u' => format!("{}", if t.wday == 0 { 7 } else { t.wday }),
            b'w' => format!("{}", t.wday),
            b'y' => format!("{:02}", (t.year + 1900) % 100),
            b'Y' => format!("{}", t.year + 1900),
            b'z' => {
                let m = t.gmtoff / 60;
                format!(
                    "{}{:02}{:02}",
                    if m < 0 { '-' } else { '+' },
                    m.abs() / 60,
                    m.abs() % 60
                )
            }
            b'Z' => cstr_lossy(t.zone),
            b'%' => "%".into(),
            c => format!("%{}", c as char),
        };
        o.push_str(&piece);
    }
    o.into_bytes()
}

extern "sysv64" fn s_strftime_l(
    s: *mut u8,
    max: usize,
    fmt: *const c_char,
    t: *const Tm,
    _l: usize,
) -> usize {
    let out = strftime(cbytes(fmt), unsafe { &*t });
    if out.len() + 1 > max {
        return 0;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(out.as_ptr(), s, out.len());
        *s.add(out.len()) = 0;
    }
    out.len()
}

// ---------- bionic-only and the process ----------

extern "sysv64" fn s_errno() -> *mut i32 {
    unsafe { &raw mut (*ts()).err }
}

extern "sysv64" fn s_android_log_write(prio: i32, tag: *const c_char, text: *const c_char) -> i32 {
    glog!(
        "[alog {prio} {}] {}\n",
        String::from_utf8_lossy(cbytes(tag)),
        String::from_utf8_lossy(cbytes(text))
    );
    1
}

extern "sysv64" fn s_android_set_abort_message(msg: *const c_char) {
    glog!("[abort-message] {}\n", cstr_lossy(msg));
}

extern "sysv64" fn s_system_property_get(name: *const c_char, value: *mut u8) -> i32 {
    glog!("[prop] {} -> \"\" from {}\n", cstr_lossy(name), caller());
    unsafe { *value = 0 };
    0
}

extern "sysv64" fn s_abort() {
    glog!("[abort] from {}\n", caller());
    import_stats();
    die(134);
}

extern "sysv64" fn s_assert2(file: *const c_char, line: i32, f: *const c_char, msg: *const c_char) {
    glog!(
        "[assert] {}:{line} {}: {}\n",
        cstr_lossy(file),
        cstr_lossy(f),
        cstr_lossy(msg)
    );
    import_stats();
    die(134);
}

extern "sysv64" fn s_stack_chk_fail() {
    glog!("[stack-chk-fail] from {}\n", caller());
    import_stats();
    die(134);
}

extern "sysv64" fn s_exit(c: i32) {
    glog!("[exit] {c} from {}\n", caller());
    import_stats();
    die(c as u32);
}

extern "sysv64" fn s_exit_now(c: i32) {
    glog!("[_exit] {c} from {}\n", caller());
    die(c as u32);
}

extern "sysv64" fn s_cxa_atexit(_f: usize, _arg: usize, _dso: usize) -> i32 {
    0
}

extern "sysv64" fn s_cxa_finalize(_dso: usize) {}

extern "sysv64" fn s_register_atfork(_a: usize, _b: usize, _c: usize, _d: usize) -> i32 {
    0
}

extern "sysv64" fn s_getenv(n: *const c_char) -> *const c_char {
    let name = cbytes(n);
    let v = super::env()
        .iter()
        .find(|(k, _)| k.as_bytes() == name)
        .map(|(_, v)| v);
    glog!(
        "[env] getenv({}) -> {}\n",
        String::from_utf8_lossy(name),
        v.map_or_else(|| "(null)".into(), |v| v.to_string_lossy().into_owned())
    );
    v.map_or(std::ptr::null(), |v| v.as_ptr())
}

extern "sysv64" fn s_getpid() -> i32 {
    unsafe { GetCurrentProcessId() as i32 }
}

extern "sysv64" fn s_gettid() -> i32 {
    super::tid() as i32
}

extern "sysv64" fn s_prctl(opt: i32, a: usize, _b: usize, _c: usize, _d: usize) -> i32 {
    if opt == 15 {
        // PR_SET_NAME
        let name = String::from_utf8_lossy(cbytes(a as *const c_char)).into_owned();
        let w: Vec<u16> = name.encode_utf16().take(31).chain([0]).collect();
        unsafe { SetThreadDescription(GetCurrentThread(), w.as_ptr()) };
        let short: String = name.chars().take(16).collect();
        glog!("[thread] name {short} tid={}\n", super::tid());
        return 0;
    }
    glog!("[prctl] {opt} unsupported from {}\n", caller());
    set_errno(EINVAL);
    -1
}

/// `sysconf`, with bionic's `_SC_*` numbering.
extern "sysv64" fn s_sysconf(name: i32) -> i64 {
    let mem = || {
        let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        unsafe { GlobalMemoryStatusEx(&raw mut m) };
        m
    };
    let r = match name {
        0x06 => 100,
        0x0b => 1024,
        0x27 | 0x28 => 4096,
        0x60 | 0x61 => i64::from(cpus()),
        0x62 => (mem().ullTotalPhys / 4096) as i64,
        0x63 => (mem().ullAvailPhys / 4096) as i64,
        _ => {
            set_errno(EINVAL);
            -1
        }
    };
    glog!("[sysconf] bionic {name:#x} -> {r} from {}\n", caller());
    r
}

extern "sysv64" fn s_getrandom(buf: *mut u8, n: usize, _flags: u32) -> i64 {
    if n > 0 {
        random_bytes(unsafe { std::slice::from_raw_parts_mut(buf, n) });
    }
    n as i64
}

/// `syscall(2)`: the futex, getrandom, the ids, and the network calls,
/// refused as on Linux. Anything else is logged and fails with ENOSYS.
extern "sysv64" fn s_syscall(n: i64, a: i64, b: i64, c: i64, d: i64, _e: i64, _f: i64) -> i64 {
    static SEEN: [AtomicBool; 512] = [const { AtomicBool::new(false) }; 512];
    if let Some(s) = usize::try_from(n).ok().and_then(|i| SEEN.get(i))
        && !s.swap(true, Ordering::Relaxed)
    {
        glog!("[syscall] {n} first call from {}\n", caller());
    }
    let r = match n {
        202 => sys_futex(
            a as *const AtomicU32,
            b as i32,
            c as u32,
            d as *const Timespec,
        ),
        318 => return s_getrandom(a as *mut u8, b as usize, c as u32),
        186 => return super::tid() as i64,
        39 => return i64::from(s_getpid()),
        41..=47 | 49..=55 | 288 | 299 | 307 => {
            glog!("[net] syscall {n} DENIED from {}\n", caller());
            set_errno(EACCES);
            return -1;
        }
        _ => {
            glog!(
                "[syscall] {n}({a:#x}, {b:#x}, {c:#x}) unsupported from {}\n",
                caller()
            );
            -i64::from(ENOSYS)
        }
    };
    if r < 0 {
        set_errno(-r as i32);
        return -1;
    }
    r
}

extern "sysv64" fn vsyslog_imp(pri: i32, fmt: *const c_char, ap: *mut VaList) {
    let b = vformat(fmt, ap);
    glog!("[syslog {pri}] {}\n", String::from_utf8_lossy(&b));
}
variadic!(s_syslog, 2, "rdx", vsyslog_imp);

extern "sysv64" fn s_openlog(_i: *const c_char, _o: i32, _f: i32) {}

extern "sysv64" fn s_closelog() {}

// ---------- the dynamic linker ----------

/// bionic's `struct dl_phdr_info`.
#[repr(C)]
struct PhdrInfo {
    addr: usize,
    name: *const c_char,
    phdr: usize,
    phnum: u16,
    adds: u64,
    subs: u64,
    tls_modid: usize,
    tls_data: usize,
}

type PhdrCb = extern "sysv64" fn(*mut PhdrInfo, usize, *mut c_void) -> i32;

/// Reports the library, the only module there is.
extern "sysv64" fn s_dl_iterate_phdr(cb: PhdrCb, data: *mut c_void) -> i32 {
    let Some(m) = module() else { return 0 };
    let mut info = PhdrInfo {
        addr: m.base,
        name: m.name.as_ptr(),
        phdr: m.phdr,
        phnum: m.phnum,
        adds: 1,
        subs: 0,
        tls_modid: 0,
        tls_data: 0,
    };
    cb(&raw mut info, size_of::<PhdrInfo>(), data)
}

#[repr(C)]
struct DlInfo {
    fname: *const c_char,
    fbase: usize,
    sname: *const c_char,
    saddr: usize,
}

extern "sysv64" fn s_dladdr(addr: usize, info: *mut DlInfo) -> i32 {
    unsafe {
        info.write(DlInfo {
            fname: std::ptr::null(),
            fbase: 0,
            sname: std::ptr::null(),
            saddr: 0,
        });
    }
    match module() {
        Some(m) if in_module(addr) => {
            unsafe {
                (*info).fname = m.name.as_ptr();
                (*info).fbase = m.base;
            }
            1
        }
        _ => 0,
    }
}

extern "sysv64" fn s_dlopen(name: *const c_char, flags: i32) -> usize {
    glog!(
        "[dlopen] {} flags={flags:#x} from {} -> NULL\n",
        cstr_lossy(name),
        caller()
    );
    0
}

extern "sysv64" fn s_dlsym(h: usize, name: *const c_char) -> usize {
    let n = String::from_utf8_lossy(cbytes(name));
    let p = match super::shim(&n) {
        Some(s) if !s.data && n != "sdallocx" => s.addr,
        _ => 0,
    };
    glog!("[dlsym] {h:#x} {n} -> {p:#x} from {}\n", caller());
    p
}

extern "sysv64" fn s_dlerror() -> *const c_char {
    c"rekindle loader: not available".as_ptr()
}

extern "sysv64" fn s_dlclose(_h: usize) -> i32 {
    0
}

// ---------- the network: refused ----------

extern "sysv64" fn s_socket(d: i32, t: i32, p: i32) -> i32 {
    glog!("[net] socket({d}, {t:#x}, {p}) DENIED from {}\n", caller());
    set_errno(EACCES);
    -1
}

macro_rules! deny {
    ($($name:ident),*) => {$(
        extern "sysv64" fn $name() -> i64 {
            glog!(concat!("[net] ", stringify!($name), " DENIED from {}\n"), caller());
            set_errno(EACCES);
            -1
        }
    )*};
}
deny!(
    connect,
    bind,
    getsockname,
    getsockopt,
    setsockopt,
    recvfrom,
    recvmsg,
    sendmsg,
    sendto
);

extern "sysv64" fn s_getaddrinfo(
    n: *const c_char,
    s: *const c_char,
    _h: usize,
    res: *mut usize,
) -> i32 {
    glog!(
        "[net] getaddrinfo({}, {}) DENIED from {}\n",
        String::from_utf8_lossy(cbytes(n)),
        String::from_utf8_lossy(cbytes(s)),
        caller()
    );
    unsafe { *res = 0 };
    4 // bionic's EAI_FAIL
}

extern "sysv64" fn s_freeaddrinfo(_a: usize) {}

extern "sysv64" fn s_gai_strerror(_e: i32) -> *const c_char {
    c"denied by the loader".as_ptr()
}

extern "sysv64" fn s_android_setsocknetwork(_net: u64, _fd: i32) -> i32 {
    set_errno(EACCES);
    -1
}

// Linux event and timer descriptors: not provided.
macro_rules! nosys {
    ($($name:ident),*) => {$(
        extern "sysv64" fn $name() -> i64 {
            glog!(concat!("[io] ", stringify!($name), " unsupported from {}\n"), caller());
            set_errno(ENOSYS);
            -1
        }
    )*};
}
nosys!(
    epoll_create1,
    epoll_ctl,
    epoll_wait,
    eventfd,
    timerfd_create,
    timerfd_settime,
    poll
);

macro_rules! s {
    ($($name:ident),* $(,)?) => {
        vec![$((&stringify!($name)[2..], $name as *const () as usize, false)),*]
    };
}
macro_rules! plain {
    ($($name:ident),* $(,)?) => {
        vec![$((stringify!($name), $name as *const () as usize, false)),*]
    };
}

pub(super) fn table() -> Vec<(&'static str, usize, bool)> {
    let mut t = s![
        s_clock_gettime,
        s_clock_nanosleep,
        s_time,
        s_getrusage,
        s_strftime_l,
        s_android_set_abort_message,
        s_abort,
        s_exit,
        s_getenv,
        s_getpid,
        s_gettid,
        s_prctl,
        s_sysconf,
        s_getrandom,
        s_syscall,
        s_syslog,
        s_openlog,
        s_closelog,
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
    ];
    t.extend(plain![
        connect,
        bind,
        getsockname,
        getsockopt,
        setsockopt,
        recvfrom,
        recvmsg,
        sendmsg,
        sendto,
        epoll_create1,
        epoll_ctl,
        epoll_wait,
        eventfd,
        timerfd_create,
        timerfd_settime,
        poll,
    ]);
    t.extend([
        ("__errno", s_errno as *const () as usize, false),
        ("_exit", s_exit_now as *const () as usize, false),
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
        (
            "__stack_chk_fail",
            s_stack_chk_fail as *const () as usize,
            false,
        ),
        ("__cxa_atexit", s_cxa_atexit as *const () as usize, false),
        (
            "__cxa_finalize",
            s_cxa_finalize as *const () as usize,
            false,
        ),
        (
            "__register_atfork",
            s_register_atfork as *const () as usize,
            false,
        ),
    ]);
    t
}
