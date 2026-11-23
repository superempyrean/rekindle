//! The live client: a real launch, or the self-test showing or driving it.
//! rekindle stays attached and ticks the runtime until the client ends.

use std::time::{Duration, Instant};

use rekindle_extension::Driver;
use rekindle_install::env;

use crate::install::{installed_mode, store_installed, store_mode};
use crate::layout::load_palette;

/// Whether rekindle stays attached after the report: a real launch, or the
/// self-test keeping the client (`REKINDLE_HOST_KEEP`, for `--show` and
/// `--drive`).
pub(crate) fn stays() -> bool {
    installed_mode() || std::env::var_os("REKINDLE_HOST_KEEP").is_some()
}

/// Set up the live surfaces: theming by recolouring, and in drive mode the
/// drive script, with the client's window kept off the desktop. Returns
/// the mode for the report: `installed`, `drive` or `show`.
/// `client_surfaces` is false when rekindle runs the framework layer only.
#[cfg_attr(
    not(target_os = "linux"),
    expect(
        clippy::unnecessary_wraps,
        reason = "drive mode fails to start on Linux under Wayland"
    )
)]
pub(crate) fn start(driver: &mut Driver, client_surfaces: bool) -> anyhow::Result<&'static str> {
    use rekindle_ui_hook::{drive, inspector};
    driver.use_framework_theme(load_palette());
    // One value-matching route at a time: with the theme on the wire, the
    // scene isn't recoloured as well.
    if crate::wire::theme_on_wire() {
        driver.use_wire_theme();
    }
    let drives = std::env::var_os(env::DRIVE).is_some() && !(client_surfaces && installed_mode());
    // The drive checks the plugins' buttons where the client's registry or
    // the framework layer put them, so they move onto the wire after it.
    if !drives {
        crate::wire::start_ui(driver);
    }
    let mode = if drives {
        #[cfg(target_os = "linux")]
        crate::drive::install()?;
        // The self-test's queued copy is not for the user's clipboard.
        drop(rekindle_ui_hook::take_clipboard());
        if inspector::enabled() {
            drive::start_inspect();
        } else if !client_surfaces {
            drive::start_framework_only();
        } else if store_mode() {
            drive::start_store();
        } else {
            drive::start();
        }
        "drive"
    } else if installed_mode() {
        "installed"
    } else {
        "show"
    };
    driver.pump();
    Ok(mode)
}

/// rekindle's clock while the client runs: ~60 Hz ticks feed the cursor and
/// the plugin timers, and publish overlay sprites to the in-client layer.
/// Never returns; the process ends when the user closes the client.
pub(crate) fn keep_alive(mut driver: Driver) -> ! {
    let mut last = Instant::now();
    let mut drive_written = false;
    loop {
        driver.tick();
        crate::wire::follow(&driver);
        if !drive_written {
            drive_written = write_drive_result(&mut driver);
        }
        // The tick's sleep: answers a send at once if one arrives.
        driver.serve_compose(Duration::from_millis(16));
        if last.elapsed() >= Duration::from_secs(1) {
            last = Instant::now();
            write_status(&driver);
        }
    }
}

/// Drive mode: hand the injector the script's result once it's in
/// (`%TEMP%/rekindle-drive.txt`). True once written, or once there is
/// nothing more to write. The plugins' buttons go on the wire before the
/// file is written, since the self-test presses them once it is.
fn write_drive_result(driver: &mut Driver) -> bool {
    // A failed startup validation ends the script before it starts.
    let failed = rekindle_native::validate::failed().then(|| {
        format!(
            "validated=false validation={}",
            rekindle_native::validate::status()
        )
    });
    let Some(mut r) = failed.or_else(rekindle_ui_hook::drive::result) else {
        return false;
    };
    crate::wire::start_ui(driver);
    crate::wire::follow(driver);
    hold_toast();
    if let Some(counts) = crate::wire::counts() {
        r.push(' ');
        r.push_str(&counts);
    }
    if store_mode() {
        r.push_str(" store_installed=");
        r.push_str(&store_installed());
    }
    let dir = std::env::temp_dir();
    let tmp = dir.join("rekindle-drive.txt.tmp");
    // The injector waits for the file, and times out without it.
    if std::fs::write(&tmp, format!("{r}\n")).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join("rekindle-drive.txt"));
    }
    true
}

/// With `REKINDLE_HOST_TOAST` set (a test run, never an installed one),
/// show its text as a toast that stays a minute once the drive is done, so
/// the run can press what the toast covers.
fn hold_toast() {
    let Some(text) = std::env::var("REKINDLE_HOST_TOAST")
        .ok()
        .filter(|t| !t.is_empty() && !installed_mode())
    else {
        return;
    };
    rekindle_ui_hook::push_toast_for(
        &text,
        rekindle_ui_hook::ToastStyle::Info,
        Duration::from_secs(60),
    );
}

/// A status line, so the live run can be checked without touching the
/// window (`%TEMP%/rekindle-host-status.txt`), and the geometry the chat-bar
/// rule last saw (`%TEMP%/rekindle-frame.json`). Diagnostics only: a failed
/// write is ignored.
fn write_status(driver: &Driver) {
    let geom = rekindle_ui_hook::last_frame_geometry();
    let dump = match &geom {
        Some((h, f)) => serde_json::json!({ "read": true, "hitboxes": h, "filled": f }),
        None => serde_json::json!({ "read": false }),
    };
    let _ = std::fs::write(
        std::env::temp_dir().join("rekindle-frame.json"),
        dump.to_string(),
    );
    let view = rekindle_ui_hook::overlay_view();
    let (painted, bounds) = rekindle_ui_hook::native_painted();
    // The wire detours go on counting after the drive result is written:
    // the client's own input reaches the event detour later.
    let wire = crate::wire::counts()
        .map(|c| format!(" {c}"))
        .unwrap_or_default();
    #[cfg(target_os = "android")]
    let wire = format!("{wire} {}", crate::android::counts());
    let _ = std::fs::write(
        std::env::temp_dir().join("rekindle-host-status.txt"),
        format!(
            "validation={} drive={} framework_frames={} chat_bar_frames={} native_frames={} native_painted={painted} native_bounds={bounds:?} native_failed={:?} theme={} recolored={} scene_recolor={} framework_failed={} overlay_sprites={} toasts={} toast_cards={} modal={} pointer={:?} viewport={:?}{wire}\n",
            rekindle_native::validate::status(),
            rekindle_ui_hook::drive::status(),
            rekindle_ui_hook::framework_frames(),
            rekindle_ui_hook::chat_bar_frames(),
            rekindle_native::frames(),
            rekindle_native::failure(),
            rekindle_ui_hook::detected_theme().unwrap_or_else(|| "none".into()),
            rekindle_ui_hook::recolored_last_frame(),
            u8::from(driver.scene_recolor()),
            rekindle_ui_hook::framework_failed(),
            driver.overlay_sprites(),
            rekindle_ui_hook::toast_texts().len(),
            toast_cards(),
            rekindle_ui_hook::modal_plugin().unwrap_or_else(|| "none".into()),
            view.map(|v| v.pointer),
            view.map(|v| v.viewport),
        ),
    );
}

/// The toasts' cards as the status line lists them: `x,y,w,h` each,
/// `;`-separated, `none` when there are none.
fn toast_cards() -> String {
    let cards = rekindle_ui_hook::toast_cards();
    if cards.is_empty() {
        return "none".into();
    }
    cards
        .iter()
        .map(|(x, y, w, h)| format!("{x:.0},{y:.0},{w:.0},{h:.0}"))
        .collect::<Vec<_>>()
        .join(";")
}
