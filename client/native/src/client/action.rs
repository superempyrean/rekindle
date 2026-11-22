//! The client's actions, found and dispatched by name: what its keymap and
//! menus do.

use anyhow::{Context, Result, bail};

use super::{check, layout, layout_or_err, read_checked};
use crate::bytes::Bytes;
use crate::mem;

/// The build function registered for action `name` in the client's own
/// action registry (`App.actions.by_name`), found by its *name* (what gpui's
/// keymap and menus use), so it survives any code change that keeps it.
///
/// # Safety
/// `cx` must be the client's live `App`, on its UI thread.
unsafe fn find_action(cx: *const u8, name: &str) -> Option<usize> {
    let l = layout()?;
    let a = l.c.actions.as_ref()?;
    // SAFETY: the caller's contract; `app_actions` is the app's
    // `Rc<ActionRegistry>`.
    let rc = unsafe { read_checked::<usize>(cx.addr() + a.app_actions, "the app's actions")? };
    let map = rc + a.rc_value + a.registry_by_name;
    // SAFETY: the registry's `by_name` map, a hashbrown table: its control
    // bytes and bucket mask.
    let (ctrl, mask) = unsafe {
        (
            read_checked::<usize>(map + a.map_ctrl, "the action map")?,
            read_checked::<usize>(map + a.map_bucket_mask, "the action map")?,
        )
    };
    if ctrl == 0 || mask > 1 << 20 {
        return None;
    }
    // hashbrown: slot i's bucket sits just below the control bytes, the i-th
    // from the end, and a full slot's control byte has its top bit clear.
    let buckets = (mask + 1) * a.bucket_size;
    check(
        ctrl.checked_sub(buckets)?,
        buckets + mask + 1,
        false,
        "the action map's buckets",
    )?;
    (0..=mask).find_map(|i| {
        // SAFETY: inside the buckets and control bytes checked readable
        // above; each bucket is a `(&str, ActionData)`, whose fields lie
        // inside it (checked by `resolve`).
        unsafe {
            if mem::read::<u8>(ctrl + i) & 0x80 != 0 {
                return None;
            }
            let bucket = ctrl - (i + 1) * a.bucket_size;
            let [key, len] = mem::read::<[usize; 2]>(bucket + a.bucket_key);
            if len != name.len() || key == 0 || check(key, len, false, "an action's name").is_none()
            {
                return None;
            }
            // Checked readable just above.
            (std::slice::from_raw_parts(key as *const u8, len) == name.as_bytes())
                .then(|| mem::read::<usize>(bucket + a.bucket_value + a.build))
        }
    })
}

/// Dispatch the client's action `name` in `window`, as its keymap would:
/// build it with the registered builder (JSON `null`, for a parameterless
/// action), then the client's own `Window::dispatch_action` (which defers it
/// to the app's effect queue and routes it from the focused element).
///
/// # Safety
/// `window` and `cx` must be the client's live `Window` and `App`, on its
/// UI thread, outside any draw (e.g. from `on_next_frame`).
pub unsafe fn dispatch_action(window: *mut u8, cx: *mut u8, name: &str) -> Result<()> {
    let l = layout_or_err()?;
    let a =
        l.c.actions
            .as_ref()
            .context("the layout file has no actions by name")?;
    if l.fns.window_dispatch_action == 0 {
        bail!("this client has no Window::dispatch_action");
    }
    // SAFETY: the caller's contract.
    let build = unsafe { find_action(cx, name) }
        .with_context(|| format!("no action {name:?} registered"))?;
    // A `serde_json::Value::Null`, which `build` takes: `null` holds no
    // heap, so the buffer is just storage once it's moved out.
    let mut value = Bytes::copied(&a.json_null, a.json_null.len().max(32));
    let mut out = [0usize; 2];
    // SAFETY: `build(ret: *mut Result<Box<dyn Action>>, value: *mut Value)`
    // is the registered builder, found above, with a whole `Value`.
    unsafe {
        let f: unsafe fn(*mut [usize; 2], *mut u8) -> *mut [usize; 2] = std::mem::transmute(build);
        f(&raw mut out, value.as_mut_ptr());
    }
    // `Ok(Box<dyn Action>)` is (data, vtable); `Err` has a null first word.
    if out[0] == 0 {
        bail!("building {name:?} failed");
    }
    // SAFETY: the client's `Window::dispatch_action(&mut self, Box<dyn
    // Action>, &mut App)`, which takes the box just built, with the caller's
    // live window and app.
    unsafe {
        let d: unsafe fn(*mut u8, usize, usize, *mut u8) =
            std::mem::transmute(l.fns.window_dispatch_action);
        d(window, out[0], out[1], cx);
    }
    Ok(())
}
