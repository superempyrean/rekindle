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

/// The client's image file: what the anchors resolve against. A copy of the
/// client's library, placed in the config dir; the loader can't name the
/// library's own APK-relative path for `open`, and with
/// `extractNativeLibs=false` there is no extracted file to read. Named by
/// [`IMAGE_ENV`], or the default copy in rekindle's config dir.
#[cfg(target_os = "android")]
pub(crate) fn image_path() -> anyhow::Result<std::path::PathBuf> {
    if let Some(p) = std::env::var_os(IMAGE_ENV) {
        return Ok(p.into());
    }
    let p = crate::android::config_file(DEFAULT_IMAGE);
    if p.exists() {
        return Ok(p);
    }
    anyhow::bail!("{IMAGE_ENV} is not set and {} is missing", p.display())
}

/// The file name of the client's library: [`IMAGE_ENV`]'s, or the default
/// copy's. The loaded library is found by it, without reading the copy.
#[cfg(target_os = "android")]
fn image_name() -> std::ffi::OsString {
    std::env::var_os(IMAGE_ENV)
        .and_then(|p| std::path::Path::new(&p).file_name().map(ToOwned::to_owned))
        .unwrap_or_else(|| DEFAULT_IMAGE.into())
}

/// The client's library, as the app names it.
#[cfg(target_os = "android")]
const DEFAULT_IMAGE: &str = "liblibdiscord_wasm_host.so";

/// The client image's load bias: the address its `p_vaddr` 0 is mapped at,
/// which an anchor's virtual address is added to. See [`loaded_image`].
#[cfg(target_os = "android")]
pub(crate) fn image_base() -> Option<usize> {
    loaded_image().map(|l| l.bias)
}

/// The client's library as the dynamic linker loaded it.
#[cfg(target_os = "android")]
pub(crate) struct Loaded {
    /// The load bias (`dlpi_addr`).
    pub(crate) bias: usize,
    /// Its GNU build ID, read from its mapped `PT_NOTE`, if it has one.
    pub(crate) build_id: Option<Vec<u8>>,
}

/// The client's library, once the dynamic linker has loaded it.
///
/// A library the app maps straight out of the APK (`extractNativeLibs=false`)
/// has no mapping at file offset 0, so this reads what the dynamic linker
/// recorded (`dl_iterate_phdr`) rather than a `/proc/self/maps` line,
/// matching the loaded object by [`image_name`]. The linker holds its lock
/// from mapping a library through running its initializers, and
/// `dl_iterate_phdr` takes the same lock, so a library this returns is
/// relocated and its `PT_GNU_RELRO` already read-only.
#[cfg(target_os = "android")]
pub(crate) fn loaded_image() -> Option<Loaded> {
    use std::os::unix::ffi::OsStrExt;

    struct Search<'a> {
        name: &'a [u8],
        found: Option<Loaded>,
    }

    extern "C" fn visit(
        info: *mut libc::dl_phdr_info,
        _size: libc::size_t,
        data: *mut std::ffi::c_void,
    ) -> libc::c_int {
        // SAFETY: `dl_iterate_phdr` passes a live `dl_phdr_info` and the
        // `data` pointer this call handed it, a `&mut Search`.
        let (info, search) = unsafe { (&*info, &mut *data.cast::<Search>()) };
        if info.dlpi_name.is_null() {
            return 0;
        }
        // SAFETY: `dlpi_name` is a C string owned by the loader.
        let path = unsafe { std::ffi::CStr::from_ptr(info.dlpi_name) }.to_bytes();
        if !path.ends_with(search.name) {
            return 0;
        }
        let bias = info.dlpi_addr as usize;
        let mut build_id = None;
        for i in 0..usize::from(info.dlpi_phnum) {
            // SAFETY: `dlpi_phdr` points at `dlpi_phnum` program headers.
            let ph = unsafe { &*info.dlpi_phdr.add(i) };
            if ph.p_type != libc::PT_NOTE {
                continue;
            }
            // SAFETY: a `PT_NOTE` lies in a loaded, readable segment, at the
            // load bias plus its `p_vaddr`, for `p_memsz` bytes.
            let notes = unsafe {
                std::slice::from_raw_parts(
                    (bias + ph.p_vaddr as usize) as *const u8,
                    ph.p_memsz as usize,
                )
            };
            if let Some(id) = rekindle_hook::elf::gnu_build_id(notes) {
                build_id = Some(id.to_vec());
                break;
            }
        }
        search.found = Some(Loaded { bias, build_id });
        1
    }

    let name = image_name();
    let mut search = Search {
        name: name.as_bytes(),
        found: None,
    };
    // SAFETY: `visit` has the callback's signature and reads only through the
    // pointers the loader passes; `search` outlives the call.
    unsafe {
        libc::dl_iterate_phdr(Some(visit), (&raw mut search).cast());
    }
    search.found
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
