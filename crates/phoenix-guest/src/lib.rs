//! Runs the guest compiled into Phoenix's Android `x86_64` host library
//! (`liblibdiscord_wasm_host.so`) on Windows or Linux, with no Android
//! userland.
//!
//! [`load`] maps the library, decodes its packed relocations, binds its
//! bionic imports to shims, and runs its initialisers. [`Library::create`]
//! creates the guest through `ldh_create_linked` with a [`Transport`], and
//! [`Guest::start`] and [`Guest::dispatch`] call `ldh_start` and
//! `ldh_dispatch`. The guest emits from inside those calls, through
//! [`Transport::emit`]. A host CALL is answered before `emit` returns, with
//! [`Reply::send`], which calls `ldh_reply`.
//!
//! On Windows every call into the library runs on a stack committed in
//! full, and the library's `fs:[0x28]` stack guard reads are rewritten to
//! read a slot after the image.
#![cfg(all(target_arch = "x86_64", any(windows, target_os = "linux")))]

mod elf;
#[cfg(any(windows, test))]
mod errno;
mod loader;
mod log;
mod motion;
#[cfg(any(windows, test))]
mod num;
#[cfg(any(windows, test))]
mod printf;
#[cfg(any(windows, test))]
mod scanf;
mod valist;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as sys;
#[cfg(windows)]
mod win;
#[cfg(windows)]
use win as sys;

use std::ffi::{CStr, c_char, c_void};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anyhow::{Context as _, Result, bail};

pub use motion::{Motion, Tick};

/// Which faults the loader's handler reports, then ends the process for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Faults {
    /// Every fault. On Linux, only this mode installs a handler.
    All,
    /// Faults in the library or on a guest stack. For a host process that
    /// handles its own.
    GuestOnly,
}

/// How to load the library.
#[derive(Clone, Debug)]
pub struct Options {
    /// The host library.
    pub so: PathBuf,
    /// The loader's log. `None` logs nothing.
    pub log: Option<PathBuf>,
    /// Windows: the directory the guest's `/` maps to. With none, every
    /// path the guest opens is missing. Linux uses the real file system.
    pub root: Option<PathBuf>,
    /// The guest's environment, as `getenv` answers it.
    pub env: Vec<(String, String)>,
    /// Import logging: 0 off, 1 the first 20 calls of each import, 2 all.
    pub trace: i32,
    pub faults: Faults,
    /// Neutralize the client-access gate: the guest then shows its app to an
    /// account outside the rollout, as it does for a selected one. Off by
    /// default and for the headless oracle; `PHOENIX_PATCH_GATE` in the
    /// environment forces it on regardless.
    pub patch_access_gate: bool,
}

impl Options {
    /// Options with no log and no root, `HOME=/sandbox`, and every fault
    /// reported.
    pub fn new(so: impl Into<PathBuf>) -> Options {
        Options {
            so: so.into(),
            log: None,
            root: None,
            env: vec![
                ("HOME".into(), "/sandbox".into()),
                ("PATH".into(), "/usr/bin:/bin".into()),
            ],
            trace: 1,
            faults: Faults::All,
            patch_access_gate: false,
        }
    }
}

/// The loaded library's C platform API.
pub struct Library {
    create: usize,
    start: usize,
    dispatch: usize,
    reply: usize,
    engines: Option<usize>,
    motion: Option<motion::Syms>,
}

static LIBRARY: OnceLock<Library> = OnceLock::new();

/// Loads and initialises the library, once per process, on the calling
/// thread.
pub fn load(opts: &Options) -> Result<&'static Library> {
    if LIBRARY.get().is_some() {
        bail!("the library is already loaded");
    }
    let file = match &opts.log {
        Some(p) => {
            Some(std::fs::File::create(p).with_context(|| format!("creating {}", p.display()))?)
        }
        None => None,
    };
    log::open(file);
    log::set_trace(opts.trace);
    sys::init(opts);
    let want_gate = opts.patch_access_gate || std::env::var_os("PHOENIX_PATCH_GATE").is_some();
    let m = loader::load(&opts.so, want_gate)?;
    m.init();
    let sym = |n: &str| {
        m.sym(n)
            .with_context(|| format!("the library exports no {n}"))
    };
    let lib = Library {
        create: sym("ldh_create_linked")?,
        start: sym("ldh_start")?,
        dispatch: sym("ldh_dispatch")?,
        reply: sym("ldh_reply")?,
        engines: m.sym("ldh_engines"),
        motion: motion::Syms::find(m),
    };
    log::glog!(
        "[lib] loaded: create={:#x} start={:#x} dispatch={:#x} reply={:#x}\n",
        lib.create,
        lib.start,
        lib.dispatch,
        lib.reply
    );
    Ok(LIBRARY.get_or_init(|| lib))
}

/// The library [`load`] loaded, if it has.
pub fn library() -> Option<&'static Library> {
    LIBRARY.get()
}

/// A line in the loader's log.
pub fn log(line: &str) {
    log::glog!("{line}\n");
}

/// Calls to each import so far, for imports called at least once.
pub fn import_calls() -> Vec<(&'static str, u64)> {
    loader::import_calls()
}

/// Writes the import call counts to the log.
pub fn log_import_stats() {
    loader::import_stats();
}

fn c_text(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        // SAFETY: the library returns NUL-terminated error strings.
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

/// The platform side of the guest's C transport.
pub trait Transport: Send + Sync {
    /// A buffer the guest emits: a render turn (led by a `u32` 0) or a host
    /// frame. A host CALL is answered by `reply.send` before this returns.
    /// Returning false makes the emit an error in the guest.
    fn emit(&self, buf: &[u8], reply: Reply<'_>) -> bool;

    /// The guest's `notify`, called after some host calls.
    fn notify(&self) {}
}

/// The reply slot of one emit.
pub struct Reply<'a> {
    slot: *mut c_void,
    lib: &'a Library,
}

impl Reply<'_> {
    /// Whether the guest passed a slot: only a CALL has one.
    pub fn is_some(&self) -> bool {
        !self.slot.is_null()
    }

    /// Answers the CALL with a REPLY frame, through `ldh_reply`.
    pub fn send(&self, frame: &[u8]) {
        if self.slot.is_null() {
            return;
        }
        sys::guest_call(
            self.lib.reply,
            [
                self.slot as u64,
                frame.as_ptr() as u64,
                frame.len() as u64,
                0,
            ],
        );
    }
}

/// `struct transport { void *ctx; bool (*emit)(...); void (*notify)(void *); }`
#[repr(C)]
struct CTransport {
    ctx: *const c_void,
    emit: extern "sysv64" fn(*const c_void, *const u8, usize, *mut c_void) -> bool,
    notify: extern "sysv64" fn(*const c_void),
}

struct Ctx {
    transport: Arc<dyn Transport>,
    lib: &'static Library,
}

extern "sysv64" fn t_emit(
    ctx: *const c_void,
    buf: *const u8,
    len: usize,
    slot: *mut c_void,
) -> bool {
    // SAFETY: `ctx` is the `Ctx` the guest's handle was created with, which
    // the `Guest` keeps; the guest passes `len` bytes that live for the call.
    let (c, bytes) = unsafe {
        let b = if len == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(buf, len)
        };
        (&*ctx.cast::<Ctx>(), b)
    };
    let reply = Reply { slot, lib: c.lib };
    // A panic must not unwind into the guest's frames.
    std::panic::catch_unwind(AssertUnwindSafe(|| c.transport.emit(bytes, reply))).unwrap_or(true)
}

extern "sysv64" fn t_notify(ctx: *const c_void) {
    // SAFETY: as in `t_emit`.
    let c = unsafe { &*ctx.cast::<Ctx>() };
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| c.transport.notify()));
}

impl Library {
    /// `ldh_engines`: the engines the library was built with.
    pub fn engines(&self) -> Option<String> {
        self.engines
            .map(|f| c_text(sys::guest_call(f, [0; 4]) as *const c_char))
    }

    /// A motion engine, created as the Android platform creates its own.
    /// Use it from the thread that created it.
    pub fn motion(&self) -> Result<Motion> {
        match self.motion {
            Some(s) => Motion::new(s),
            None => bail!("the library lacks an ldh_motion_* export"),
        }
    }

    /// Creates the guest with `ldh_create_linked`.
    pub fn create(&'static self, transport: Arc<dyn Transport>) -> Result<Guest> {
        let ctx = Box::new(Ctx {
            transport,
            lib: self,
        });
        let ct = Box::new(CTransport {
            ctx: (&raw const *ctx).cast(),
            emit: t_emit,
            notify: t_notify,
        });
        let mut err: *const c_char = std::ptr::null();
        let h = sys::guest_call(
            self.create,
            [(&raw const *ct) as u64, (&raw mut err) as u64, 0, 0],
        );
        if h == 0 {
            let e = c_text(err);
            bail!(
                "ldh_create_linked: {}",
                if e.is_empty() { "returned NULL" } else { &e }
            );
        }
        Ok(Guest {
            handle: h,
            lib: self,
            _ctx: ctx,
            _transport: ct,
        })
    }
}

/// The guest. Calls into it are made from one thread at a time.
pub struct Guest {
    handle: u64,
    lib: &'static Library,
    _ctx: Box<Ctx>,
    _transport: Box<CTransport>,
}

impl Guest {
    /// `ldh_start`.
    pub fn start(&self) -> Result<()> {
        let mut err: *const c_char = std::ptr::null();
        let ok = sys::guest_call(self.lib.start, [self.handle, (&raw mut err) as u64, 0, 0]) as u8;
        if !err.is_null() || ok == 0 {
            bail!("ldh_start: {}", c_text(err));
        }
        Ok(())
    }

    /// `ldh_dispatch`: a UI event or host frame.
    pub fn dispatch(&self, buf: &[u8]) -> Result<()> {
        let mut err: *const c_char = std::ptr::null();
        let ok = sys::guest_call(
            self.lib.dispatch,
            [
                self.handle,
                buf.as_ptr() as u64,
                buf.len() as u64,
                (&raw mut err) as u64,
            ],
        ) as u8;
        if !err.is_null() || ok == 0 {
            bail!("ldh_dispatch: {}", c_text(err));
        }
        Ok(())
    }
}

/// The current thread's id, as the log shows it.
pub fn thread_id() -> u64 {
    sys::tid()
}

/// Ends the process at once, without running destructors or exit
/// handlers: threads the guest started may hold locks.
pub fn exit(code: i32) -> ! {
    sys::die(code as _)
}
