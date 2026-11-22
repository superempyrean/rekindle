//! Keeping the live client off the user's desktop, for the launcher's check
//! and the self-test.
//!
//! When a check runs the real client through this shim, the client would open
//! and activate a window. To keep to the project's rule of no synthetic input
//! and no window on the desktop, the client image's `user32` activation
//! imports point at stand-ins: no focus change, no OS input, and the window
//! placed off-screen. The host's drive mode relies on these patches. This
//! runs in the shim's initializer, before
//! `main`, so the first show is already off-screen. It only writes memory and
//! calls `VirtualProtect`, which is safe under the loader lock.

use std::ffi::c_void;

use crate::{iat, sys};

#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct WindowPlacement {
    length: u32,
    flags: u32,
    show_cmd: u32,
    min_position: [i32; 2],
    max_position: [i32; 2],
    normal_position: Rect,
}

const GWL_EXSTYLE: i32 = -20;
const WS_EX_TOOLWINDOW: isize = 0x80;
const WS_EX_APPWINDOW: isize = 0x40000;
const SWP_NOACTIVATE: u32 = 0x10;
const SWP_NOZORDER: u32 = 0x4;
const SWP_SHOWWINDOW: u32 = 0x40;
/// Far outside any monitor; `SetWindowPos` (unlike `SetWindowPlacement`)
/// doesn't pull a window back onto the desktop.
const OFFSCREEN: i32 = -30000;

extern "system" fn send_input(count: u32, _inputs: *const c_void, _size: i32) -> u32 {
    count
}
extern "system" fn set_foreground_window(_hwnd: *mut c_void) -> i32 {
    1
}
extern "system" fn set_active_window(_hwnd: *mut c_void) -> *mut c_void {
    std::ptr::null_mut()
}
extern "system" fn set_focus(_hwnd: *mut c_void) -> *mut c_void {
    std::ptr::null_mut()
}
extern "system" fn show_window_async(_hwnd: *mut c_void, _cmd: i32) -> i32 {
    1
}

/// `SetWindowPlacement`: the window shown off-screen at its own size, out of
/// the taskbar and not activated.
///
/// # Safety
///
/// `placement` points at a valid `WINDOWPLACEMENT`, as `SetWindowPlacement`
/// requires of its callers.
unsafe extern "system" fn set_window_placement(
    hwnd: *mut c_void,
    placement: *const WindowPlacement,
) -> i32 {
    // SAFETY: the caller's contract.
    let r = unsafe { &(*placement).normal_position };
    // SAFETY: the user32 calls take the client's own window handle and plain
    // values.
    unsafe {
        let ex = sys::GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        sys::SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            (ex | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW,
        );
        sys::SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            OFFSCREEN,
            OFFSCREEN,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW,
        )
    }
}

/// Point the client exe's user32 activation imports at the stand-ins, and
/// return how many slots now do. All or nothing: if any import is missing
/// this refuses, so a client that could still activate is never left
/// runnable.
pub(crate) fn install() -> Result<usize, &'static str> {
    let base = iat::exe_base().ok_or("no image base")?;
    let stand_ins: [(&str, usize); 6] = [
        ("SendInput", send_input as *const () as usize),
        (
            "SetForegroundWindow",
            set_foreground_window as *const () as usize,
        ),
        ("SetActiveWindow", set_active_window as *const () as usize),
        ("SetFocus", set_focus as *const () as usize),
        ("ShowWindowAsync", show_window_async as *const () as usize),
        (
            "SetWindowPlacement",
            set_window_placement as *const () as usize,
        ),
    ];
    let mut slots = Vec::new();
    for (name, f) in stand_ins {
        // SAFETY: `base` is the client exe's image, mapped by the loader.
        let found = unsafe { iat::slots(base, "user32.dll", name) };
        if found.is_empty() {
            return Err("client does not import a required user32 activation function");
        }
        slots.extend(found.into_iter().map(|s| (s, f)));
    }
    for &(slot, f) in &slots {
        // SAFETY: `slot` is one of the image's import slots.
        unsafe { iat::write(slot, f) }?;
    }
    Ok(slots.len())
}
