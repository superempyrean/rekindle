//! Maps the library, relocates it, binds its imports to the shims through
//! logging trampolines, and runs its `DT_INIT_ARRAY`.

use std::cell::Cell;
use std::ffi::{CStr, CString, c_char};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use anyhow::{Context as _, Result, bail};

use crate::elf::{self, Phdr};
use crate::log::glog;
use crate::sys;

/// The mapped library.
pub(crate) struct Module {
    pub(crate) base: usize,
    pub(crate) name: CString,
    /// The program headers, inside the first `PT_LOAD`.
    pub(crate) phdr: usize,
    pub(crate) phnum: u16,
    symtab: usize,
    strtab: usize,
    nsyms: usize,
    init: Vec<usize>,
}

static MODULE: OnceLock<Module> = OnceLock::new();
static BASE: AtomicUsize = AtomicUsize::new(0);
static SIZE: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn module() -> Option<&'static Module> {
    MODULE.get()
}

pub(crate) fn in_module(a: usize) -> bool {
    let base = BASE.load(Ordering::Relaxed);
    base != 0 && a >= base && a < base + SIZE.load(Ordering::Relaxed)
}

/// An address as the log shows it: `+offset` inside the library.
pub(crate) fn where_(a: usize) -> String {
    if in_module(a) {
        format!("+{:#x}", a - BASE.load(Ordering::Relaxed))
    } else {
        format!("{a:#x}")
    }
}

thread_local! {
    /// The return address of this thread's latest call to an import.
    static CALLER: Cell<usize> = const { Cell::new(0) };
}

/// Where the import being run was called from, as `where_` shows it. Set by
/// its trampoline; a shim the library found with `dlsym` reads a stale one.
pub(crate) fn caller() -> String {
    where_(CALLER.get())
}

struct Import {
    name: &'static str,
    calls: AtomicU64,
    quiet: bool,
}

static IMPORTS: OnceLock<Vec<Import>> = OnceLock::new();

/// Imports called too often to log: counted only.
const QUIET: &[&str] = &[
    "malloc",
    "free",
    "calloc",
    "realloc",
    "posix_memalign",
    "memcpy",
    "memmove",
    "memset",
    "memcmp",
    "memchr",
    "strlen",
    "__memcpy_chk",
    "__memset_chk",
    "__strlen_chk",
    "pthread_mutex_lock",
    "pthread_mutex_unlock",
    "pthread_getspecific",
    "pthread_setspecific",
    "clock_gettime",
    "__errno",
    "strcmp",
    "strncmp",
    "bcmp",
    "sincosf",
    "sinf",
    "cosf",
    "powf",
    "expf",
    "logf",
    "atan2f",
    "hypotf",
    "pow",
    "exp",
    "log",
    "sin",
    "cos",
    "sincos",
    "atan2",
    "hypot",
    "tanf",
    "tan",
    "acosf",
    "atanf",
    "exp2f",
    "log2f",
    "log10f",
    "tanhf",
    "expm1",
    "log2",
    "exp2",
    "log10",
    "acos",
    "pthread_rwlock_rdlock",
    "pthread_rwlock_wrlock",
    "pthread_rwlock_unlock",
    "pthread_cond_broadcast",
    "sched_yield",
    "syscall",
    "wmemchr",
    "mbrtowc",
    "wcrtomb",
    "__stack_chk_fail",
];

/// Called by every import's trampoline before the import runs.
extern "sysv64" fn imp_log(idx: usize, entry_rsp: *const usize, a0: u64, a1: u64) {
    // SAFETY: the trampoline passes its own entry stack pointer, which
    // holds the caller's return address.
    let ret = unsafe { *entry_rsp };
    CALLER.set(ret);
    let Some(im) = IMPORTS.get().and_then(|v| v.get(idx)) else {
        return;
    };
    let n = im.calls.fetch_add(1, Ordering::Relaxed) + 1;
    let level = crate::log::trace();
    if im.quiet || level == 0 || (n > 20 && level < 2) {
        return;
    }
    glog!(
        "[imp] {}({a0:#x}, {a1:#x}) from {} tid={} #{n}\n",
        im.name,
        where_(ret),
        sys::tid()
    );
}

// The import trampoline, copied once per import. The 24 bytes after
// `phoenix_guest_tramp_end` hold {index, target, imp_log}. It saves the
// argument registers (and `al`, the vector count of a variadic call),
// calls imp_log(index, entry rsp, a0, a1), restores them and jumps to the
// target.
core::arch::global_asm!(
    ".text",
    ".p2align 4",
    ".globl phoenix_guest_tramp_begin",
    ".globl phoenix_guest_tramp_end",
    "phoenix_guest_tramp_begin:",
    "push rdi",
    "push rsi",
    "push rdx",
    "push rcx",
    "push r8",
    "push r9",
    "push rax",
    "sub rsp, 0x80",
    "movdqu [rsp], xmm0",
    "movdqu [rsp + 0x10], xmm1",
    "movdqu [rsp + 0x20], xmm2",
    "movdqu [rsp + 0x30], xmm3",
    "movdqu [rsp + 0x40], xmm4",
    "movdqu [rsp + 0x50], xmm5",
    "movdqu [rsp + 0x60], xmm6",
    "movdqu [rsp + 0x70], xmm7",
    "mov rdi, qword ptr [rip + .Lpg_tramp_data]",
    "lea rsi, [rsp + 0xb8]",
    "mov rdx, [rsp + 0xb0]",
    "mov rcx, [rsp + 0xa8]",
    "call qword ptr [rip + .Lpg_tramp_data + 16]",
    "movdqu xmm0, [rsp]",
    "movdqu xmm1, [rsp + 0x10]",
    "movdqu xmm2, [rsp + 0x20]",
    "movdqu xmm3, [rsp + 0x30]",
    "movdqu xmm4, [rsp + 0x40]",
    "movdqu xmm5, [rsp + 0x50]",
    "movdqu xmm6, [rsp + 0x60]",
    "movdqu xmm7, [rsp + 0x70]",
    "add rsp, 0x80",
    "pop rax",
    "pop r9",
    "pop r8",
    "pop rcx",
    "pop rdx",
    "pop rsi",
    "pop rdi",
    "jmp qword ptr [rip + .Lpg_tramp_data + 8]",
    "phoenix_guest_tramp_end:",
    ".Lpg_tramp_data:",
    ".quad 0, 0, 0",
);

unsafe extern "C" {
    static phoenix_guest_tramp_begin: u8;
    static phoenix_guest_tramp_end: u8;
}

fn tramp_code() -> &'static [u8] {
    let (b, e) = (
        &raw const phoenix_guest_tramp_begin,
        &raw const phoenix_guest_tramp_end,
    );
    // SAFETY: both labels are in the template above, `end` after `begin`.
    unsafe { std::slice::from_raw_parts(b, e as usize - b as usize) }
}

fn tramp_stride() -> usize {
    (tramp_code().len() + 24 + 15) & !15
}

/// Executable stubs for imports nothing provides.
struct Stubs {
    page: usize,
    used: usize,
}

const STUB_PAGE: usize = 0x10000;

/// What a stub calls: logs the import and returns 0.
extern "sysv64" fn missing_called(name: *const c_char, ret: usize) -> u64 {
    // SAFETY: the stub passes the import's name, a string in the library.
    let name = unsafe { CStr::from_ptr(name) }.to_string_lossy();
    glog!("[missing] {name} called from {} -> 0\n", where_(ret));
    0
}

impl Stubs {
    fn make(&mut self, name: *const c_char) -> Result<usize> {
        if self.page == 0 {
            self.page = sys::alloc_rw(STUB_PAGE)? as usize;
        }
        if self.used + 32 > STUB_PAGE {
            bail!("too many missing imports");
        }
        let t = self.page + self.used;
        // movabs rdi, name; mov rsi, [rsp]; movabs rax, missing_called; jmp rax
        let mut code = vec![0x48, 0xbf];
        code.extend((name as u64).to_le_bytes());
        code.extend([0x48, 0x8b, 0x34, 0x24, 0x48, 0xb8]);
        code.extend((missing_called as *const () as usize as u64).to_le_bytes());
        code.extend([0xff, 0xe0]);
        // SAFETY: 32 bytes of the RW page are free at `t`.
        unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), t as *mut u8, code.len()) };
        self.used += 32;
        Ok(t)
    }
}

struct Loader {
    base: usize,
    symtab: usize,
    strtab: usize,
    resolved: Vec<Option<usize>>,
    imports: Vec<Import>,
    tramps: usize,
    tramp_used: usize,
    stubs: Stubs,
    relocs: u64,
}

struct Sym {
    name: &'static str,
    name_ptr: *const c_char,
    bind: u8,
    kind: u8,
    shndx: u16,
    value: u64,
}

fn sym_at(symtab: usize, strtab: usize, i: usize) -> Sym {
    let p = symtab + i * 24;
    // SAFETY: `i` is below the symbol count of the mapped dynsym, and its
    // name is a NUL-terminated string in dynstr. The library is never
    // unmapped, so the name lives as long as the process.
    unsafe {
        let st_name = (p as *const u32).read_unaligned();
        let info = *((p + 4) as *const u8);
        let shndx = ((p + 6) as *const u16).read_unaligned();
        let value = ((p + 8) as *const u64).read_unaligned();
        let name_ptr = (strtab + st_name as usize) as *const c_char;
        Sym {
            name: CStr::from_ptr(name_ptr).to_str().unwrap_or("?"),
            name_ptr,
            bind: info >> 4,
            kind: info & 0xf,
            shndx,
            value,
        }
    }
}

impl Loader {
    fn resolve_import(&mut self, s: &Sym) -> Result<usize> {
        if let Some(shim) = sys::shim(s.name) {
            return Ok(shim.addr);
        }
        if let Some(p) = sys::fallback(s.name) {
            return Ok(p);
        }
        if s.bind == elf::STB_WEAK {
            glog!("[load] weak {} -> NULL\n", s.name);
            return Ok(0);
        }
        self.stubs.make(s.name_ptr)
    }

    fn sym_value(&mut self, si: usize) -> Result<usize> {
        if let Some(v) = self.resolved[si] {
            return Ok(v);
        }
        let s = sym_at(self.symtab, self.strtab, si);
        let v = if s.shndx != 0 {
            self.base + s.value as usize
        } else {
            let target = self.resolve_import(&s)?;
            let is_func = s.kind == elf::STT_FUNC || s.kind == elf::STT_NOTYPE;
            let is_data = sys::shim(s.name).is_some_and(|x| x.data);
            if is_func && !is_data && target != 0 {
                self.trampoline(s.name, target)
            } else {
                target
            }
        };
        self.resolved[si] = Some(v);
        Ok(v)
    }

    fn trampoline(&mut self, name: &'static str, target: usize) -> usize {
        let idx = self.imports.len();
        self.imports.push(Import {
            name,
            calls: AtomicU64::new(0),
            quiet: QUIET.contains(&name),
        });
        let code = tramp_code();
        let t = self.tramps + self.tramp_used;
        // SAFETY: the trampoline area has a stride per dynamic symbol, and
        // each symbol gets at most one.
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), t as *mut u8, code.len());
            let data = (t + code.len()) as *mut u64;
            data.write_unaligned(idx as u64);
            data.add(1).write_unaligned(target as u64);
            data.add(2)
                .write_unaligned(imp_log as *const () as usize as u64);
        }
        self.tramp_used += tramp_stride();
        t
    }

    fn apply(&mut self, (off, info, addend): elf::Rela) -> Result<()> {
        let at = (self.base + off as usize) as *mut u64;
        let kind = info as u32;
        let si = (info >> 32) as usize;
        let v = match kind {
            elf::R_X86_64_NONE => return Ok(()),
            elf::R_X86_64_RELATIVE => (self.base as i64).wrapping_add(addend) as u64,
            elf::R_X86_64_GLOB_DAT | elf::R_X86_64_JUMP_SLOT => self.sym_value(si)? as u64,
            elf::R_X86_64_64 => (self.sym_value(si)? as i64).wrapping_add(addend) as u64,
            _ => bail!("unhandled relocation type {kind} at {off:#x}"),
        };
        // SAFETY: relocations write inside the mapped image.
        unsafe { at.write_unaligned(v) };
        self.relocs += 1;
        Ok(())
    }
}

fn round_page(v: u64) -> u64 {
    (v + 0xfff) & !0xfff
}

/// Maps and relocates the library at `path`. Returns the module, ready for
/// [`init`].
#[expect(clippy::too_many_lines, reason = "the steps of a load, in order")]
pub(crate) fn load(path: &Path, patch_gate: bool) -> Result<&'static Module> {
    if MODULE.get().is_some() {
        bail!("the library is already loaded");
    }
    let file = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let hdr = elf::header(&file)?;
    let loads: Vec<Phdr> = hdr
        .phdrs
        .iter()
        .copied()
        .filter(|p| p.kind == elf::PT_LOAD)
        .collect();
    let hi = loads
        .iter()
        .map(|p| p.vaddr + p.memsz)
        .max()
        .context("no PT_LOAD")?;
    let dynph = hdr
        .phdrs
        .iter()
        .find(|p| p.kind == elf::PT_DYNAMIC)
        .context("no PT_DYNAMIC")?;
    let size = round_page(hi) as usize;
    // One more page after the image holds the stack guard on Windows,
    // within rip range of the code.
    let base = sys::alloc_rw(size + 0x1000)? as usize;
    for p in &loads {
        let src = file
            .get(p.offset as usize..(p.offset + p.filesz) as usize)
            .context("a PT_LOAD past the end of the file")?;
        // SAFETY: the segment lies inside the mapping, which is `hi` long.
        unsafe {
            std::ptr::copy_nonoverlapping(
                src.as_ptr(),
                (base + p.vaddr as usize) as *mut u8,
                src.len(),
            );
        }
    }
    BASE.store(base, Ordering::Relaxed);
    SIZE.store(size, Ordering::Relaxed);
    glog!("[load] {} at {base:#x} size {size:#x}\n", path.display());

    let mut d = std::collections::HashMap::new();
    let mut at = base + dynph.vaddr as usize;
    loop {
        // SAFETY: the dynamic section is inside the image, ended by DT_NULL.
        let (tag, val) = unsafe {
            (
                (at as *const i64).read_unaligned(),
                ((at + 8) as *const u64).read_unaligned(),
            )
        };
        if tag == 0 {
            break;
        }
        d.insert(tag, val);
        at += 16;
    }
    let dt = |t: i64| d.get(&t).copied().unwrap_or(0) as usize;
    let symtab = base + dt(elf::DT_SYMTAB);
    let strtab = base + dt(elf::DT_STRTAB);
    let nsyms = (dt(elf::DT_VERSYM)
        .checked_sub(dt(elf::DT_SYMTAB))
        .context("no DT_VERSYM after DT_SYMTAB")?)
        / 24;
    let tramp_len = nsyms * tramp_stride();
    let mut l = Loader {
        base,
        symtab,
        strtab,
        resolved: vec![None; nsyms],
        imports: Vec::new(),
        tramps: sys::alloc_rw(tramp_len)? as usize,
        tramp_used: 0,
        stubs: Stubs { page: 0, used: 0 },
        relocs: 0,
    };
    let image = |off: usize, len: usize| {
        // SAFETY: a range of the mapped image the dynamic section names.
        unsafe { std::slice::from_raw_parts((base + off) as *const u8, len) }
    };
    if d.contains_key(&elf::DT_ANDROID_RELA) {
        let start = dt(elf::DT_ANDROID_RELA);
        let len = d.get(&0x6000_0012).copied().unwrap_or(0) as usize;
        let len = if len == 0 { size - start } else { len };
        elf::aps2(image(start, len), |r| l.apply(r))?;
    }
    if dt(elf::DT_RELA) != 0 {
        for r in elf::rela(image(dt(elf::DT_RELA), dt(elf::DT_RELASZ))) {
            l.apply(r)?;
        }
    }
    if dt(elf::DT_JMPREL) != 0 {
        for r in elf::rela(image(dt(elf::DT_JMPREL), dt(elf::DT_PLTRELSZ))) {
            l.apply(r)?;
        }
    }
    glog!(
        "[load] {} relocations, {} imports trampolined\n",
        l.relocs,
        l.imports.len()
    );

    if patch_gate {
        match patch_access_gate(base, &loads, &hdr.phdrs) {
            Some(at) => glog!("[load] access gate neutralized at {at:#x}\n"),
            None => glog!("[load] access gate not patched: no single match, left as is\n"),
        }
    }

    #[cfg(windows)]
    {
        let slot = base + size;
        let guard = sys::random_u64() & !0xff;
        // SAFETY: the slot page is mapped RW, after the image.
        unsafe { (slot as *mut u64).write(guard) };
        let mut sites = 0;
        for p in loads.iter().filter(|p| p.flags & elf::PF_X != 0) {
            let addr = base + p.vaddr as usize;
            // SAFETY: the segment's file bytes, mapped RW.
            let text =
                unsafe { std::slice::from_raw_parts_mut(addr as *mut u8, p.filesz as usize) };
            sites += elf::patch_stack_guard(text, addr, slot)?;
        }
        glog!("[load] stack guard: {sites} fs:[0x28] reads patched to the slot at {slot:#x}\n");
        sys::protect(slot, 0x1000, elf::PF_R);
    }

    for p in &loads {
        let s = p.vaddr & !0xfff;
        let e = round_page(p.vaddr + p.memsz);
        sys::protect(base + s as usize, (e - s) as usize, p.flags);
    }
    for p in hdr.phdrs.iter().filter(|p| p.kind == elf::PT_GNU_RELRO) {
        let s = p.vaddr & !0xfff;
        let e = (p.vaddr + p.memsz) & !0xfff;
        if e > s {
            sys::protect(base + s as usize, (e - s) as usize, elf::PF_R);
        }
    }
    sys::protect(l.tramps, tramp_len, elf::PF_R | elf::PF_X);
    if l.stubs.page != 0 {
        sys::protect(l.stubs.page, STUB_PAGE, elf::PF_R | elf::PF_X);
    }
    sys::flush_icache();

    let init_at = base + dt(elf::DT_INIT_ARRAY);
    let init = (0..dt(elf::DT_INIT_ARRAYSZ) / 8)
        // SAFETY: the init array is inside the image, relocated.
        .map(|i| unsafe { ((init_at + i * 8) as *const usize).read_unaligned() })
        .collect();
    let _ = IMPORTS.set(l.imports);
    let m = Module {
        base,
        name: CString::new(path.to_string_lossy().as_bytes()).unwrap_or_default(),
        phdr: base + hdr.phoff as usize,
        phnum: hdr.phnum,
        symtab,
        strtab,
        nsyms,
        init,
    };
    Ok(MODULE.get_or_init(|| m))
}

/// Neutralizes the client-access gate in the mapped, relocated image, while
/// its text is still writable. The gate is one function that reads the
/// `2026-11-phoenix-app-access` experiment and returns `Option<bool>`; a
/// selected account gets `Some(true)`. This finds that one function — the
/// experiment string is referenced nowhere else — and overwrites its entry
/// to return `Some(true)` for every account. It patches nothing unless
/// exactly one function is found, so a
/// changed build leaves the gate in place rather than the wrong code patched.
/// Returns the patched function's address. `x86_64` only, which is the only
/// architecture this loader maps.
fn patch_access_gate(base: usize, loads: &[Phdr], phdrs: &[Phdr]) -> Option<usize> {
    // SAFETY: each PT_LOAD's file bytes are mapped at base+vaddr, filesz long,
    // and readable here (text and data are still RW before the protect pass).
    let mem = |p: &Phdr| unsafe {
        std::slice::from_raw_parts((base + p.vaddr as usize) as *const u8, p.filesz as usize)
    };
    let word = |m: &[u8], off: usize| -> Option<usize> {
        m.get(off..off + 8)
            .map(|b| usize::from_le_bytes(b.try_into().unwrap()))
    };

    // The experiment string: in .rodata, which some layouts fold into the
    // read-execute text segment, so every load is searched.
    let mut str_addrs = Vec::new();
    for p in loads {
        let m = mem(p);
        for (off, win) in m.windows(elf::ACCESS_EXPERIMENT.len()).enumerate() {
            if win == elf::ACCESS_EXPERIMENT {
                str_addrs.push(base + p.vaddr as usize + off);
            }
        }
    }
    if str_addrs.is_empty() {
        glog!("[load] access gate: experiment string not found\n");
        return None;
    }

    // The `&str` slots whose pointer is the string and whose length matches.
    let mut slots = Vec::new();
    for p in loads {
        let m = mem(p);
        let mut off = 0;
        while off + 16 <= m.len() {
            if let (Some(ptr), Some(len)) = (word(m, off), word(m, off + 8))
                && str_addrs.contains(&ptr)
                && len == elf::ACCESS_EXPERIMENT.len()
            {
                slots.push(base + p.vaddr as usize + off);
            }
            off += 8;
        }
    }
    // Prefer the `&str` slots; fall back to the string bytes if it is inlined.
    let mut targets = if slots.is_empty() { str_addrs } else { slots };
    targets.sort_unstable();
    targets.dedup();

    let eh = phdrs.iter().find(|p| p.kind == elf::PT_GNU_EH_FRAME)?;
    let starts = elf::eh_frame_starts(mem(eh), base + eh.vaddr as usize);
    if starts.is_empty() {
        glog!("[load] access gate: no usable .eh_frame_hdr\n");
        return None;
    }

    let mut funcs = Vec::new();
    for p in loads.iter().filter(|p| p.flags & elf::PF_X != 0) {
        for ip in elf::lea_rip_refs(mem(p), base + p.vaddr as usize, &targets) {
            if let Some(f) = elf::function_of(&starts, ip) {
                funcs.push(f);
            }
        }
    }
    funcs.sort_unstable();
    funcs.dedup();
    let [func] = funcs[..] else {
        glog!(
            "[load] access gate: {} candidate functions, not patching\n",
            funcs.len()
        );
        return None;
    };

    // `mov al, 1; ret` = Some(true) for Option<bool>. Keep a leading endbr64
    // so an indirect call still lands on a valid target.
    // SAFETY: `func` is an entry inside an executable load, mapped RW now.
    let head = unsafe { std::slice::from_raw_parts(func as *const u8, 4) };
    let at = if head == [0xf3, 0x0f, 0x1e, 0xfa] {
        func + 4
    } else {
        func
    };
    // SAFETY: three bytes at the entry (or just after endbr64), within text.
    unsafe { std::ptr::copy_nonoverlapping([0xb0, 0x01, 0xc3].as_ptr(), at as *mut u8, 3) };
    Some(func)
}

impl Module {
    /// The address of a symbol the library defines.
    pub(crate) fn sym(&self, name: &str) -> Option<usize> {
        (1..self.nsyms)
            .map(|i| sym_at(self.symtab, self.strtab, i))
            .find(|s| s.shndx != 0 && s.name == name)
            .map(|s| self.base + s.value as usize)
    }

    /// Runs `DT_INIT_ARRAY` with bionic's `(argc, argv, envp)`.
    pub(crate) fn init(&self) {
        static ARGS: OnceLock<[usize; 4]> = OnceLock::new();
        let a = ARGS.get_or_init(|| {
            [
                c"phoenix-guest".as_ptr() as usize,
                0,
                c"ANDROID_ROOT=/system".as_ptr() as usize,
                0,
            ]
        });
        for (i, &f) in self.init.iter().enumerate() {
            if f == 0 || f == usize::MAX {
                continue;
            }
            glog!("[init] {i}: +{:#x}\n", f - self.base);
            sys::guest_call(f, [1, &raw const a[0] as u64, &raw const a[2] as u64, 0]);
        }
    }
}

/// Calls to each import so far, in binding order.
pub(crate) fn import_calls() -> Vec<(&'static str, u64)> {
    IMPORTS.get().map_or_else(Vec::new, |v| {
        v.iter()
            .map(|i| (i.name, i.calls.load(Ordering::Relaxed)))
            .filter(|(_, n)| *n > 0)
            .collect()
    })
}

fn stats_text() -> String {
    let mut s = String::from(
        "[stats] import calls:
",
    );
    for (name, n) in import_calls() {
        let _ = writeln!(s, "  {name:<28} {n}");
    }
    s
}

/// Writes the import call counts to the log.
pub(crate) fn import_stats() {
    glog!("{}", stats_text());
}

/// [`import_stats`] for fault handlers, which may run while the log is held.
pub(crate) fn try_import_stats() {
    crate::log::try_write(format_args!("{}", stats_text()));
}
