//! Updating an install's files and its launcher from the release channel
//! ([`run`]). The launcher runs it when it runs, and the host runs it from
//! inside the client.
//!
//! The channel ([`channel`]) must be signed by a project key, not older
//! than a channel already accepted, and not expired.
//!
//! The `framework` section ([`framework`]) names a version of rekindle's
//! files. A version newer than the installed one and not one that failed
//! before is downloaded and checked against the listing into
//! `versions/<version>/`, and becomes `pending`. The next launch of the
//! client tries it first, and falls back if it doesn't come up
//! (`rekindle_install::State::launch`).
//! The `launcher` section ([`launcher`]) names a newer `rekindle.exe`. It
//! is downloaded into `launcher-update\`, checked against the listing, and
//! run with `--version`, which must print the version listed. Windows lets a
//! running program be renamed but not replaced, so the installed one moves
//! to `rekindle.exe.old` and the new one takes its name; the next run
//! deletes the old one. A launcher that doesn't pass the `--version` check
//! is never downloaded again, and the installed one stays.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use rekindle_install::{EXE_NAME, Install, State};

use crate::framework::{self, Head};
use crate::location::Location;
use crate::signing::PublicKey;

/// How long a downloaded launcher has to answer `--version`.
const VERSION_TIMEOUT: Duration = Duration::from_secs(15);

/// What a framework check did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checked {
    /// The channel has nothing newer.
    UpToDate,
    /// Downloaded and pending: the next launch tries it first.
    Downloaded(String),
}

/// What [`run`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    pub framework: Checked,
    /// The launcher version installed, if one was.
    pub launcher: Option<String>,
}

/// Check the channel at `location` for the install: replace the launcher,
/// download a newer framework, save the state and remove what it no longer
/// keeps. A launcher that can't be replaced is logged and doesn't stop the
/// framework.
pub fn run(install: &Install, location: &Location, keys: &[PublicKey], now: u64) -> Result<Ran> {
    let mut state = install.state()?;
    let ran = (|| -> Result<Ran> {
        let head = channel(&mut state, location, keys, now)?;
        let launcher = launcher(install, &mut state, location, &head).unwrap_or_else(|e| {
            install.log(&format!("launcher update: {e:#}"));
            None
        });
        Ok(Ran {
            framework: framework(install, &mut state, location, &head)?,
            launcher,
        })
    })()
    .context("update check failed");
    install.save(&state)?;
    install.gc(&state);
    let ran = ran?;
    install.log(&format!("update check: {:?}", ran.framework));
    Ok(ran)
}

/// Read and verify the channel at `location`. Updates the accepted serial
/// in `state`; the caller saves it.
pub fn channel(
    state: &mut State,
    location: &Location,
    keys: &[PublicKey],
    now: u64,
) -> Result<Head> {
    let (bytes, sig) = framework::fetch_channel(location)?;
    let head = framework::verify_head(&bytes, &sig, keys)?;
    if head.expires <= now {
        bail!("the release channel expired (serial {})", head.serial);
    }
    if head.serial < state.channel_serial {
        bail!(
            "the release channel has serial {}, older than one already accepted ({}); refusing to roll back",
            head.serial,
            state.channel_serial
        );
    }
    state.channel_serial = head.serial;
    Ok(head)
}

/// Download the channel's framework if it is newer. Updates `state`
/// (pending version); the caller saves it.
pub fn framework(
    install: &Install,
    state: &mut State,
    location: &Location,
    head: &Head,
) -> Result<Checked> {
    let Some(fw) = &head.framework else {
        return Ok(Checked::UpToDate);
    };
    if !state.wants(&fw.version) {
        return Ok(Checked::UpToDate);
    }
    let dest = install.version_dir(&fw.version);
    if dest.exists() && fw.check_dir(&dest).is_err() {
        std::fs::remove_dir_all(&dest)?;
    }
    if !dest.exists() {
        let installed: Vec<_> = state
            .keep()
            .iter()
            .map(|v| install.version_dir(v))
            .filter(|d| d.is_dir())
            .collect();
        let got = fw.fetch(location, &dest, &installed)?;
        install.log(&format!(
            "framework {}: {} file(s) kept from an installed version, {} downloaded ({} bytes)",
            fw.version, got.reused, got.downloaded, got.bytes
        ));
    }
    state.pending = Some(fw.version.clone());
    Ok(Checked::Downloaded(fw.version.clone()))
}

/// Replace the installed `rekindle.exe` with the channel's launcher if it
/// is newer. Returns the version installed, if one was. Updates `state`
/// (installed and bad launcher versions); the caller saves it.
pub fn launcher(
    install: &Install,
    state: &mut State,
    location: &Location,
    head: &Head,
) -> Result<Option<String>> {
    let Some(l) = &head.launcher else {
        return Ok(None);
    };
    if !state.wants_launcher(&l.version) {
        return Ok(None);
    }
    let dir = install.launcher_update_dir();
    // Left over from an interrupted update, if it exists.
    let _ = std::fs::remove_dir_all(&dir);
    let result = (|| {
        l.fetch(location, &dir, &[])?;
        let new = dir.join(EXE_NAME);
        if let Err(e) = starts_as(&new, &l.version) {
            state.bad_launchers.insert(l.version.clone());
            return Err(e.context(format!(
                "launcher {} didn't start, so it won't be downloaded again",
                l.version
            )));
        }
        replace(&install.exe(), &install.old_exe(), &new)
    })();
    // Best effort: `Install::gc` removes it too.
    let _ = std::fs::remove_dir_all(&dir);
    result?;
    state.launcher = Some(l.version.clone());
    install.log(&format!("launcher {} installed", l.version));
    Ok(Some(l.version.clone()))
}

/// Run `exe --version` and check it prints `rekindle-launcher <version>`.
fn starts_as(exe: &Path, version: &str) -> Result<()> {
    let mut command = std::process::Command::new(exe);
    command
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))?;
    let start = Instant::now();
    while child.try_wait()?.is_none() {
        if start.elapsed() >= VERSION_TIMEOUT {
            // It may have ended in the meantime.
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "{} didn't answer --version within {VERSION_TIMEOUT:?}",
                exe.display()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output()?;
    let printed = String::from_utf8_lossy(&out.stdout);
    let expected = format!("rekindle-launcher {version}");
    if !out.status.success() || printed.trim() != expected {
        bail!(
            "{} printed `{}` for --version, not `{expected}`",
            exe.display(),
            printed.trim()
        );
    }
    Ok(())
}

/// Put `new` at `exe`, moving the program there (which may be running) to
/// `old`. If `new` can't be moved in, the program goes back.
fn replace(exe: &Path, old: &Path, new: &Path) -> Result<()> {
    // The program an earlier update replaced, if it is still there.
    let _ = std::fs::remove_file(old);
    let had = exe.exists();
    if had {
        std::fs::rename(exe, old).with_context(|| format!("moving {} aside", exe.display()))?;
    }
    if let Err(e) = std::fs::rename(new, exe) {
        if had {
            // Putting it back is all that can be done; the error below says
            // what failed.
            let _ = std::fs::rename(old, exe);
        }
        return Err(e).with_context(|| format!("moving the new launcher to {}", exe.display()));
    }
    Ok(())
}
