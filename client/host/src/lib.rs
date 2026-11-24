//! rekindle as a host DLL inside the client.
//!
//! The side-load shim (`client/proxy`) loads this library into the client:
//! the client's own loader maps the shim, which loads the host, so the
//! client runs normally (no suspension, no cross-process writes). From a
//! worker thread started on load, rekindle resolves the client's functions,
//! starts the plugin runtime inside it, checks what it can through the
//! client's own functions, and writes a one-line report to the handshake
//! file.
//!
//! It depends on neither the client nor its types. Every hook point is an
//! anchor (`rekindle-anchor`: redundant locators, such as a short unique
//! signature, the whole-function pattern and string references), resolved
//! against the client's own image file from `REKINDLE_HOST_ANCHORS`; every
//! offset is an entry of the layout file in `REKINDLE_HOST_LAYOUT`.

/// Drive mode on Linux (test harness). On Windows the shim keeps the
/// client's window off the desktop before `main`.
#[cfg(target_os = "linux")]
mod drive {
    /// The test harness runs the client on a headless X server; gpui picks
    /// Wayland over X11 when it can, which would be the user's desktop.
    pub(crate) fn install() -> anyhow::Result<usize> {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            anyhow::bail!("WAYLAND_DISPLAY is set; drive mode runs only on a headless X server");
        }
        Ok(0)
    }
}
#[cfg(target_os = "android")]
mod android;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub mod app;
mod debug;
#[cfg(any(target_os = "android", target_os = "ios", test))]
mod gateway;
mod hooks;
mod image;
mod install;
mod layout;
mod live;
#[cfg(any(target_os = "android", target_os = "ios", test))]
mod messages;
mod profile;
#[cfg(any(target_os = "android", target_os = "ios", test))]
mod rows;
mod verify;
mod wire;

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU32, Ordering};

use rekindle_extension::Driver;
use rekindle_install::env;

use hooks::{Hooks, frame_hook};
use install::installed_mode;
use layout::Layout;

/// Set once `run` has finished, whether rekindle came up or gave up.
static READY: AtomicU32 = AtomicU32::new(0);

/// 1 once `run` has finished (rekindle's hooks are in, or it gave up), else 0.
/// The shim holds the client's startup on it, so the client builds its
/// registries and opens its window after rekindle's detours exist.
#[unsafe(no_mangle)]
pub extern "system" fn rekindle_host_ready() -> u32 {
    READY.load(Ordering::Acquire)
}

#[cfg(windows)]
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static HOST_CTOR: extern "C" fn() = host_ctor;

#[cfg(any(target_os = "linux", target_os = "android"))]
#[used]
#[unsafe(link_section = ".init_array")]
static HOST_CTOR: extern "C" fn() = host_ctor;

/// The library's initializer: start the worker. Starting a thread from it
/// is fine; the thread must not touch the loader until the loader lock is
/// released, which the sleep in [`worker`] ensures.
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
extern "C" fn host_ctor() {
    // The initializer runs when the app maps this library. The first log
    // line proves the loader reached rekindle, before any other work.
    #[cfg(target_os = "android")]
    android::log(&format!("host .init_array: pid={}", std::process::id()));
    // SAFETY: the initializer runs before the client's `main`, on its only
    // thread.
    #[cfg(target_os = "linux")]
    unsafe {
        image::leave_preload();
    }
    let started = std::thread::Builder::new()
        .name("rekindle-host".into())
        .spawn(worker);
    // Preloaded on Linux, the initializer runs before the client's `main`,
    // and holds it there until rekindle is ready (or 20 s pass).
    #[cfg(target_os = "linux")]
    if started.is_ok() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while rekindle_host_ready() == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    // Without the worker, rekindle stays out and the client runs as it is.
    #[cfg(not(target_os = "linux"))]
    drop(started);
}

/// Worker started on load: it waits for the loader lock to release, does the
/// work, and writes the report. Keeping the heavy work off the static
/// initializer (which runs under the Windows loader lock) is what makes it safe
/// to scan, hook and call into the client here.
#[cfg(not(target_os = "ios"))]
fn worker() {
    // Android: the host is loaded into the app, resolves its anchors against
    // the client's library once that library is mapped, swaps the emit slot
    // and runs the engine on the turns. The desktop `run` path (gpui frame
    // hook, native theming, the client's own registries) doesn't apply.
    #[cfg(target_os = "android")]
    {
        android_worker();
        return;
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    desktop_worker();
}

/// The desktop worker: the shared [`run`], then the report to the install and
/// the handshake, and the client kept alive if a driver came back.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn desktop_worker() {
    // Let `DllMain`/the loader finish before doing anything non-trivial.
    #[cfg(windows)]
    std::thread::sleep(std::time::Duration::from_millis(150));
    let (report, kept) = std::panic::catch_unwind(run).unwrap_or_else(|_| {
        let head = report_head();
        (format!("{head} error=panic"), None)
    });
    // The shim loaded this version from the install: say whether it came up,
    // before the report, which ends a check.
    let version = std::env::var(env::HOST_VERSION)
        .ok()
        .filter(|_| installed_mode());
    if let Some(v) = &version {
        install::report_to_install(v, &report);
    }
    if let Ok(path) = std::env::var(env::HANDSHAKE) {
        // Only the shim reads it, and a shim that never gets it reports
        // that itself.
        let _ = std::fs::write(path, format!("{report}\n"));
    }
    READY.store(1, Ordering::Release);
    if version.is_some() {
        install::start_updates();
    }
    if let Some(driver) = kept {
        live::keep_alive(driver);
    }
}

/// The Android worker: resolve the anchors and install the emit hook
/// ([`android::run_worker`]), write the report to logcat and the handshake
/// file, then log the running counts while the app runs.
#[cfg(target_os = "android")]
fn android_worker() {
    let report = std::panic::catch_unwind(android::run_worker)
        .unwrap_or_else(|_| format!("{} error=panic", report_head()));
    android::log(&report);
    // logcat is the primary channel; a copy goes to the config dir (or the
    // handshake path, if a test run set one) to be pulled off the device.
    let handshake = std::env::var_os(env::HANDSHAKE)
        .map_or_else(|| android::config_file("handshake.txt"), Into::into);
    let _ = std::fs::write(handshake, format!("{report}\n"));
    READY.store(1, Ordering::Release);
    let mut tick = 0u64;
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        app::poll();
        tick += 1;
        if tick.is_multiple_of(5) {
            android::log(&app::counts());
        }
    }
}

fn report_head() -> String {
    format!(
        "rekindle-host pid={} base={:#x}",
        std::process::id(),
        image::module_base()
    )
}

/// The report the loader reads back, and the driver to keep if the client
/// stays live. One line of `key=value` fields, so the loader's `base=0x..`
/// parser still works and the injector can assert on it.
fn run() -> (String, Option<Driver>) {
    let mut out = report_head();
    let mut layout = match std::env::var(env::LAYOUT)
        .map_err(|e| e.to_string())
        .and_then(|p| Layout::load(&p).map_err(|e| e.to_string()))
    {
        Ok(f) => f,
        Err(e) => return (format!("{out} error=layout:{e}"), None),
    };
    let anchors_path = match std::env::var(env::ANCHORS) {
        Ok(p) => p,
        Err(e) => return (format!("{out} error=anchors:{e}"), None),
    };
    let hooks = match Hooks::resolve(&anchors_path) {
        Ok(h) => h,
        Err(e) => return (format!("{out} error=anchors:{e}"), None),
    };
    report_anchors(&hooks, &mut out);
    let wire = wire::install(&hooks);
    field(&mut out, "wire_detour", wire.turns);
    field(&mut out, "wire_event_detour", wire.events);
    field(&mut out, "theme_route", wire.theme);
    field(&mut out, "wire_ui", wire.ui);
    field(&mut out, "wire_surfaces", wire.surfaces);
    if let Some(port) = debug::start() {
        field(&mut out, "debug_port", port);
    }
    let r = layout::reconcile_layout(&mut layout, &hooks);
    field(
        &mut out,
        "layout_derived",
        format_args!("{}/{}", r.agreed, r.total),
    );
    if !r.corrected.is_empty() {
        field(&mut out, "layout_corrected", r.corrected.join(","));
    }
    if !r.unknown.is_empty() {
        field(
            &mut out,
            "layout_unknown",
            r.unknown.join(",").replace(' ', "_"),
        );
    }
    field(
        &mut out,
        "anchors",
        if hooks.from_healed {
            "healed"
        } else {
            "shipped"
        },
    );
    // Anchors this build weakened get new locators for the next build. It
    // takes seconds, so it runs on its own thread.
    let healing = (hooks::healable(&hooks.report) > 0).then(|| {
        let (set, report, base, path) = (
            hooks.set.clone(),
            hooks.report.clone(),
            hooks.base.clone(),
            hooks::healed_path(&anchors_path),
        );
        std::thread::spawn(move || hooks::heal_for_next_build(&set, &report, base, &path))
    });
    match frame_hook(&hooks) {
        Ok((rekindle_ui_hook::FrameHook::DrawRoots(_), _)) => {
            field(&mut out, "frame_hook", "draw_roots");
        }
        Ok((rekindle_ui_hook::FrameHook::Draw(_), _)) => field(&mut out, "frame_hook", "draw"),
        Err(e) => field(&mut out, "frame_hook", err(&e)),
    }

    // Start rekindle's plugin runtime inside the client, and check what the
    // client's own functions say about the plugins' contributions.
    let missing: Vec<&str> = CLIENT_ANCHORS
        .iter()
        .copied()
        .filter(|n| hooks.addr(n).is_err())
        .collect();
    let outcome = if missing.is_empty() {
        verify::drive_and_verify(&layout, &hooks)
    } else {
        framework_only(&layout, &hooks, &missing)
    };
    let mut kept = None;
    match outcome {
        Ok((fields, k)) => {
            out.push_str(&fields);
            kept = k;
        }
        Err(e) => field(&mut out, "surfaces", err(&e)),
    }
    // A test run's process ends after the report, so it waits for the
    // healing. A real launch leaves it running.
    if let Some(h) = healing.filter(|_| !installed_mode()) {
        match h.join() {
            Ok(Ok(n)) => field(&mut out, "healed", n),
            Ok(Err(e)) => field(&mut out, "healed", err(&e)),
            Err(_) => field(&mut out, "healed", "ERR(panic)"),
        }
    }
    (out, kept)
}

/// Each anchor's address or value and how many of its locators still hold,
/// then how many resolved and how many are whole.
fn report_anchors(hooks: &Hooks, out: &mut String) {
    for a in &hooks.report.anchors {
        let found = match hooks.values.get(&a.name) {
            Some(v) => v.as_ref().map(ToString::to_string),
            None => match hooks.addrs.get(&a.name) {
                Some(addr) => addr.as_ref().map(|addr| format!("{addr:#x}")),
                None => continue,
            },
        };
        match found {
            Ok(v) => field(
                out,
                &a.name,
                format_args!("{v}({}/{})", a.unique_locators, a.total_locators),
            ),
            Err(e) => field(out, &a.name, format_args!("ERR({e})")),
        }
    }
    let resolved = hooks.addrs.values().filter(|a| a.is_ok()).count()
        + hooks.values.values().filter(|v| v.is_ok()).count();
    let total = hooks.addrs.len() + hooks.values.len();
    let full = hooks
        .report
        .anchors
        .iter()
        .filter(|a| a.unique_locators == a.total_locators)
        .count();
    field(out, "resolved", format_args!("{resolved}/{total}"));
    field(out, "anchors_full", format_args!("{full}/{total}"));
}

/// Append ` key=value` to the report.
fn field(out: &mut String, key: &str, value: impl std::fmt::Display) {
    // Writing to a String can't fail.
    let _ = write!(out, " {key}={value}");
}

/// An error as a report value: `ERR(...)`, with no spaces.
fn err(e: &anyhow::Error) -> String {
    format!("ERR({})", format!("{e:#}").replace(' ', "_"))
}

/// `bytes` as lowercase hex.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        // Writing to a String can't fail.
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The client hook point rekindle's client surfaces need: the registries'
/// constructor, where rekindle registers its providers and entries. Every
/// other client function is only a checkpoint. One that an optimized build
/// inlined away, or that a new build broke, is `n/a` in the report, while
/// the client still calls rekindle's providers. Without it, only the
/// framework-level surfaces run (see [`framework_only`]).
const CLIENT_ANCHORS: &[&str] = &["registries_with_builtins"];

/// When some client hook points are gone: run rekindle's framework-level
/// surfaces only (real elements in the client's tree: toasts, chat-bar
/// buttons by shape, overlay sprites; theming by recolouring; plugins
/// loaded), and report which client surfaces are off and why.
fn framework_only(
    layout: &Layout,
    hooks: &Hooks,
    missing: &[&str],
) -> anyhow::Result<(String, Option<Driver>)> {
    use rekindle_extension::RecoveredProfile;
    use rekindle_extension::theme::ThemeProfile;
    profile::configure_native(layout, hooks)?;
    let profile = RecoveredProfile {
        // The theme function still hooks if it survived; framework theming
        // replaces it live anyway.
        theme: ThemeProfile {
            addr: hooks.addr("theme").ok(),
            return_size: layout.size("theme_return")?,
            offsets: layout.colors.clone(),
        },
        registry: None,
        compose: None,
        framework: Some(rekindle_ui_hook::FrameworkLayout {
            frame_hook: frame_hook(hooks)?.0,
        }),
        surfaces: crate::wire::surfaces(),
        slash_commands: false,
        wire_menus: false,
        wire_values: false,
    };
    let (ext, n_plugins) = profile::ext_args();
    // SAFETY: the profile's addresses come from anchors resolved against this
    // process's own image, its offsets from the layout checked against it,
    // and rekindle-native is configured.
    let mut driver = unsafe { Driver::start(ext, profile) }.map_err(|e| anyhow::anyhow!(e))?;
    driver.rekindle().poll();
    driver.pump();
    let loaded = driver.rekindle().infos().len();
    let kept = if live::stays() {
        driver.use_framework_chat_bar();
        live::start(&mut driver, false)?;
        Some(driver)
    } else {
        None
    };
    Ok((
        format!(
            " plugins_loaded={loaded}/{n_plugins} native_spec=true client_surfaces=off missing={}",
            missing.join(",")
        ),
        kept,
    ))
}
