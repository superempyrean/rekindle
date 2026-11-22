//! Installing and uninstalling, without the dialogs (`main` asks; this
//! does). Per user, no administrator rights, nothing that starts by itself.
//!
//! Installing writes rekindle's files into the install directory and records
//! the client. Placing the shim beside the client (so its loader maps
//! rekindle) is a separate step the caller runs after, with a check and a
//! fallback (`crate::place_best`). Uninstalling removes the placed shim and
//! everything in the install directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::launch::{self, write_atomic};
use crate::payload::Bundle;
use crate::state::{Install, State};
use crate::win;

/// What to install.
pub struct Setup<'a> {
    pub install: &'a Install,
    /// The launcher program (the exe part of the setup program).
    pub exe: &'a [u8],
    pub bundle: &'a Bundle,
    /// The client program rekindle is placed beside.
    pub client: &'a Path,
    /// The Add/Remove Programs entry (`win::UNINSTALL_KEY`), if any.
    pub uninstall_key: Option<&'a str>,
}

/// Put a bundle's files in `versions/<version>/`, unless they are there
/// already.
fn put_version(install: &Install, bundle: &Bundle) -> Result<()> {
    let dest = install.version_dir(&bundle.version);
    if dest.exists() {
        if bundle.framework().check_dir(&dest).is_ok() {
            return Ok(());
        }
        std::fs::remove_dir_all(&dest).with_context(|| format!("replacing {}", dest.display()))?;
    }
    let mut partial = dest.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    // Left over from an interrupted install, if it exists.
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial).with_context(|| format!("creating {}", partial.display()))?;
    for (name, bytes) in &bundle.files {
        std::fs::write(partial.join(name), bytes).with_context(|| format!("writing {name}"))?;
    }
    std::fs::rename(&partial, &dest)
        .with_context(|| format!("moving {} into place", dest.display()))
}

/// Install (or update an install). Returns the state written. A first
/// install makes the bundle's version current; installing a newer one
/// over an install makes it pending (the caller places and checks the best
/// one, falling back if it doesn't come up). This does not place the shim
/// beside the client: see [`crate::place_best`].
pub fn install(s: &Setup) -> Result<State> {
    let root = s.install.root();
    std::fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
    let exe = s.install.exe();
    let mut state = s.install.state()?;
    // An older setup run over an install that has updated its launcher keeps
    // the newer launcher.
    if !(exe.is_file() && state.launcher_newer_than(crate::LAUNCHER_VERSION)) {
        if std::fs::read(&exe).ok().as_deref() != Some(s.exe) {
            write_atomic(&exe, s.exe)?;
        }
        state.launcher = Some(crate::LAUNCHER_VERSION.into());
    }
    put_version(s.install, s.bundle)?;

    let v = s.bundle.version.clone();
    // Installing a version by hand is a fresh chance for it.
    state.bad.remove(&v);
    if state.current.is_none() {
        state.current = Some(v);
    } else if state.wants(&v) {
        state.pending = Some(v);
    }
    state.client = Some(s.client.to_path_buf());
    s.install.save(&state)?;
    s.install.gc(&state);

    if let Some(key) = s.uninstall_key {
        let size: u64 = s
            .bundle
            .files
            .iter()
            .map(|(_, b)| b.len() as u64)
            .sum::<u64>()
            + s.exe.len() as u64;
        let e = win::UninstallEntry {
            version: &s.bundle.version,
            root,
            exe: &exe,
            size_kb: (size / 1024) as u32,
        };
        win::write_uninstall_entry(key, &e)?;
    }
    Ok(state)
}

/// Remove the shim from the client's directory, the uninstall entry, and the
/// files the installer put in the install directory, then the directory if
/// nothing else is in it (the CLI's `bin`, for one, stays). The user's plugins
/// are not touched here.
pub fn uninstall(install: &Install, uninstall_key: Option<&str>) -> Result<()> {
    let state = install.state()?;
    // Best effort from here to the program: whatever stays behind is
    // unused once `state.json` and the shim are gone.
    if let Some(client) = &state.client {
        let _ = launch::unplace(client);
    }
    if let Some(key) = uninstall_key {
        win::remove_uninstall_entry(key);
    }
    let _ = std::fs::remove_dir_all(install.versions_dir());
    let _ = std::fs::remove_dir_all(install.launcher_update_dir());
    let _ = std::fs::remove_file(install.old_exe());
    for f in [
        install.state_path(),
        install.log_path(),
        install.old_log_path(),
    ] {
        let _ = std::fs::remove_file(f);
    }
    remove_program(&install.exe())?;
    // Fails while anything else is in it, which then stays.
    let _ = std::fs::remove_dir(install.root());
    Ok(())
}

/// Remove the installed program. When it is the program running the
/// uninstall, Windows refuses to delete it but allows renaming it, so it is
/// moved to the temp directory, where Windows' own cleanup deletes it later.
fn remove_program(exe: &Path) -> Result<()> {
    if !exe.exists() || std::fs::remove_file(exe).is_ok() {
        return Ok(());
    }
    let parked = launch::temp_path("uninstalled").with_extension("old");
    std::fs::rename(exe, &parked).with_context(|| {
        format!(
            "removing {} (close rekindle and delete it by hand)",
            exe.display()
        )
    })
}
