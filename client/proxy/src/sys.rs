//! The Win32 functions and constants the shim calls. No `windows` crate:
//! the shim stays a small library the client's loader maps.

use std::ffi::c_void;

#[link(name = "kernel32")]
unsafe extern "system" {
    pub(crate) fn LoadLibraryW(name: *const u16) -> *mut c_void;
    pub(crate) fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    pub(crate) fn GetSystemDirectoryW(buf: *mut u16, size: u32) -> u32;
    pub(crate) fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    pub(crate) fn GetModuleHandleExW(
        flags: u32,
        module_name: *const u16,
        module: *mut *mut c_void,
    ) -> i32;
    pub(crate) fn GetModuleFileNameW(module: *mut c_void, buf: *mut u16, size: u32) -> u32;
    pub(crate) fn GetFileAttributesW(name: *const u16) -> u32;
    pub(crate) fn VirtualProtect(addr: *mut c_void, size: usize, new: u32, old: *mut u32) -> i32;
    pub(crate) fn ExitProcess(code: u32) -> !;
}

#[link(name = "user32")]
unsafe extern "system" {
    pub(crate) fn SetWindowPos(
        hwnd: *mut c_void,
        after: *mut c_void,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        flags: u32,
    ) -> i32;
    pub(crate) fn GetWindowLongPtrW(hwnd: *mut c_void, index: i32) -> isize;
    pub(crate) fn SetWindowLongPtrW(hwnd: *mut c_void, index: i32, value: isize) -> isize;
}

pub(crate) const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
pub(crate) const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x4;
pub(crate) const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x2;
pub(crate) const PAGE_READWRITE: u32 = 0x04;
