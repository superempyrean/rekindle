//! Windows x64: memory, the guest stacks, faults and the shims.
//!
//! The library calls with the System V convention and Windows code uses
//! the Microsoft one, so every shim is `extern "sysv64"`, and every call
//! into the library goes through [`guest_call`]. Code built for Linux may
//! use the 128 bytes below `rsp` and need not touch the pages of a large
//! frame in order, as a Windows stack with a guard page requires, so the
//! library runs on stacks committed in full.

mod libc;
mod process;
mod stdio;
mod thread;

use std::cell::Cell;
use std::ffi::c_void;
use std::fmt::Write as _;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use anyhow::{Result, bail};
use windows_sys::Win32::Foundation::{
    EXCEPTION_ACCESS_VIOLATION, EXCEPTION_BREAKPOINT, EXCEPTION_DATATYPE_MISALIGNMENT,
    EXCEPTION_ILLEGAL_INSTRUCTION, EXCEPTION_IN_PAGE_ERROR, EXCEPTION_INT_DIVIDE_BY_ZERO,
    EXCEPTION_PRIV_INSTRUCTION, EXCEPTION_STACK_OVERFLOW,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_POINTERS, FlushInstructionCache, SEM_FAILCRITICALERRORS,
    SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX, SetErrorMode,
};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
    PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThreadId, TerminateProcess,
};

use crate::elf::{PF_R, PF_W, PF_X};
use crate::loader::{in_module, where_};
use crate::log::glog;
use crate::{Faults, Options};

pub(crate) use process::random_bytes;

pub(crate) fn tid() -> u64 {
    // SAFETY: no preconditions.
    u64::from(unsafe { GetCurrentThreadId() })
}

pub(crate) fn alloc_rw(len: usize) -> Result<*mut u8> {
    // SAFETY: a fresh reservation.
    let p = unsafe {
        VirtualAlloc(
            std::ptr::null(),
            len,
            MEM_RESERVE | MEM_COMMIT,
            PAGE_READWRITE,
        )
    };
    if p.is_null() {
        bail!(
            "VirtualAlloc of {len:#x} bytes failed: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(p.cast())
}

pub(crate) fn protect(addr: usize, len: usize, pf: u32) {
    let (r, w, x) = (pf & PF_R != 0, pf & PF_W != 0, pf & PF_X != 0);
    let prot = match (r, w, x) {
        (_, true, true) => PAGE_EXECUTE_READWRITE,
        (true, false, true) => PAGE_EXECUTE_READ,
        (false, false, true) => PAGE_EXECUTE,
        (_, true, false) => PAGE_READWRITE,
        (true, false, false) => PAGE_READONLY,
        (false, false, false) => PAGE_NOACCESS,
    };
    let mut old = 0;
    // SAFETY: a range the loader mapped.
    if unsafe { VirtualProtect(addr as *const c_void, len, prot, &raw mut old) } == 0 {
        glog!(
            "[load] VirtualProtect {addr:#x}: {}\n",
            std::io::Error::last_os_error()
        );
    }
}

pub(crate) fn flush_icache() {
    // SAFETY: flushing the whole cache of this process.
    unsafe { FlushInstructionCache(GetCurrentProcess(), std::ptr::null(), 0) };
}

pub(crate) fn random_u64() -> u64 {
    let mut b = [0u8; 8];
    random_bytes(&mut b);
    u64::from_le_bytes(b)
}

/// Ends the process at once, after flushing the log and the standard
/// streams. Threads the library started may hold locks, so nothing else
/// runs.
pub(crate) fn die(code: u32) -> ! {
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // SAFETY: ends this process.
    unsafe { TerminateProcess(GetCurrentProcess(), code) };
    unreachable!()
}

// ---------- per-thread state ----------

pub(crate) const NKEYS: usize = 256;

/// What the shims keep per thread: the library's errno, its pthread keys,
/// its `thread_local` destructors, the thread's guest stack and locale.
pub(crate) struct TState {
    pub(crate) err: i32,
    pub(crate) on_guest_stack: bool,
    pub(crate) keys: [usize; NKEYS],
    pub(crate) dtors: Vec<(usize, usize)>,
    pub(crate) stack: Option<Stack>,
    pub(crate) locale: usize,
}

thread_local! {
    static TS: Cell<*mut TState> = const { Cell::new(std::ptr::null_mut()) };
}

/// This thread's state. Fields are read and written through the pointer,
/// briefly, since a shim may call back into the library.
pub(crate) fn ts() -> *mut TState {
    TS.with(|c| {
        if c.get().is_null() {
            c.set(Box::into_raw(Box::new(TState {
                err: 0,
                on_guest_stack: false,
                keys: [0; NKEYS],
                dtors: Vec::new(),
                stack: None,
                locale: 0,
            })));
        }
        c.get()
    })
}

fn ts_peek() -> *mut TState {
    TS.try_with(Cell::get).unwrap_or(std::ptr::null_mut())
}

/// Frees this thread's state, once nothing will run on its guest stack.
pub(crate) fn ts_free() {
    let p = TS.with(|c| c.replace(std::ptr::null_mut()));
    if !p.is_null() {
        // SAFETY: allocated by `ts` on this thread, and no longer in use.
        let t = unsafe { Box::from_raw(p) };
        if let Some(s) = t.stack {
            s.free();
        }
    }
}

pub(crate) fn set_errno(e: i32) {
    // SAFETY: this thread's state.
    unsafe { (*ts()).err = e };
}

pub(crate) fn errno() -> i32 {
    // SAFETY: this thread's state.
    unsafe { (*ts()).err }
}

// ---------- guest stacks ----------

/// A stack committed in full, with a no-access page below it.
pub(crate) struct Stack {
    alloc: usize,
    pub(crate) lo: usize,
    pub(crate) hi: usize,
}

impl Stack {
    pub(crate) fn new(size: usize) -> Stack {
        let size = (size + 0xffff) & !0xffff;
        let Ok(p) = alloc_rw(size + 0x1000) else {
            glog!(
                "[stack] VirtualAlloc {size:#x} failed: {}\n",
                std::io::Error::last_os_error()
            );
            die(70);
        };
        protect(p as usize, 0x1000, 0);
        let lo = p as usize + 0x1000;
        Stack {
            alloc: p as usize,
            lo,
            hi: lo + size,
        }
    }

    fn free(self) {
        // SAFETY: the reservation `new` made.
        unsafe { VirtualFree(self.alloc as *mut c_void, 0, MEM_RELEASE) };
    }
}

/// `run_on_stack(body, arg, top, limit)`: switches `rsp` to `top` and the
/// TEB's `StackBase` and `StackLimit` to `top` and `limit`, calls `body(arg)`,
/// and switches back.
#[unsafe(naked)]
pub(crate) extern "sysv64" fn run_on_stack(
    body: extern "sysv64" fn(usize),
    arg: usize,
    top: usize,
    limit: usize,
) {
    core::arch::naked_asm!(
        "push rbp",
        "mov rbp, rsp",
        "push rbx",
        "push r12",
        "mov rax, qword ptr gs:[0x08]",
        "push rax",
        "mov rax, qword ptr gs:[0x10]",
        "push rax",
        "mov qword ptr gs:[0x08], rdx",
        "mov qword ptr gs:[0x10], rcx",
        "mov rbx, rsp",
        "mov rsp, rdx",
        "and rsp, -16",
        "mov rax, rdi",
        "mov rdi, rsi",
        "call rax",
        "mov rsp, rbx",
        "pop rax",
        "mov qword ptr gs:[0x10], rax",
        "pop rax",
        "mov qword ptr gs:[0x08], rax",
        "pop r12",
        "pop rbx",
        "pop rbp",
        "ret",
    )
}

/// The stack of the threads that call into the library.
const MAIN_STACK: usize = 64 << 20;

struct GCall {
    f: usize,
    a: [u64; 6],
    x: [f32; 8],
    ret: u64,
}

extern "sysv64" fn gcall_body(c: usize) {
    let c = c as *mut GCall;
    // SAFETY: `guest_call_f` passes its record; `f` is a System V function
    // of the library that takes up to six integer and eight `f32`
    // arguments. Integers and floats fill separate registers, so the ones
    // it doesn't take are ignored.
    unsafe {
        let f: extern "sysv64" fn(
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
        ) -> u64 = std::mem::transmute((*c).f);
        let (a, x) = ((*c).a, (*c).x);
        (*c).ret = f(
            a[0], a[1], a[2], a[3], a[4], a[5], x[0], x[1], x[2], x[3], x[4], x[5], x[6], x[7],
        );
    }
}

/// Calls a function of the library on this thread's guest stack. A call
/// made from inside one (a shim or a transport callback) runs where it is.
pub(crate) fn guest_call(f: usize, a: [u64; 4]) -> u64 {
    guest_call_f(f, [a[0], a[1], a[2], a[3], 0, 0], [0.0; 8])
}

/// [`guest_call`] with six integer arguments and eight `f32` ones, which
/// go in `xmm0` to `xmm7`.
pub(crate) fn guest_call_f(f: usize, a: [u64; 6], x: [f32; 8]) -> u64 {
    let mut c = GCall { f, a, x, ret: 0 };
    let t = ts();
    // SAFETY: this thread's state, touched only between calls.
    unsafe {
        if (*t).on_guest_stack {
            gcall_body(&raw mut c as usize);
            return c.ret;
        }
        let (lo, hi) = {
            let s = (*t).stack.get_or_insert_with(|| Stack::new(MAIN_STACK));
            (s.lo, s.hi)
        };
        (*t).on_guest_stack = true;
        run_on_stack(gcall_body, &raw mut c as usize, hi, lo);
        (*t).on_guest_stack = false;
    }
    c.ret
}

// bionic's setjmp and longjmp, for the library's own jmp_buf. They save
// the System V callee-saved registers.
#[unsafe(naked)]
extern "sysv64" fn s_setjmp(_buf: usize) -> i32 {
    core::arch::naked_asm!(
        "mov [rdi], rbx",
        "mov [rdi + 0x08], rbp",
        "mov [rdi + 0x10], r12",
        "mov [rdi + 0x18], r13",
        "mov [rdi + 0x20], r14",
        "mov [rdi + 0x28], r15",
        "lea rdx, [rsp + 8]",
        "mov [rdi + 0x30], rdx",
        "mov rdx, [rsp]",
        "mov [rdi + 0x38], rdx",
        "xor eax, eax",
        "ret",
    )
}

#[unsafe(naked)]
extern "sysv64" fn s_longjmp(_buf: usize, _v: i32) {
    core::arch::naked_asm!(
        "mov eax, esi",
        "test eax, eax",
        "jnz 2f",
        "inc eax",
        "2:",
        "mov rbx, [rdi]",
        "mov rbp, [rdi + 0x08]",
        "mov r12, [rdi + 0x10]",
        "mov r13, [rdi + 0x18]",
        "mov r14, [rdi + 0x20]",
        "mov r15, [rdi + 0x28]",
        "mov rsp, [rdi + 0x30]",
        "jmp qword ptr [rdi + 0x38]",
    )
}

// ---------- faults ----------

static GUEST_ONLY: AtomicBool = AtomicBool::new(false);
static IN_FAULT: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn on_fault(ep: *mut EXCEPTION_POINTERS) -> i32 {
    const CONTINUE_SEARCH: i32 = 0;
    // SAFETY: Windows passes valid exception pointers.
    let (rec, ctx) = unsafe { (&*(*ep).ExceptionRecord, &*(*ep).ContextRecord) };
    let code = rec.ExceptionCode;
    let fatal = [
        EXCEPTION_ACCESS_VIOLATION,
        EXCEPTION_ILLEGAL_INSTRUCTION,
        EXCEPTION_PRIV_INSTRUCTION,
        EXCEPTION_INT_DIVIDE_BY_ZERO,
        EXCEPTION_STACK_OVERFLOW,
        EXCEPTION_BREAKPOINT,
        EXCEPTION_IN_PAGE_ERROR,
        EXCEPTION_DATATYPE_MISALIGNMENT,
        0xC000_0409u32 as i32,
    ];
    if !fatal.contains(&code) {
        return CONTINUE_SEARCH;
    }
    let rip = ctx.Rip as usize;
    if GUEST_ONLY.load(Ordering::Relaxed) {
        let t = ts_peek();
        // SAFETY: this thread's state, if it has one.
        let on_guest = !t.is_null() && unsafe { (*t).on_guest_stack };
        if !in_module(rip) && !on_guest {
            return CONTINUE_SEARCH;
        }
    }
    if IN_FAULT.swap(1, Ordering::SeqCst) != 0 {
        die(129);
    }
    let addr = if rec.NumberParameters >= 2 {
        rec.ExceptionInformation[1]
    } else {
        0
    };
    let mut s = format!(
        "[fault] exception {:#x} addr={addr:#x} rip={rip:#x} ({}) rsp={:#x} tid={}\n",
        code as u32,
        where_(rip),
        ctx.Rsp,
        tid()
    );
    s.push_str("[fault] stack words that point into the module:");
    let sp = ctx.Rsp as usize;
    for i in 0..256 {
        let at = sp + i * 8;
        if !process::readable(at, 8) {
            break;
        }
        // SAFETY: checked readable just above.
        let w = unsafe { *(at as *const usize) };
        if in_module(w) {
            let _ = write!(s, " {}", where_(w));
        }
    }
    s.push('\n');
    crate::log::try_write(format_args!("{s}"));
    crate::loader::try_import_stats();
    die(139);
}

// ---------- setup ----------

static ROOT: OnceLock<std::path::PathBuf> = OnceLock::new();
static ENV: OnceLock<Vec<(String, std::ffi::CString)>> = OnceLock::new();

pub(crate) fn root() -> Option<&'static std::path::Path> {
    ROOT.get().map(std::path::PathBuf::as_path)
}

pub(crate) fn env() -> &'static [(String, std::ffi::CString)] {
    ENV.get().map_or(&[], Vec::as_slice)
}

pub(crate) fn init(opts: &Options) {
    // SAFETY: no preconditions.
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX)
    };
    process::init_clock();
    if let Some(r) = &opts.root {
        let _ = ROOT.set(r.clone());
    }
    let _ = ENV.set(
        opts.env
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    std::ffi::CString::new(v.as_str()).unwrap_or_default(),
                )
            })
            .collect(),
    );
    match root() {
        Some(r) => glog!("[shims] root {}\n", r.display()),
        None => glog!("[shims] root (none: every path is missing)\n"),
    }
    GUEST_ONLY.store(opts.faults == Faults::GuestOnly, Ordering::Relaxed);
    // SAFETY: registers a handler that lives as long as the process.
    unsafe { AddVectoredExceptionHandler(1, Some(on_fault)) };
}

// ---------- the table ----------

/// A shim for an import: a function, or a variable (`data`).
#[derive(Clone, Copy)]
pub(crate) struct Shim {
    pub(crate) addr: usize,
    pub(crate) data: bool,
}

macro_rules! f {
    ($name:literal, $f:expr) => {
        ($name, $f as *const () as usize, false)
    };
}

fn table() -> &'static [(&'static str, usize, bool)] {
    static T: OnceLock<Vec<(&'static str, usize, bool)>> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = vec![
            ("__sF", stdio::fake_sf(), true),
            ("stderr", stdio::fake_stderr(), true),
            f!("setjmp", s_setjmp),
            f!("longjmp", s_longjmp),
        ];
        t.extend(libc::table());
        t.extend(stdio::table());
        t.extend(thread::table());
        t.extend(process::table());
        t
    })
}

pub(crate) fn shim(name: &str) -> Option<Shim> {
    table()
        .iter()
        .find(|(n, ..)| *n == name)
        .map(|&(_, addr, data)| Shim { addr, data })
}

/// Windows has no other source of symbols: every import is a shim or a
/// stub.
pub(crate) fn fallback(_name: &str) -> Option<usize> {
    None
}
