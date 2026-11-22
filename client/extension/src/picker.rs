//! The OS "open file" dialog, for rekindle's "Install a plugin from a
//! file…" button. It runs on its own thread (the dialog is modal to that
//! thread only), so the client keeps drawing; the choice comes back
//! through [`Picker::try_take`].

use std::path::PathBuf;
use std::sync::mpsc;

pub(crate) struct Picker {
    rx: mpsc::Receiver<Option<PathBuf>>,
}

/// How the dialog closed.
pub(crate) enum Picked {
    File(PathBuf),
    Cancelled,
}

impl Picker {
    /// Show the dialog. Fails if its thread can't start.
    pub(crate) fn open() -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("rekindle-file-picker".into())
            .spawn(move || {
                // The receiver is gone only when rekindle is, and then
                // nothing waits for the choice.
                let _ = tx.send(imp::pick());
            })?;
        Ok(Self { rx })
    }

    /// How the dialog closed, once it has.
    pub(crate) fn try_take(&self) -> Option<Picked> {
        match self.rx.try_recv() {
            Ok(Some(path)) => Some(Picked::File(path)),
            Ok(None) | Err(mpsc::TryRecvError::Disconnected) => Some(Picked::Cancelled),
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;

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

    const OFN_NOCHANGEDIR: u32 = 0x8;
    const OFN_PATHMUSTEXIST: u32 = 0x800;
    const OFN_FILEMUSTEXIST: u32 = 0x1000;
    const OFN_EXPLORER: u32 = 0x80000;
    const COINIT_APARTMENTTHREADED: u32 = 0x2;

    #[link(name = "comdlg32")]
    unsafe extern "system" {
        fn GetOpenFileNameW(ofn: *mut OpenFileNameW) -> i32;
    }

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    pub(super) fn pick() -> Option<PathBuf> {
        // "rekindle plugins and themes\0*.wasm;*.json\0\0"
        let mut filter = wide("rekindle plugins and themes (*.wasm, *.theme.json)");
        filter.push(0);
        filter.extend(wide("*.wasm;*.json"));
        filter.extend([0, 0]);
        let mut title = wide("Install a rekindle plugin or theme");
        title.push(0);
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
        // The dialog's shell views need COM on this thread. If it is already
        // initialized another way, the dialog still runs.
        // SAFETY: no reserved pointer; this thread is the picker's own.
        unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) };
        // SAFETY: every pointer in `ofn` points into a buffer that outlives
        // the call; the dialog writes at most `max_file` units into `file`.
        let picked = unsafe { GetOpenFileNameW(&raw mut ofn) };
        if picked == 0 {
            return None;
        }
        let len = file.iter().position(|&c| c == 0).unwrap_or(file.len());
        Some(PathBuf::from(std::ffi::OsString::from_wide(&file[..len])))
    }
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn pick() -> Option<std::path::PathBuf> {
        None
    }
}
