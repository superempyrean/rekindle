//! Startup validation: before rekindle shows anything in a client, check the
//! recovered layout against ground truth it doesn't come from.
//!
//! 1. **OS** (from the first frame, read-only, before any element is built): the
//!    window's viewport and scale factor, read at recovered offsets, must
//!    match what the OS says about the client window (on Windows its client
//!    rectangle and DPI, on Linux its X11 geometry).
//! 2. **Canary** (next frames): rekindle's layer is a single 1x1 element of
//!    a colour nothing else uses, at the window's top-left. It must appear in
//!    the finished scene with those bounds: the element path (construction,
//!    style writes, insertion into the tree) works in this build.
//!
//! Only when both pass does rekindle's real UI go in. A failure switches the
//! layer off with the reason (`status`), and the framework layer stops too
//! (`failed`), rather than writing anywhere in a build it doesn't understand.

use std::sync::Mutex;

use crate::client;

/// The canary's colour: near-invisible, and no client paints it.
const CANARY_RGBA: u32 = 0x0b_1d_2f_02;
/// Frames the canary may take to show up.
const CANARY_FRAMES: u32 = 30;
/// How long the OS check may keep failing before it counts: a window manager can map
/// and size the window after gpui's first frame.
const OS_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq)]
enum Check {
    Pending,
    Passed,
    Failed(String),
}

struct State {
    os: Check,
    canary: Check,
    canary_frames: u32,
    os_since: Option<std::time::Instant>,
}

static STATE: Mutex<State> = Mutex::new(State {
    os: Check::Pending,
    canary: Check::Pending,
    canary_frames: 0,
    os_since: None,
});

/// `true` once both checks passed (rekindle's UI may go in).
pub fn passed() -> bool {
    let s = STATE.lock().unwrap();
    s.os == Check::Passed && s.canary == Check::Passed
}

/// `true` if a check failed (rekindle stays out of this client).
pub fn failed() -> bool {
    let s = STATE.lock().unwrap();
    matches!(s.os, Check::Failed(_)) || matches!(s.canary, Check::Failed(_))
}

/// `os=..,canary=..` for status lines.
pub fn status() -> String {
    let s = STATE.lock().unwrap();
    let f = |c: &Check| match c {
        Check::Pending => "pending".to_string(),
        Check::Passed => "ok".to_string(),
        Check::Failed(why) => format!("FAILED({why})"),
    };
    format!("os={},canary={}", f(&s.os), f(&s.canary)).replace(' ', "_")
}

/// The OS check, from the first frame until it passes or [`OS_GRACE`] runs out.
///
/// # Safety
/// `window` must be the client's live `Window`, on its UI thread.
pub(crate) unsafe fn check_os(window: *const u8) {
    if STATE.lock().unwrap().os != Check::Pending {
        return;
    }
    // SAFETY: the caller's contract.
    let result = unsafe { os_ground_truth(window) };
    let mut s = STATE.lock().unwrap();
    let since = *s.os_since.get_or_insert_with(std::time::Instant::now);
    s.os = match result {
        Ok(()) => Check::Passed,
        Err(_) if since.elapsed() < OS_GRACE => Check::Pending,
        Err(why) => Check::Failed(why),
    };
}

/// Should this frame's layer be the canary (and only the canary)?
pub(crate) fn wants_canary() -> bool {
    let s = STATE.lock().unwrap();
    s.os == Check::Passed && s.canary == Check::Pending
}

/// After a frame with the canary in it: look for it in the scene.
///
/// # Safety
/// As [`check_os`], after the client's `draw_roots`.
pub unsafe fn after_frame(window: *const u8) {
    if !wants_canary() {
        return;
    }
    let want = crate::Rgba::hex(CANARY_RGBA).to_hsla();
    let close = |c: &[f32; 4]| c.iter().zip(want).all(|(a, b)| (a - b).abs() < 2e-3);
    // SAFETY: the caller's contract.
    let frame = unsafe { client::frame(window) };
    let found = frame.is_some_and(|f| {
        f.quads.iter().any(|(r, c)| {
            c.as_ref().is_some_and(close)
                && r.0.abs() < 0.5
                && r.1.abs() < 0.5
                && (r.2 - 1.0).abs() < 0.5
                && (r.3 - 1.0).abs() < 0.5
        })
    });
    let mut s = STATE.lock().unwrap();
    s.canary_frames += 1;
    if found {
        s.canary = Check::Passed;
    } else if s.canary_frames > CANARY_FRAMES {
        s.canary = Check::Failed(format!("not in the scene after {CANARY_FRAMES} frames"));
    }
}

/// The canary element.
pub(crate) fn canary() -> crate::Node {
    use crate::{Edges, Len, Node, Rgba, Style};
    Node::div(
        Style {
            absolute: true,
            inset: Edges {
                top: Some(Len::Px(0.0)),
                right: None,
                bottom: None,
                left: Some(Len::Px(0.0)),
            },
            width: Some(Len::Px(1.0)),
            height: Some(Len::Px(1.0)),
            background: Some(Rgba::hex(CANARY_RGBA)),
            ..Default::default()
        },
        vec![],
    )
}

/// The client's window as Windows sees it: its client area and DPI against
/// gpui's viewport and scale factor.
///
/// # Safety
/// As [`check_os`].
#[cfg(windows)]
unsafe fn os_ground_truth(window: *const u8) -> Result<(), String> {
    use std::ffi::c_void;
    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(
            f: unsafe extern "system" fn(*mut c_void, isize) -> i32,
            lparam: isize,
        ) -> i32;
        fn GetWindowThreadProcessId(hwnd: *mut c_void, pid: *mut u32) -> u32;
        fn IsWindowVisible(hwnd: *mut c_void) -> i32;
        fn GetClientRect(hwnd: *mut c_void, rect: *mut Rect) -> i32;
        fn GetDpiForWindow(hwnd: *mut c_void) -> u32;
    }
    /// The process's visible top-level window with the largest client area.
    struct Found {
        pid: u32,
        best: Option<(*mut c_void, Rect)>,
    }
    /// `EnumWindows`'s callback: `lparam` is the `Found` being filled.
    unsafe extern "system" fn each(hwnd: *mut c_void, lparam: isize) -> i32 {
        // SAFETY: `lparam` is the `&mut Found` passed to `EnumWindows`
        // below, which outlives the enumeration.
        let found = unsafe { &mut *(lparam as *mut Found) };
        let mut pid = 0u32;
        let mut r = Rect::default();
        // SAFETY: `hwnd` is a window `EnumWindows` handed over, and each
        // call writes only to the local passed to it.
        let ours = unsafe {
            GetWindowThreadProcessId(hwnd, &raw mut pid);
            pid == found.pid && IsWindowVisible(hwnd) != 0 && GetClientRect(hwnd, &raw mut r) != 0
        };
        let area = |r: &Rect| i64::from(r.right - r.left) * i64::from(r.bottom - r.top);
        if ours && found.best.as_ref().is_none_or(|(_, b)| area(&r) > area(b)) {
            found.best = Some((hwnd, r));
        }
        1
    }
    let mut found = Found {
        pid: std::process::id(),
        best: None,
    };
    // SAFETY: `each` matches `EnumWindows`'s callback, and `found` outlives
    // the call.
    unsafe { EnumWindows(each, (&raw mut found).addr() as isize) };
    let Some((hwnd, r)) = found.best else {
        return Err("no visible window".into());
    };
    // SAFETY: `hwnd` is one of this process's windows, found just above.
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        return Err("no DPI for the window".into());
    }
    let os_scale = dpi as f32 / 96.0;
    let (w, h) = (
        (r.right - r.left) as f32 / os_scale,
        (r.bottom - r.top) as f32 / os_scale,
    );
    // SAFETY: the caller's contract.
    let (scale, viewport) = unsafe { (client::scale_factor(window), client::viewport(window)) };
    let scale = scale.ok_or("not configured")?;
    let (vw, vh) = viewport.ok_or("not configured")?;
    if (scale - os_scale).abs() > 0.01 {
        return Err(format!("scale {scale} vs OS {os_scale}"));
    }
    if (vw - w).abs() > 1.5 || (vh - h).abs() > 1.5 {
        return Err(format!("viewport {vw}x{vh} vs OS {w}x{h}"));
    }
    Ok(())
}

/// The client's window as X11 sees it. X11 has no window scale (gpui picks
/// one from its env, `Xft.dpi` or `RandR`), so gpui's viewport times gpui's
/// scale must be the window's pixel size.
///
/// # Safety
/// As [`check_os`].
#[cfg(target_os = "linux")]
unsafe fn os_ground_truth(window: *const u8) -> Result<(), String> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, MapState};
    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("no X display: {e}"))?;
    let root = conn.setup().roots.get(screen).ok_or("no X screen")?.root;
    let pid_atom = conn
        .intern_atom(true, b"_NET_WM_PID")
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?
        .atom;
    let me = std::process::id();
    let mut best: Option<(u32, u32)> = None;
    let mut stack = vec![root];
    while let Some(w) = stack.pop() {
        if let Ok(tree) = conn
            .query_tree(w)
            .map_err(drop)
            .and_then(|c| c.reply().map_err(drop))
        {
            stack.extend(tree.children);
        }
        let pid = conn
            .get_property(false, w, pid_atom, AtomEnum::CARDINAL, 0, 1)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32()?.next());
        let viewable = conn
            .get_window_attributes(w)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|a| a.map_state == MapState::VIEWABLE);
        if pid != Some(me) || !viewable {
            continue;
        }
        let Some(g) = conn.get_geometry(w).ok().and_then(|c| c.reply().ok()) else {
            continue;
        };
        let (gw, gh) = (u32::from(g.width), u32::from(g.height));
        if best.is_none_or(|(bw, bh)| gw * gh > bw * bh) {
            best = Some((gw, gh));
        }
    }
    let (w, h) = best.ok_or("no visible window")?;
    // SAFETY: the caller's contract.
    let (scale, viewport) = unsafe { (client::scale_factor(window), client::viewport(window)) };
    let scale = scale.ok_or("not configured")?;
    let (vw, vh) = viewport.ok_or("not configured")?;
    if !(0.5..=4.0).contains(&scale) {
        return Err(format!("scale {scale}"));
    }
    if (vw * scale - w as f32).abs() > 1.5 || (vh * scale - h as f32).abs() > 1.5 {
        return Err(format!(
            "viewport {vw}x{vh} at scale {scale} vs OS {w}x{h} px"
        ));
    }
    Ok(())
}

#[cfg(not(any(windows, target_os = "linux")))]
unsafe fn os_ground_truth(_window: *const u8) -> Result<(), String> {
    Err("OS check is implemented for Windows and Linux".into())
}
