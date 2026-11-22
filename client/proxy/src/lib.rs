//! The `dwmapi.dll` proxy: the side-load shim.
//!
//! This is the native equivalent of what the web client mods do by swapping
//! `app.asar`: instead of changing a shortcut or the `discord://` handler, it
//! changes what the client loads when it starts, so every launch path picks
//! rekindle up without any of them being touched.
//!
//! The client statically imports a handful of functions from `dwmapi.dll`, and
//! `dwmapi` is not a `KnownDLL`, so Windows resolves it from the application
//! directory first. A copy of this library named `dwmapi.dll`, placed beside
//! the client exe, is therefore mapped by the client's own loader during
//! process init, before `main`. It happens the same way on a Start menu
//! launch, a `discord://` link, autostart, or a bare `Discord.exe`. The shim
//! does two things:
//!
//! 1. It forwards every export to the genuine system `dwmapi.dll`, so the
//!    client's compositor calls behave as before. If the real one can't be
//!    resolved, each export returns a failure HRESULT the client already has
//!    to handle, rather than crashing it.
//! 2. It starts the host ([`host`]): `rekindle_host.dll` from the version
//!    folder of the install (`%LOCALAPPDATA%\rekindle\versions\<v>\`) that
//!    `state.json` picks when the install records this client
//!    ([`rekindle_install::State::launch`]), and otherwise from its own
//!    directory. The host reads its data from the same folder and reports
//!    through the handshake. A version the host downloads while the client
//!    runs is the one the next launch picks.
//!
//! The host comes up on a worker thread, alongside the client's `main`, so the
//! shim also holds the client's startup until the host is ready ([`gate`]).
//! Nothing on disk changes but `state.json`, and the shim beside the exe.
//! Placing it, and placing it again after a client update, is the launcher's
//! job.
//!
//! The launcher's check and the self-tests also run through the shim: they
//! keep the client off the desktop ([`offscreen`]) and read the host's report
//! ([`check`]).

#![cfg(windows)]

mod check;
mod gate;
mod host;
mod iat;
mod offscreen;
mod sys;

use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

use rekindle_install::env;

type Hwnd = *mut c_void;
type Hresult = i32;

/// The `E_FAIL` HRESULT, returned by an export when the real dwmapi can't be
/// found, which doesn't happen on a working Windows install.
const E_FAIL: Hresult = 0x8000_4005_u32 as i32;

/// `s` as a NUL-terminated UTF-16 string.
fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// The system directory, `C:\Windows\System32` on most machines.
fn system_dir() -> Option<PathBuf> {
    let mut buf = [0u16; 320];
    // SAFETY: the buffer holds `buf.len()` units, the size passed.
    let n = unsafe { sys::GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n != 0 && n < buf.len()).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

/// Append `msg` to the file `REKINDLE_PROXY_TRACE` names, if it is set. The
/// launcher's check reads it when the host didn't report.
fn trace(msg: &str) {
    use std::io::Write as _;
    let Ok(path) = std::env::var(env::TRACE) else {
        return;
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        // Diagnostics only: a failed write leaves a shorter trace.
        let _ = writeln!(f, "{t} {msg}");
    }
}

// ---------------------------------------------------------------------------
// Forwarding: resolve the genuine system dwmapi once, lazily.
// ---------------------------------------------------------------------------

/// The real `dwmapi.dll` module handle, loaded from System32 by full path,
/// since loading `dwmapi` by name would find this library again.
static REAL_DWMAPI: AtomicUsize = AtomicUsize::new(0);
static REAL_INIT: Once = Once::new();

fn real_dwmapi() -> *mut c_void {
    REAL_INIT.call_once(|| {
        let Some(path) = system_dir().map(|d| d.join("dwmapi.dll")) else {
            return;
        };
        // SAFETY: the path is NUL-terminated and outlives the call.
        let h = unsafe { sys::LoadLibraryW(wide(path.as_os_str()).as_ptr()) };
        REAL_DWMAPI.store(h as usize, Ordering::Release);
    });
    REAL_DWMAPI.load(Ordering::Acquire) as *mut c_void
}

/// Resolve one export of the genuine dwmapi by name (NUL-terminated).
fn real_proc(name: &[u8]) -> Option<*mut c_void> {
    debug_assert_eq!(name.last(), Some(&0), "name must be NUL-terminated");
    let module = real_dwmapi();
    if module.is_null() {
        return None;
    }
    // SAFETY: `module` is a loaded library and `name` is NUL-terminated.
    let p = unsafe { sys::GetProcAddress(module, name.as_ptr()) };
    (!p.is_null()).then_some(p)
}

/// Define a proxy export that forwards to the same-named function in the genuine
/// dwmapi. The real pointer is cached after first resolution.
macro_rules! forward {
    ($name:ident ( $($arg:ident : $ty:ty),* $(,)? ) -> $ret:ty, $cname:literal, $fallback:expr) => {
        #[unsafe(no_mangle)]
        pub extern "system" fn $name($($arg: $ty),*) -> $ret {
            static SLOT: AtomicUsize = AtomicUsize::new(0);
            let mut p = SLOT.load(Ordering::Acquire);
            if p == 0 {
                match real_proc($cname) {
                    Some(addr) => {
                        SLOT.store(addr as usize, Ordering::Release);
                        p = addr as usize;
                    }
                    None => return $fallback,
                }
            }
            // SAFETY: `p` is the address of the same-named export of the
            // system dwmapi, whose signature is the Win32 one declared here.
            let f: extern "system" fn($($ty),*) -> $ret = unsafe { std::mem::transmute(p) };
            f($($arg),*)
        }
    };
}

// The three exports the mock client imports. Signatures match the Win32
// headers; `HWND` and option structs are opaque pointers here, only passed
// through.
forward!(DwmFlush() -> Hresult, b"DwmFlush\0", E_FAIL);
forward!(
    DwmSetWindowAttribute(hwnd: Hwnd, attr: u32, value: *const c_void, size: u32) -> Hresult,
    b"DwmSetWindowAttribute\0",
    E_FAIL
);
forward!(
    DwmGetCompositionTimingInfo(hwnd: Hwnd, info: *mut c_void) -> Hresult,
    b"DwmGetCompositionTimingInfo\0",
    E_FAIL
);

// ---------------------------------------------------------------------------
// Load time: the initializer and the worker it starts.
// ---------------------------------------------------------------------------

/// How far the worker got: 0 = not yet run, 1 = ran with no host, 2 = ran
/// and the host mapped. The startup gate and the self-test read it.
static STARTED: AtomicUsize = AtomicUsize::new(0);

/// Runs during the loader's initialization of this library, under the loader
/// lock, before the client's `main`.
extern "C" fn proxy_ctor() {
    // Test only: record that the client's loader mapped the shim, then exit.
    if let Ok(out) = std::env::var(check::PROVE_ENV) {
        check::prove_and_exit(&out);
    }
    // Test only: when a check or the self-test drives the real client, its
    // activation imports are pointed at stand-ins before `main` opens a
    // window. If that can't be done, the client ends here rather than risk
    // a visible window on the desktop.
    let driving = std::env::var_os(env::DRIVE).is_some() || std::env::var_os(env::CHECK).is_some();
    trace(&format!("ctor: driving={driving}"));
    if driving {
        match offscreen::install() {
            Ok(n) => trace(&format!("install_offscreen: ok {n} slots")),
            Err(e) => {
                trace(&format!("install_offscreen: err {e}"));
                // SAFETY: ending the process is always allowed.
                unsafe { sys::ExitProcess(3) }
            }
        }
    }
    if host::present() {
        let n = gate::install();
        trace(&format!("startup gate: {n} slots"));
    }
    // Without the worker the shim is a plain dwmapi, which is what a failure
    // here should leave.
    let _ = std::thread::Builder::new()
        .name("rekindle-proxy".into())
        .spawn(worker);
}

#[used]
#[unsafe(link_section = ".CRT$XCU")]
static PROXY_CTOR: extern "C" fn() = proxy_ctor;

fn worker() {
    // Give the loader time to finish mapping the client before loading
    // another library, as the host's own worker does.
    std::thread::sleep(std::time::Duration::from_millis(150));
    let ok = host::start();
    STARTED.store(if ok { 2 } else { 1 }, Ordering::Release);
    trace(if ok {
        "host: loaded"
    } else {
        "host: not loaded"
    });
    if let Ok(path) = std::env::var(check::MARKER_ENV) {
        // The self-test reads it; without it, the test fails.
        let _ = std::fs::write(path, if ok { b"loaded".as_slice() } else { b"nohost" });
    }
    if let Ok(out) = std::env::var(env::CHECK) {
        check::run(&out);
    }
}

/// [`STARTED`], for the self-test.
#[unsafe(no_mangle)]
pub extern "system" fn rekindle_proxy_state() -> u32 {
    STARTED.load(Ordering::Acquire) as u32
}
