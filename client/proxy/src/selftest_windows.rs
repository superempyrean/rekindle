use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
}

type Flush = extern "system" fn() -> i32;
type State = extern "system" fn() -> u32;

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn beside_exe(name: &str) -> Result<PathBuf> {
    let dir = std::env::current_exe()?
        .parent()
        .context("locating this binary's directory")?
        .to_path_buf();
    Ok(dir.join(name))
}

fn load(path: &std::path::Path) -> Result<*mut c_void> {
    // SAFETY: the path is NUL-terminated and outlives the call.
    let h = unsafe { LoadLibraryW(wide(path.as_os_str()).as_ptr()) };
    if h.is_null() {
        bail!("LoadLibraryW failed for {}", path.display());
    }
    Ok(h)
}

fn proc(module: *mut c_void, name: &[u8]) -> Option<*mut c_void> {
    // SAFETY: `module` is a loaded library and `name` is NUL-terminated.
    let p = unsafe { GetProcAddress(module, name.as_ptr()) };
    (!p.is_null()).then_some(p)
}

/// The real system dwmapi, for comparison.
fn real_dwmapi() -> Result<*mut c_void> {
    let mut buf = [0u16; 320];
    // SAFETY: the buffer holds `buf.len()` units, the size passed.
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        bail!("GetSystemDirectoryW failed");
    }
    let mut path = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    path.push("dwmapi.dll");
    load(&path)
}

fn main() -> Result<()> {
    let proxy_path = match std::env::args().nth(1) {
        Some(p) => PathBuf::from(p),
        None => beside_exe("dwmapi.dll")?,
    };
    let dir = proxy_path
        .parent()
        .context("proxy has no parent dir")?
        .to_path_buf();
    if !proxy_path.is_file() {
        bail!("proxy not found: {} (build the crate first)", proxy_path.display());
    }

    // Point the shim's host kick at a stand-in host (the proxy itself: loading
    // it again just returns the existing handle, which is all we need to prove
    // the kick path ran) and have it drop a marker we can read back.
    let marker = std::env::temp_dir().join("rekindle-proxy-selftest.marker");
    let _ = std::fs::remove_file(&marker);
    let name = proxy_path.file_name().context("proxy has no file name")?;
    // SAFETY: on Windows `set_var` is safe from any thread, and this binary
    // is Windows-only.
    unsafe {
        std::env::set_var("REKINDLE_PROXY_DIR", &dir);
        std::env::set_var("REKINDLE_PROXY_HOST_NAME", name);
        std::env::set_var("REKINDLE_PROXY_MARKER", &marker);
    }

    // --- Load the proxy. Its ctor starts the worker thread. ---
    let module = load(&proxy_path).context("loading the proxy")?;
    println!("ok: proxy loaded from {}", proxy_path.display());

    // --- 1. Every imported export is present. ---
    let exports: &[&[u8]] = &[
        b"DwmFlush\0",
        b"DwmSetWindowAttribute\0",
        b"DwmGetCompositionTimingInfo\0",
        b"rekindle_proxy_state\0",
    ];
    for name in exports {
        if proc(module, name).is_none() {
            bail!("missing export: {}", String::from_utf8_lossy(&name[..name.len() - 1]));
        }
    }
    println!("ok: all {} exports present", exports.len());

    // --- 1b. DwmFlush forwards: same HRESULT as the genuine dwmapi. ---
    let real = real_dwmapi().context("loading real dwmapi")?;
    let (Some(proxy_flush), Some(real_flush)) = (proc(module, b"DwmFlush\0"), proc(real, b"DwmFlush\0"))
    else {
        bail!("DwmFlush is missing");
    };
    // SAFETY: both are `DwmFlush`, `HRESULT DwmFlush(void)`.
    let (proxy_flush, real_flush) = unsafe {
        (
            std::mem::transmute::<*mut c_void, Flush>(proxy_flush),
            std::mem::transmute::<*mut c_void, Flush>(real_flush),
        )
    };
    let (via_proxy, via_real) = (proxy_flush(), real_flush());
    if via_proxy != via_real {
        bail!("forwarding mismatch: proxy DwmFlush=0x{via_proxy:08x} real=0x{via_real:08x}");
    }
    println!("ok: DwmFlush forwards (HRESULT 0x{via_proxy:08x} both ways)");

    // --- 2. The host kick fired. Poll the state export. ---
    let state = proc(module, b"rekindle_proxy_state\0").context("no rekindle_proxy_state")?;
    // SAFETY: the shim exports it as `extern "system" fn() -> u32`.
    let state = unsafe { std::mem::transmute::<*mut c_void, State>(state) };
    let mut st = 0;
    for _ in 0..40 {
        st = state();
        if st != 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    match st {
        0 => bail!("host kick never ran (worker did not start)"),
        1 => bail!("host kick ran but no host mapped"),
        2 => println!("ok: host kick fired and the stand-in host mapped"),
        other => bail!("unexpected proxy state {other}"),
    }
    let marker_val = std::fs::read_to_string(&marker).unwrap_or_default();
    if marker_val != "loaded" {
        bail!("marker not written as expected (got {marker_val:?})");
    }
    println!("ok: marker confirms the kick path");

    println!("\nPASS: dwmapi proxy forwards all exports and starts the host");
    Ok(())
}
