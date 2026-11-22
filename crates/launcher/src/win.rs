//! The few Windows pieces an installer needs, called directly: message
//! boxes, the "open file" dialog, the per-user Add/Remove Programs entry,
//! and starting a program through Explorer.
//!
//! Nothing here starts anything automatically: no Run key, no scheduled
//! task, no service. The uninstall entry is how Windows lists the program
//! so the user can remove it.

use std::path::{Path, PathBuf};

use anyhow::Result;

/// The per-user uninstall entry's key, under `HKEY_CURRENT_USER`.
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\rekindle";

/// The `ShowWindow` command for a window opened the usual way.
pub const SW_SHOWNORMAL: i32 = 1;

/// The button a message box was closed with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    Cancel,
}

/// The icon of a message box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Info,
    Warning,
    Error,
}

/// What the uninstall entry says.
pub struct UninstallEntry<'a> {
    pub version: &'a str,
    pub root: &'a Path,
    pub exe: &'a Path,
    pub size_kb: u32,
}

#[cfg(windows)]
pub use imp::*;

#[cfg(windows)]
mod imp {
    use super::{Answer, Icon, Path, PathBuf, Result, UninstallEntry};
    use std::ffi::c_void;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    use anyhow::bail;

    type Hkey = *mut c_void;
    /// A COM interface pointer: a pointer to the object's vtable pointer.
    type Obj = *mut *const usize;

    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);

    const CLSID_SHELL_WINDOWS: Guid = Guid(
        0x9ba0_5972,
        0xf6a8,
        0x11cf,
        [0xa4, 0x42, 0x00, 0xa0, 0xc9, 0x0a, 0x8f, 0x39],
    );
    const IID_ISHELL_WINDOWS: Guid = Guid(
        0x85cb_6900,
        0x4d95,
        0x11cf,
        [0x96, 0x0c, 0x00, 0x80, 0xc7, 0xf4, 0xee, 0x85],
    );
    const IID_ISERVICE_PROVIDER: Guid = Guid(
        0x6d51_40c1,
        0x7436,
        0x11ce,
        [0x80, 0x34, 0x00, 0xaa, 0x00, 0x60, 0x09, 0xfa],
    );
    const SID_STOP_LEVEL_BROWSER: Guid = Guid(
        0x4c96_be40,
        0x915c,
        0x11cf,
        [0x99, 0xd3, 0x00, 0xaa, 0x00, 0x4a, 0xe8, 0x37],
    );
    const IID_ISHELL_BROWSER: Guid = Guid(0x0002_14e2, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
    const IID_IDISPATCH: Guid = Guid(0x0002_0400, 0, 0, [0xc0, 0, 0, 0, 0, 0, 0, 0x46]);
    const IID_ISHELL_FOLDER_VIEW_DUAL: Guid = Guid(
        0xe7a1_af80,
        0x4d96,
        0x11cf,
        [0x96, 0x0c, 0x00, 0x80, 0xc7, 0xf4, 0xee, 0x85],
    );
    const IID_ISHELL_DISPATCH2: Guid = Guid(
        0xa4c6_892c,
        0x3ba9,
        0x11d2,
        [0x9d, 0xea, 0x00, 0xc0, 0x4f, 0xb1, 0x61, 0x62],
    );

    const COINIT_APARTMENTTHREADED: u32 = 0x2;
    const CLSCTX_LOCAL_SERVER: u32 = 0x4;
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const MB_YESNO: u32 = 0x4;
    const MB_YESNOCANCEL: u32 = 0x3;
    const MB_ICONQUESTION: u32 = 0x20;
    const MB_SETFOREGROUND: u32 = 0x1_0000;
    const IDYES: i32 = 6;
    const IDNO: i32 = 7;
    const OFN_NOCHANGEDIR: u32 = 0x8;
    const OFN_PATHMUSTEXIST: u32 = 0x800;
    const OFN_FILEMUSTEXIST: u32 = 0x1000;
    const OFN_EXPLORER: u32 = 0x8_0000;
    const KEY_WRITE: u32 = 0x2_0006;
    const KEY_READ: u32 = 0x2_0019;
    const REG_SZ: u32 = 1;
    const REG_DWORD: u32 = 4;
    const PROCESS_IMAGE_LOAD_POLICY: i32 = 10;
    const PREFER_SYSTEM32_IMAGES: u32 = 0x4;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(hwnd: *mut c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(pid: u32) -> i32;
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn GetCurrentProcess() -> *mut c_void;
        fn GetProcessMitigationPolicy(
            process: *mut c_void,
            policy: i32,
            buffer: *mut u32,
            size: usize,
        ) -> i32;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> i32;
        fn CoCreateInstance(
            clsid: *const Guid,
            outer: *mut c_void,
            ctx: u32,
            iid: *const Guid,
            out: *mut Obj,
        ) -> i32;
    }
    #[link(name = "oleaut32")]
    unsafe extern "system" {
        fn SysAllocString(s: *const u16) -> *mut u16;
        fn SysFreeString(s: *mut u16);
    }
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegCreateKeyExW(
            key: Hkey,
            sub: *const u16,
            r: u32,
            class: *const u16,
            opts: u32,
            sam: u32,
            sec: *mut c_void,
            out: *mut Hkey,
            disp: *mut u32,
        ) -> i32;
        fn RegSetValueExW(
            key: Hkey,
            name: *const u16,
            r: u32,
            kind: u32,
            data: *const u8,
            len: u32,
        ) -> i32;
        fn RegCloseKey(key: Hkey) -> i32;
        fn RegDeleteTreeW(key: Hkey, sub: *const u16) -> i32;
        fn RegOpenKeyExW(key: Hkey, sub: *const u16, opts: u32, sam: u32, out: *mut Hkey) -> i32;
    }
    #[link(name = "comdlg32")]
    unsafe extern "system" {
        fn GetOpenFileNameW(ofn: *mut OpenFileNameW) -> i32;
    }

    /// `s` as a NUL-terminated UTF-16 string.
    fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
        s.as_ref().encode_wide().chain([0]).collect()
    }

    fn hkcu() -> Hkey {
        // HKEY_CURRENT_USER is ((HKEY)(LONG)0x80000001): sign-extended.
        0x8000_0001u32 as i32 as isize as Hkey
    }

    /// Join COM on this thread as a single-threaded apartment.
    fn co_initialize() {
        // SAFETY: the reserved argument must be null, and is. The result is
        // ignored: COM is either initialized here now or was already.
        unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) };
    }

    /// Write to the terminal that started this windowed program, unless its
    /// output already goes somewhere (a pipe, a file). `--check` and the
    /// other command-line uses print to it.
    pub fn attach_console() {
        // SAFETY: both calls take only a constant and touch no memory of ours.
        unsafe {
            if GetStdHandle(STD_OUTPUT_HANDLE).is_null() {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }
        }
    }

    fn message(title: &str, text: &str, kind: u32) -> i32 {
        let (t, c) = (wide(text), wide(title));
        // SAFETY: `t` and `c` are NUL-terminated and outlive the call; a null
        // owner window is allowed.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                t.as_ptr(),
                c.as_ptr(),
                kind | MB_SETFOREGROUND,
            )
        }
    }

    /// A message box with an OK button.
    pub fn tell(title: &str, text: &str, icon: Icon) {
        let icon = match icon {
            Icon::Info => 0x40,
            Icon::Warning => 0x30,
            Icon::Error => 0x10,
        };
        message(title, text, icon);
    }

    /// A question with Yes and No buttons, and Cancel if `cancel`.
    pub fn ask(title: &str, text: &str, cancel: bool) -> Answer {
        let buttons = if cancel { MB_YESNOCANCEL } else { MB_YESNO };
        match message(title, text, MB_ICONQUESTION | buttons) {
            IDYES => Answer::Yes,
            IDNO => Answer::No,
            _ => Answer::Cancel,
        }
    }

    #[repr(C)]
    struct OpenFileNameW {
        struct_size: u32,
        owner: *mut c_void,
        instance: *mut c_void,
        filter: *const u16,
        custom_filter: *mut u16,
        max_cust_filter: u32,
        filter_index: u32,
        file: *mut u16,
        max_file: u32,
        file_title: *mut u16,
        max_file_title: u32,
        initial_dir: *const u16,
        title: *const u16,
        flags: u32,
        file_offset: u16,
        file_extension: u16,
        def_ext: *const u16,
        cust_data: isize,
        hook: *mut c_void,
        template_name: *const u16,
        reserved_ptr: *mut c_void,
        reserved: u32,
        flags_ex: u32,
    }

    /// The "open file" dialog for a program (`*.exe`). `None` if the user
    /// cancelled.
    pub fn pick_program(title: &str) -> Option<PathBuf> {
        let filter: Vec<u16> = "Programs (*.exe)"
            .encode_utf16()
            .chain([0])
            .chain("*.exe".encode_utf16())
            .chain([0, 0])
            .collect();
        let title = wide(title);
        let mut file = vec![0u16; 4096];
        let mut ofn = OpenFileNameW {
            struct_size: std::mem::size_of::<OpenFileNameW>() as u32,
            owner: std::ptr::null_mut(),
            instance: std::ptr::null_mut(),
            filter: filter.as_ptr(),
            custom_filter: std::ptr::null_mut(),
            max_cust_filter: 0,
            filter_index: 1,
            file: file.as_mut_ptr(),
            max_file: file.len() as u32,
            file_title: std::ptr::null_mut(),
            max_file_title: 0,
            initial_dir: std::ptr::null(),
            title: title.as_ptr(),
            flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
            file_offset: 0,
            file_extension: 0,
            def_ext: std::ptr::null(),
            cust_data: 0,
            hook: std::ptr::null_mut(),
            template_name: std::ptr::null(),
            reserved_ptr: std::ptr::null_mut(),
            reserved: 0,
            flags_ex: 0,
        };
        co_initialize();
        // SAFETY: every pointer in `ofn` points into a buffer that outlives
        // the call; the dialog writes at most `max_file` units into `file`.
        if unsafe { GetOpenFileNameW(&raw mut ofn) } == 0 {
            return None;
        }
        let len = file.iter().position(|&c| c == 0).unwrap_or(file.len());
        Some(PathBuf::from(std::ffi::OsString::from_wide(&file[..len])))
    }

    fn check(hr: i32, what: &str) -> Result<()> {
        if hr < 0 {
            bail!("{what} failed ({:#x})", hr as u32);
        }
        Ok(())
    }

    fn set_value(key: Hkey, name: &str, kind: u32, data: &[u8]) -> Result<()> {
        let n = wide(name);
        // SAFETY: `key` is open for writing, `n` is NUL-terminated, and
        // `data` holds `data.len()` bytes; all outlive the call.
        let rc =
            unsafe { RegSetValueExW(key, n.as_ptr(), 0, kind, data.as_ptr(), data.len() as u32) };
        if rc != 0 {
            bail!("writing registry value {name} failed ({rc})");
        }
        Ok(())
    }

    fn set_str(key: Hkey, name: &str, value: &str) -> Result<()> {
        let w: Vec<u8> = wide(value).iter().flat_map(|c| c.to_le_bytes()).collect();
        set_value(key, name, REG_SZ, &w)
    }

    fn set_dword(key: Hkey, name: &str, value: u32) -> Result<()> {
        set_value(key, name, REG_DWORD, &value.to_le_bytes())
    }

    /// Write the Add/Remove Programs entry at `key` (normally [`UNINSTALL_KEY`]).
    ///
    /// [`UNINSTALL_KEY`]: super::UNINSTALL_KEY
    pub fn write_uninstall_entry(key: &str, e: &UninstallEntry) -> Result<()> {
        let sub = wide(key);
        let mut h: Hkey = std::ptr::null_mut();
        // SAFETY: `sub` is NUL-terminated and outlives the call, `h` is an
        // out-pointer, and the optional arguments are null.
        let rc = unsafe {
            RegCreateKeyExW(
                hkcu(),
                sub.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_WRITE,
                std::ptr::null_mut(),
                &raw mut h,
                std::ptr::null_mut(),
            )
        };
        if rc != 0 {
            bail!("creating the uninstall entry failed ({rc})");
        }
        let exe = e.exe.display().to_string();
        let result = (|| {
            set_str(h, "DisplayName", "rekindle")?;
            set_str(h, "DisplayVersion", e.version)?;
            set_str(h, "Publisher", "rekindle")?;
            set_str(h, "InstallLocation", &e.root.display().to_string())?;
            set_str(h, "DisplayIcon", &exe)?;
            set_str(h, "UninstallString", &format!("\"{exe}\" --uninstall"))?;
            set_dword(h, "NoModify", 1)?;
            set_dword(h, "NoRepair", 1)?;
            set_dword(h, "EstimatedSize", e.size_kb)
        })();
        // SAFETY: `h` was opened above and is closed once.
        unsafe { RegCloseKey(h) };
        result
    }

    /// Whether the entry at `key` exists.
    pub fn uninstall_entry_exists(key: &str) -> bool {
        let sub = wide(key);
        let mut h: Hkey = std::ptr::null_mut();
        // SAFETY: `sub` is NUL-terminated and outlives the call; `h` is an
        // out-pointer, closed below if the key opened.
        unsafe {
            let rc = RegOpenKeyExW(hkcu(), sub.as_ptr(), 0, KEY_READ, &raw mut h);
            if rc == 0 {
                RegCloseKey(h);
            }
            rc == 0
        }
    }

    /// Remove the entry at `key`, if it exists.
    pub fn remove_uninstall_entry(key: &str) {
        let sub = wide(key);
        // SAFETY: `sub` is NUL-terminated and outlives the call. A missing key
        // is the result wanted, so the status is ignored.
        unsafe { RegDeleteTreeW(hkcu(), sub.as_ptr()) };
    }

    /// Whether this process loads DLLs from System32 before a program's own
    /// directory (`PreferSystem32Images`). It is inherited from the process
    /// that started this one: browsers run with it, so a setup opened from
    /// the browser's downloads has it, and so does every client it starts.
    pub fn prefers_system32() -> bool {
        let mut policy = 0u32;
        // SAFETY: the image load policy is one `u32`, and `policy` is one.
        let ok = unsafe {
            GetProcessMitigationPolicy(
                GetCurrentProcess(),
                PROCESS_IMAGE_LOAD_POLICY,
                &raw mut policy,
                std::mem::size_of::<u32>(),
            )
        };
        ok != 0 && policy & PREFER_SYSTEM32_IMAGES != 0
    }

    /// A COM interface this module holds one reference to, released on drop.
    struct Com(Obj);

    impl Com {
        /// Call `get` with an out-pointer for an interface, and take the
        /// reference it returns. `what` names the call in an error.
        fn take(what: &str, get: impl FnOnce(*mut Obj) -> i32) -> Result<Com> {
            let mut obj: Obj = std::ptr::null_mut();
            check(get(&raw mut obj), what)?;
            if obj.is_null() {
                bail!("{what} returned nothing");
            }
            Ok(Com(obj))
        }

        /// The method at `index` of the interface's vtable.
        ///
        /// # Safety
        ///
        /// The interface's vtable has a method of type `T` at `index`.
        unsafe fn method<T: Copy>(&self, index: usize) -> T {
            // SAFETY: `self.0` is a live interface pointer, so it points at
            // the object's vtable pointer, and the caller vouches for the
            // slot's type.
            unsafe { std::mem::transmute_copy::<usize, T>(&*(*self.0).add(index)) }
        }
    }

    impl Drop for Com {
        fn drop(&mut self) {
            type Release = unsafe extern "system" fn(Obj) -> u32;
            // SAFETY: `IUnknown::Release` is slot 2 of every COM vtable, and
            // `Com` owns the one reference it releases.
            unsafe { self.method::<Release>(2)(self.0) };
        }
    }

    /// A `BSTR`, freed on drop.
    struct Bstr(*mut u16);

    impl Bstr {
        fn new(s: &[u16]) -> Bstr {
            // SAFETY: `s` is NUL-terminated (from `wide`).
            Bstr(unsafe { SysAllocString(s.as_ptr()) })
        }
    }

    impl Drop for Bstr {
        fn drop(&mut self) {
            // SAFETY: `self.0` came from `SysAllocString` and is freed once.
            unsafe { SysFreeString(self.0) };
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Variant {
        vt: u16,
        reserved: [u16; 3],
        value: [usize; 2],
    }

    impl Variant {
        const EMPTY: Variant = Variant {
            vt: 0,
            reserved: [0; 3],
            value: [0; 2],
        };
        const VT_I4: u16 = 3;
        const VT_BSTR: u16 = 8;

        fn bstr(s: &Bstr) -> Variant {
            Variant {
                vt: Self::VT_BSTR,
                reserved: [0; 3],
                value: [s.0 as usize, 0],
            }
        }

        fn int(i: i32) -> Variant {
            Variant {
                vt: Self::VT_I4,
                reserved: [0; 3],
                value: [i as u32 as usize, 0],
            }
        }
    }

    /// Start `program` with `args` through the desktop's Explorer, as if the
    /// user had opened it there: it doesn't inherit this process's
    /// mitigations (see [`prefers_system32`]). `show` is a `ShowWindow`
    /// command. Returns once Explorer has been asked; there is no handle to
    /// wait on.
    pub fn start_through_explorer(program: &Path, args: &str, show: i32) -> Result<()> {
        type Qi = unsafe extern "system" fn(Obj, *const Guid, *mut Obj) -> i32;
        type FindWindowSw = unsafe extern "system" fn(
            Obj,
            *const Variant,
            *const Variant,
            i32,
            *mut i32,
            i32,
            *mut Obj,
        ) -> i32;
        type QueryService =
            unsafe extern "system" fn(Obj, *const Guid, *const Guid, *mut Obj) -> i32;
        type QueryActiveShellView = unsafe extern "system" fn(Obj, *mut Obj) -> i32;
        type GetItemObject = unsafe extern "system" fn(Obj, u32, *const Guid, *mut Obj) -> i32;
        type GetApplication = unsafe extern "system" fn(Obj, *mut Obj) -> i32;
        type ShellExecute =
            unsafe extern "system" fn(Obj, *mut u16, Variant, Variant, Variant, Variant) -> i32;
        const SWC_DESKTOP: i32 = 8;
        const SWFO_NEEDDISPATCH: i32 = 1;
        const SVGIO_BACKGROUND: u32 = 0;

        let file = Bstr::new(&wide(program));
        let args = Bstr::new(&wide(args));
        let dir = Bstr::new(&wide(program.parent().unwrap_or(Path::new("."))));
        co_initialize();
        // The vtable slots below are the documented ones of each interface:
        // IShellWindows, IServiceProvider, IShellBrowser, IShellView,
        // IShellFolderViewDual and IShellDispatch2. Every out-pointer is a
        // local the call fills.
        let windows = Com::take("finding Explorer", |out| {
            // SAFETY: the CLSID and IID are ShellWindows' and IShellWindows'.
            unsafe {
                CoCreateInstance(
                    &CLSID_SHELL_WINDOWS,
                    std::ptr::null_mut(),
                    CLSCTX_LOCAL_SERVER,
                    &IID_ISHELL_WINDOWS,
                    out,
                )
            }
        })?;
        let mut hwnd = 0i32;
        let mut desktop: Obj = std::ptr::null_mut();
        // SAFETY: `FindWindowSW` is slot 15 of IShellWindows.
        let hr = unsafe {
            windows.method::<FindWindowSw>(15)(
                windows.0,
                &Variant::EMPTY,
                &Variant::EMPTY,
                SWC_DESKTOP,
                &raw mut hwnd,
                SWFO_NEEDDISPATCH,
                &raw mut desktop,
            )
        };
        check(hr, "finding the desktop")?;
        if desktop.is_null() {
            bail!("Explorer isn't running");
        }
        let desktop = Com(desktop);
        let services = Com::take("IServiceProvider", |out| {
            // SAFETY: `QueryInterface` is slot 0 of every interface.
            unsafe { desktop.method::<Qi>(0)(desktop.0, &IID_ISERVICE_PROVIDER, out) }
        })?;
        let browser = Com::take("IShellBrowser", |out| {
            // SAFETY: `QueryService` is slot 3 of IServiceProvider.
            unsafe {
                services.method::<QueryService>(3)(
                    services.0,
                    &SID_STOP_LEVEL_BROWSER,
                    &IID_ISHELL_BROWSER,
                    out,
                )
            }
        })?;
        let view = Com::take("IShellBrowser::QueryActiveShellView", |out| {
            // SAFETY: `QueryActiveShellView` is slot 15 of IShellBrowser.
            unsafe { browser.method::<QueryActiveShellView>(15)(browser.0, out) }
        })?;
        let background = Com::take("IShellView::GetItemObject", |out| {
            // SAFETY: `GetItemObject` is slot 15 of IShellView.
            unsafe {
                view.method::<GetItemObject>(15)(view.0, SVGIO_BACKGROUND, &IID_IDISPATCH, out)
            }
        })?;
        let folder = Com::take("IShellFolderViewDual", |out| {
            // SAFETY: `QueryInterface` is slot 0 of every interface.
            unsafe { background.method::<Qi>(0)(background.0, &IID_ISHELL_FOLDER_VIEW_DUAL, out) }
        })?;
        let app = Com::take("IShellFolderViewDual::get_Application", |out| {
            // SAFETY: `get_Application` is slot 7 of IShellFolderViewDual.
            unsafe { folder.method::<GetApplication>(7)(folder.0, out) }
        })?;
        let shell = Com::take("IShellDispatch2", |out| {
            // SAFETY: `QueryInterface` is slot 0 of every interface.
            unsafe { app.method::<Qi>(0)(app.0, &IID_ISHELL_DISPATCH2, out) }
        })?;
        // SAFETY: `ShellExecute` is slot 31 of IShellDispatch2. The strings
        // are BSTRs that outlive the call.
        let hr = unsafe {
            shell.method::<ShellExecute>(31)(
                shell.0,
                file.0,
                Variant::bstr(&args),
                Variant::bstr(&dir),
                Variant::EMPTY,
                Variant::int(show),
            )
        };
        check(hr, "IShellDispatch2::ShellExecute")
    }
}

#[cfg(not(windows))]
pub use other::*;

#[cfg(not(windows))]
mod other {
    use super::*;
    use anyhow::bail;

    pub fn attach_console() {}

    pub fn tell(title: &str, text: &str, _icon: Icon) {
        eprintln!("{title}: {text}");
    }

    pub fn ask(title: &str, text: &str, _cancel: bool) -> Answer {
        eprintln!("{title}: {text} (no dialogs on this platform: answering no)");
        Answer::No
    }

    pub fn pick_program(_title: &str) -> Option<PathBuf> {
        None
    }

    pub fn write_uninstall_entry(_key: &str, _e: &UninstallEntry) -> Result<()> {
        bail!("the uninstall entry is Windows-only")
    }

    pub fn uninstall_entry_exists(_key: &str) -> bool {
        false
    }

    pub fn remove_uninstall_entry(_key: &str) {}

    pub fn prefers_system32() -> bool {
        false
    }

    pub fn start_through_explorer(_program: &Path, _args: &str, _show: i32) -> Result<()> {
        bail!("Explorer is Windows-only")
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Run under a parent with `PreferSystem32Images` (a browser): the
    /// program Explorer starts doesn't have it. Ignored by default, since it
    /// needs that parent; the program it starts is hidden.
    #[test]
    #[ignore = "needs a parent that prefers System32 images"]
    fn a_program_started_through_explorer_drops_the_system32_preference() {
        const SW_HIDE: i32 = 0;
        assert!(
            prefers_system32(),
            "run this under a parent that prefers System32 images"
        );
        let out =
            std::env::temp_dir().join(format!("rekindle-explorer-test-{}", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let script = format!(
            "-NoProfile -Command \"(Get-ProcessMitigation -Id $PID).ImageLoad.PreferSystem32 | Out-File -Encoding ascii '{}'\"",
            out.display()
        );
        let ps = Path::new(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe");
        start_through_explorer(ps, &script, SW_HIDE).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let got = loop {
            if let Ok(s) = std::fs::read_to_string(&out)
                && !s.trim().is_empty()
            {
                break s;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the program Explorer started didn't write {}",
                out.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        let _ = std::fs::remove_file(&out);
        assert_eq!(got.trim(), "OFF");
    }
}
