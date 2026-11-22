//! rekindle's one program for users (`rekindle-setup.exe`, installed as
//! `rekindle.exe`).
//!
//! rekindle loads through the side-load shim: a `dwmapi.dll` placed beside the
//! client exe, which the client's own loader maps on every launch and which
//! loads the host. So this program is not a launcher and
//! sits in no launch path. It is an installer and a repair tool, the way
//! Vencord's installer is:
//!
//! - **The first time** (run from a download, with rekindle's files appended:
//!   [`payload`]) it asks once, installs per user ([`install`]), places the
//!   shim and the best version beside the client, registers an Add/Remove
//!   Programs entry, and offers to open the client.
//! - **Re-run** (or `--update`): it re-places the shim after a client update
//!   moved the exe, checks the release channel for a newer version of
//!   rekindle's own files ([`update`]), and keeps the best one that comes up.
//! - **Uninstall** removes the shim from the client's directory; the client is
//!   stock again.
//! - Everything else a user does (plugins, developer mode, reviews) is in
//!   rekindle's panel inside the client (`rekindle_plugin_host::manager`).
//!
//! Updates (rekindle's files and this program) happen when the setup,
//! `rekindle.exe` or `rekindle.exe --update` runs. No service, no
//! auto-start, no background process of our own.

pub mod install;
pub mod launch;
pub mod payload;
pub mod ui;
pub mod win;
#[cfg(windows)]
pub mod window;

/// The install's layout and version state (`rekindle-install`).
pub mod state {
    pub use rekindle_install::*;
}
pub use rekindle_release::update;

use std::path::Path;

use launch::Verdict;
use state::{Install, State};

/// This program's version. The release channel's `launcher` listing names
/// a newer one to replace it with (`update::launcher`).
pub const LAUNCHER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A version that didn't come up this time.
#[derive(Debug)]
pub struct Failure {
    pub version: String,
    pub why: String,
    /// Marked bad, so it's not tried again: it never came up before, and it
    /// reported that it can't, or its checks ended with no report
    /// [`state::NO_REPORT_LIMIT`] times in a row.
    pub marked_bad: bool,
}

/// What [`place_best`] did.
#[derive(Debug)]
pub struct Placed {
    /// The version placed and running the check, and its report; `None` if
    /// none came up (the shim was removed, so the client runs stock).
    pub placed: Option<(String, String)>,
    pub failures: Vec<Failure>,
}

/// Place the best version that comes up beside `client`, best first
/// ([`State::candidates`]), checking each off the desktop through the shim and
/// keeping the first whose host reports clean. A check that ends with no
/// report runs once more before it counts. Records each result in `state`
/// (the caller saves it). If none comes up, the shim is removed so the client
/// runs stock.
pub fn place_best(install: &Install, state: &mut State, client: &Path) -> Placed {
    let mut out = Placed {
        placed: None,
        failures: Vec::new(),
    };
    for v in state.candidates() {
        let version_dir = install.version_dir(&v);
        let check = || match launch::place(client, &version_dir) {
            Ok(()) => launch::check(client, &version_dir),
            Err(e) => Verdict::NoReport(format!("{e:#}")),
        };
        let mut verdict = check();
        if let Verdict::NoReport(why) = &verdict {
            install.log(&format!("{v}: no report ({why}); checking again"));
            verdict = check();
        }
        let (why, marked_bad) = match verdict {
            Verdict::Up(report) => {
                state.started(&v);
                install.log(&format!("placed {v}"));
                out.placed = Some((v, report));
                break;
            }
            Verdict::Refused(why) => {
                let bad = state.failed(&v);
                (why, bad)
            }
            Verdict::NoReport(why) => {
                let bad = state.unreported(&v);
                (why, bad)
            }
        };
        install.log(&format!(
            "{v} didn't come up{}: {why}",
            if marked_bad { " (marked bad)" } else { "" }
        ));
        out.failures.push(Failure {
            version: v,
            why,
            marked_bad,
        });
    }
    if out.placed.is_none() {
        // Nothing came up: leave the client stock rather than a half-placed
        // shim (the shim would fail safe anyway, but a clean directory is
        // clearer).
        let _ = launch::unplace(client);
    }
    out
}
