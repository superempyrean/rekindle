//! The launcher's check and the self-tests: copying the host's report out of
//! the live client, and proving the client's loader mapped the shim.

use std::time::{Duration, Instant};

use rekindle_install::env;

use crate::{host, sys, system_dir, trace};

/// Test only: run [`prove_and_exit`] in the initializer, writing to this file.
pub(crate) const PROVE_ENV: &str = "REKINDLE_PROXY_PROVE";
/// Test only: the file the worker writes `loaded` or `nohost` to.
pub(crate) const MARKER_ENV: &str = "REKINDLE_PROXY_MARKER";
/// The file rekindle writes what its first plugin update check did to
/// (`extension/src/rekindle.rs`). A check waits for it.
const PLUGINS_UPDATED_ENV: &str = "REKINDLE_HOST_PLUGINS_UPDATED";

/// The check: wait for the host's report and copy it to `out`, wait for the
/// first update checks a test asked about, then end the off-screen client.
pub(crate) fn run(out: &str) -> ! {
    trace(&copy_report(out));
    for var in [env::UPDATED, PLUGINS_UPDATED_ENV] {
        if let Ok(path) = std::env::var(var) {
            trace(&format!("{var}: {}", wait_for(&path)));
        }
    }
    // SAFETY: ending the process is always allowed.
    unsafe { sys::ExitProcess(0) }
}

/// Wait up to 120 s for the file `path` to be written.
fn wait_for(path: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if std::fs::metadata(path).is_ok_and(|m| m.len() > 0) {
            return "prove: the update check finished".into();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    "prove: the update check didn't finish within 120 s".into()
}

/// Copy the host's handshake report to `out`; returns what happened, for the
/// trace.
fn copy_report(out: &str) -> String {
    let Ok(handshake) = std::env::var(env::HANDSHAKE) else {
        return format!("prove: {} is not set", env::HANDSHAKE);
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Ok(report) = std::fs::read_to_string(&handshake)
            && !report.trim().is_empty()
        {
            return match std::fs::write(out, report) {
                Ok(()) => "prove: report copied".into(),
                Err(e) => format!("prove: writing the report failed: {e}"),
            };
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    "prove: the host didn't report within 30 s".into()
}

/// Test only (`REKINDLE_PROXY_PROVE=<path>`): runs in the loader's
/// initialization of the shim, before the client's `main`, so it records that
/// the client's own loader mapped it, then ends the process before any window
/// exists. It loads nothing, so it never re-enters the loader: it writes who
/// loaded the shim, what it forwards to and which host it would load.
pub(crate) fn prove_and_exit(out: &str) -> ! {
    let shim = host::self_path();
    let exe = host::main_exe_path();
    let host = shim
        .as_ref()
        .and_then(|p| p.parent())
        .map(|d| d.join(host::host_name()));
    let real = system_dir()
        .map(|d| d.join("dwmapi.dll"))
        .unwrap_or_default();
    let shown =
        |p: Option<&std::path::Path>| p.map_or_else(|| "?".into(), |p| p.display().to_string());
    let report = format!(
        "loaded_as={}\nloaded_into={}\nbefore_main=true\nforward_target={} (exists={})\nhost={} (present={})\n",
        shown(shim.as_deref()),
        shown(exe.as_deref()),
        real.display(),
        host::file_exists(&real),
        shown(host.as_deref()),
        host.as_deref().is_some_and(host::file_exists),
    );
    // The process ends either way; a missing file is the test's failure.
    let _ = std::fs::write(out, report);
    // SAFETY: ending the process is always allowed.
    unsafe { sys::ExitProcess(0) }
}
