//! Finding the host and loading it, off the loader lock.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use rekindle_install::{Install, env};

use crate::{sys, trace, wide};

/// The host library this shim loads, unless `REKINDLE_PROXY_HOST_NAME`
/// names another (the self-test points it at a stand-in).
const HOST_DLL: &str = rekindle_install::HOST_DLL;
const HOST_NAME_ENV: &str = "REKINDLE_PROXY_HOST_NAME";
/// Set by the dev self-test, which drives the client itself and wants the
/// host's self-check mode, so the shim leaves [`env::INSTALLED`] unset.
const SELFTEST_ENV: &str = "REKINDLE_PROXY_SELFTEST";

/// The host's module handle once [`start`] mapped it.
static HOST: AtomicUsize = AtomicUsize::new(0);

/// Where the host and its data load from.
struct HostDir {
    dir: PathBuf,
    /// The install's version, when `state.json` picked the folder.
    version: Option<String>,
}

static HOST_DIR: OnceLock<Option<HostDir>> = OnceLock::new();

/// The file name of the host to load.
pub(crate) fn host_name() -> String {
    std::env::var(HOST_NAME_ENV).unwrap_or_else(|_| HOST_DLL.into())
}

/// The host's module handle, or null before [`start`] mapped it.
pub(crate) fn module() -> *mut c_void {
    HOST.load(Ordering::Acquire) as *mut c_void
}

/// Whether there is a host to load, so the client's startup should wait for it.
pub(crate) fn present() -> bool {
    host_dir().is_some_and(|d| file_exists(&d.dir.join(host_name())))
}

/// Load the host once. Returns whether the host DLL mapped.
pub(crate) fn start() -> bool {
    let Some(HostDir { dir, version }) = host_dir() else {
        return false;
    };
    let host = dir.join(host_name());
    if !host.is_file() {
        // No host: the shim is a plain dwmapi, and the client runs as if it
        // weren't here.
        return false;
    }
    seed_env(dir);
    if let Some(v) = version {
        // SAFETY: on Windows `set_var` is safe from any thread, and the shim
        // is Windows-only.
        unsafe { std::env::set_var(env::HOST_VERSION, v) };
    }
    // SAFETY: the path is NUL-terminated and outlives the call.
    let handle = unsafe { sys::LoadLibraryW(wide(host.as_os_str()).as_ptr()) };
    HOST.store(handle as usize, Ordering::Release);
    !handle.is_null()
}

/// The folder to load the host (and its data) from, picked once: the one
/// `REKINDLE_PROXY_DIR` names (the launcher's check, the self-tests), else
/// the version `state.json` picks when the install records this client,
/// else beside this DLL.
fn host_dir() -> Option<&'static HostDir> {
    HOST_DIR
        .get_or_init(|| {
            if let Some(dir) = std::env::var_os(env::HOST_DIR) {
                return Some(HostDir {
                    dir: PathBuf::from(dir),
                    version: None,
                });
            }
            if let Some(found) = from_install() {
                return Some(found);
            }
            Some(HostDir {
                dir: self_path()?.parent()?.to_path_buf(),
                version: None,
            })
        })
        .as_ref()
}

/// The version folder of the install that records this client, picked and
/// recorded as the one this launch tries.
fn from_install() -> Option<HostDir> {
    let install = Install::new(Install::default_root().ok()?);
    if !install.is_installed() {
        return None;
    }
    let mut state = install.state().ok()?;
    if !state.records_client(&main_exe_path()?) {
        return None;
    }
    let picked = state.launch(|v| install.version_dir(v).join(HOST_DLL).is_file());
    if let Err(e) = install.save(&state) {
        trace(&format!("install: saving state.json failed: {e:#}"));
    }
    let Some(v) = picked else {
        trace("install: no version to load");
        install.log("no version of rekindle to load; the client runs without it");
        return None;
    };
    trace(&format!("install: version {v}"));
    Some(HostDir {
        dir: install.version_dir(&v),
        version: Some(v),
    })
}

/// Point the host at the recovered data in `dir`, unless the launcher set
/// the variables already, and mark a real launch.
fn seed_env(dir: &Path) {
    for (file, var) in [
        ("layout.json", env::LAYOUT),
        ("anchors.json", env::ANCHORS),
        ("palette.json", env::PALETTE),
        ("wire.json", env::WIRE_MAP),
    ] {
        let path = dir.join(file);
        if std::env::var_os(var).is_none() && path.is_file() {
            // SAFETY: on Windows `set_var` is safe from any thread, and the
            // shim is Windows-only.
            unsafe { std::env::set_var(var, path) };
        }
    }
    // A launch through the loader is a real launch: the user's plugins, no
    // self-checks, attached for good. The dev self-test drives the client
    // itself and wants the self-check mode.
    if std::env::var_os(env::INSTALLED).is_none() && std::env::var_os(SELFTEST_ENV).is_none() {
        // SAFETY: as above.
        unsafe { std::env::set_var(env::INSTALLED, "1") };
    }
}

/// This library's own path, found through its own code address.
pub(crate) fn self_path() -> Option<PathBuf> {
    let mut handle: *mut c_void = std::ptr::null_mut();
    // SAFETY: with FROM_ADDRESS the name argument is an address inside this
    // module, and `handle` is an out-pointer.
    let ok = unsafe {
        sys::GetModuleHandleExW(
            sys::GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                | sys::GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            self_path as *const u16,
            &raw mut handle,
        )
    };
    if ok == 0 {
        return None;
    }
    module_file(handle)
}

/// The full path of the process's exe, the client this shim was loaded into.
pub(crate) fn main_exe_path() -> Option<PathBuf> {
    module_file(std::ptr::null_mut())
}

/// The file `module` was loaded from; a null module is the exe.
fn module_file(module: *mut c_void) -> Option<PathBuf> {
    let mut buf = [0u16; 1024];
    // SAFETY: the buffer holds `buf.len()` units, the size passed.
    let n = unsafe { sys::GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n != 0 && n < buf.len()).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

/// Whether `path` exists, without going through `std::fs`.
pub(crate) fn file_exists(path: &Path) -> bool {
    // SAFETY: the path is NUL-terminated and outlives the call.
    unsafe {
        sys::GetFileAttributesW(wide(path.as_os_str()).as_ptr()) != sys::INVALID_FILE_ATTRIBUTES
    }
}
