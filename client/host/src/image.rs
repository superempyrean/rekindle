//! Where the client's image and this library are mapped, and (on Linux)
//! taking this library out of `LD_PRELOAD`.
//!
//! The client's image is the executable on Windows and Linux. On Android
//! it is a shared object the app loads, named by `REKINDLE_HOST_IMAGE`.

/// The main module's (the client exe's) load address.
#[cfg(windows)]
pub(crate) fn image_base() -> Option<usize> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut std::ffi::c_void;
    }
    // SAFETY: a null name asks for the exe's own module; nothing is freed.
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    (base != 0).then_some(base)
}

/// This library's load address, or 0 if it can't be found.
#[cfg(windows)]
pub(crate) fn module_base() -> usize {
    use std::ffi::c_void;
    const FROM_ADDRESS: u32 = 0x4;
    const UNCHANGED_REFCOUNT: u32 = 0x2;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleExW(flags: u32, module_name: *const u16, module: *mut *mut c_void)
        -> i32;
    }
    let mut handle: *mut c_void = std::ptr::null_mut();
    // SAFETY: FROM_ADDRESS takes any address in the module (this function's
    // own), and UNCHANGED_REFCOUNT leaves nothing to release.
    let ok = unsafe {
        GetModuleHandleExW(
            FROM_ADDRESS | UNCHANGED_REFCOUNT,
            module_base as *const u16,
            &raw mut handle,
        )
    };
    if ok == 0 { 0 } else { handle as usize }
}

/// The main module's (the client exe's) load address.
#[cfg(target_os = "linux")]
pub(crate) fn image_base() -> Option<usize> {
    let exe = std::env::current_exe().ok()?;
    mapped_base(|path| std::path::Path::new(path) == exe)
}

/// The environment variable that names the client's image on Android: the
/// path of the shared object the anchors resolve against.
#[cfg(target_os = "android")]
pub(crate) const IMAGE_ENV: &str = "REKINDLE_HOST_IMAGE";

/// The client's image file: what the anchors resolve against.
#[cfg(not(target_os = "android"))]
pub(crate) fn image_path() -> anyhow::Result<std::path::PathBuf> {
    Ok(std::env::current_exe()?)
}

/// The client's image file: what the anchors resolve against.
#[cfg(target_os = "android")]
pub(crate) fn image_path() -> anyhow::Result<std::path::PathBuf> {
    std::env::var_os(IMAGE_ENV)
        .map(Into::into)
        .ok_or_else(|| anyhow::anyhow!("{IMAGE_ENV} is not set"))
}

/// The client image's load address: where its first mapping, at file
/// offset 0, starts. A library mapped straight out of the APK has none.
#[cfg(target_os = "android")]
pub(crate) fn image_base() -> Option<usize> {
    let image = image_path().ok()?;
    mapped_base(|path| std::path::Path::new(path) == image)
}

/// This library's load address, or 0 if it can't be found.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn module_base() -> usize {
    let Some(path) = own_path() else {
        return 0;
    };
    mapped_base(|p| std::path::Path::new(p) == path).unwrap_or(0)
}

/// Take this library out of `LD_PRELOAD`, so processes the client starts
/// don't load rekindle.
///
/// # Safety
///
/// No other thread reads or writes the environment meanwhile: the library's
/// initializer calls it before the client's `main`, when the process has
/// one thread.
#[cfg(target_os = "linux")]
pub(crate) unsafe fn leave_preload() {
    let Some(list) = std::env::var("LD_PRELOAD").ok() else {
        return;
    };
    let Some(path) = own_path() else {
        return;
    };
    let rest: Vec<&str> = list
        .split([' ', ':'])
        .filter(|p| !p.is_empty() && std::fs::canonicalize(p).ok().as_ref() != Some(&path))
        .collect();
    // SAFETY: the caller's contract.
    unsafe {
        if rest.is_empty() {
            std::env::remove_var("LD_PRELOAD");
        } else {
            std::env::set_var("LD_PRELOAD", rest.join(":"));
        }
    }
}

/// The file this library was mapped from.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn own_path() -> Option<std::path::PathBuf> {
    let me = module_base as *const () as usize;
    maps()
        .into_iter()
        .find(|m| (m.start..m.end).contains(&me))
        .map(|m| m.path.into())
}

/// The first mapping (at file offset 0) of a file `matches` accepts.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn mapped_base(matches: impl Fn(&str) -> bool) -> Option<usize> {
    maps()
        .into_iter()
        .find(|m| m.offset == 0 && matches(&m.path))
        .map(|m| m.start)
}

/// One line of `/proc/self/maps`.
#[cfg(any(target_os = "linux", target_os = "android"))]
struct Mapping {
    start: usize,
    end: usize,
    offset: u64,
    path: String,
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn maps() -> Vec<Mapping> {
    let text = std::fs::read_to_string("/proc/self/maps").unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            let (start, end) = f.next()?.split_once('-')?;
            let offset = u64::from_str_radix(f.nth(1)?, 16).ok()?;
            let path = f.nth(2).unwrap_or_default().to_string();
            Some(Mapping {
                start: usize::from_str_radix(start, 16).ok()?,
                end: usize::from_str_radix(end, 16).ok()?,
                offset,
                path,
            })
        })
        .collect()
}
