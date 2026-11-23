//! The launcher end to end: installing (files, the uninstall entry) and
//! uninstalling; and updating rekindle's own files and the launcher from a
//! signed release channel.
//!
//! Placing the shim beside the client and checking it comes up needs a real
//! client that imports `dwmapi`, so it is proven against the mock client and
//! the `--check` path, not here.

use std::path::{Path, PathBuf};

#[cfg(windows)]
use rekindle_launcher::install::{self, Setup};
use rekindle_launcher::payload::Bundle;
use rekindle_launcher::state::Install;
use rekindle_launcher::update::{self, Checked};
use rekindle_release::framework::Framework;
use rekindle_release::location::Location;
use rekindle_release::signing::{self, Keypair};

const NOW: u64 = 1_800_000_000;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rekindle-launcher-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn bundle(version: &str, host: &[u8]) -> Bundle {
    Bundle {
        version: version.into(),
        files: vec![
            ("rekindle_host.dll".into(), host.to_vec()),
            ("anchors.json".into(), b"{}".to_vec()),
        ],
    }
}

#[cfg(windows)]
#[test]
fn installs_updates_and_uninstalls_with_an_uninstall_entry() {
    use rekindle_launcher::win;
    fn setup<'a>(install: &'a Install, b: &'a Bundle, client: &'a Path, key: &'a str) -> Setup<'a> {
        Setup {
            install,
            exe: b"MZ launcher",
            bundle: b,
            client,
            uninstall_key: Some(key),
        }
    }
    let dir = scratch("install");
    let install = Install::new(dir.join("root"));
    let client = std::env::current_exe().unwrap();
    let key = format!(r"Software\rekindle-launcher-test-{}", std::process::id());

    let v1 = bundle("1.0.0", b"host 1");
    let state = install::install(&setup(&install, &v1, &client, &key)).unwrap();
    assert_eq!(state.current.as_deref(), Some("1.0.0"));
    assert_eq!(state.client.as_deref(), Some(client.as_path()));
    assert_eq!(std::fs::read(install.exe()).unwrap(), b"MZ launcher");
    v1.framework()
        .check_dir(&install.version_dir("1.0.0"))
        .unwrap();
    assert!(
        win::uninstall_entry_exists(&key),
        "Add/Remove Programs lists it"
    );

    assert_eq!(
        state.launcher.as_deref(),
        Some(rekindle_launcher::LAUNCHER_VERSION)
    );

    // A newer setup over the install: pending, so the next placement tries it.
    let state =
        install::install(&setup(&install, &bundle("1.1.0", b"host 2"), &client, &key)).unwrap();
    assert_eq!(
        (state.current.as_deref(), state.pending.as_deref()),
        (Some("1.0.0"), Some("1.1.0"))
    );

    // A setup older than a launcher the install updated to keeps that launcher.
    let mut state = install.state().unwrap();
    state.launcher = Some("99.0.0".into());
    install.save(&state).unwrap();
    std::fs::write(install.exe(), b"MZ newer launcher").unwrap();
    let state =
        install::install(&setup(&install, &bundle("1.2.0", b"host 3"), &client, &key)).unwrap();
    assert_eq!(std::fs::read(install.exe()).unwrap(), b"MZ newer launcher");
    assert_eq!(state.launcher.as_deref(), Some("99.0.0"));

    install::uninstall(&install, Some(&key)).unwrap();
    assert!(!win::uninstall_entry_exists(&key));
    assert!(!install.versions_dir().exists() && !install.is_installed());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Kept running by `uninstall_removes_the_running_program_and_keeps_other_files`.
#[test]
#[ignore = "a stand-in for the running rekindle.exe"]
fn stand_in_for_the_running_program() {
    std::thread::sleep(std::time::Duration::from_secs(20));
}

#[cfg(windows)]
#[test]
fn uninstall_removes_the_running_program_and_keeps_other_files() {
    let dir = scratch("uninstall-running");
    let install = Install::new(dir.join("root"));
    let client = std::env::current_exe().unwrap();
    let b = bundle("1.0.0", b"host 1");
    install::install(&Setup {
        install: &install,
        exe: b"MZ placeholder",
        bundle: &b,
        client: &client,
        uninstall_key: None,
    })
    .unwrap();
    // The installed program, running, as when Settings › Apps runs the uninstall.
    std::fs::copy(&client, install.exe()).unwrap();
    let mut running = std::process::Command::new(install.exe())
        .args(["--ignored", "--exact", "stand_in_for_the_running_program"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        std::fs::remove_file(install.exe()).is_err(),
        "Windows refuses to delete a running program"
    );
    // Something the installer didn't put there, like the CLI's `bin`.
    std::fs::create_dir_all(install.root().join("bin")).unwrap();
    std::fs::write(install.root().join("bin").join("rekindle-cli.exe"), b"cli").unwrap();

    install::uninstall(&install, None).unwrap();
    assert!(
        !install.exe().exists(),
        "the running program is gone from the install directory"
    );
    assert!(!install.is_installed());
    assert!(
        install
            .root()
            .join("bin")
            .join("rekindle-cli.exe")
            .is_file(),
        "files uninstall didn't place stay"
    );

    let _ = running.kill();
    let _ = running.wait();
    for e in std::fs::read_dir(std::env::temp_dir()).unwrap().flatten() {
        let path = e.path();
        let parked = path.extension().is_some_and(|x| x == "old")
            && e.file_name()
                .to_string_lossy()
                .starts_with("rekindle-uninstalled-");
        if parked {
            let _ = std::fs::remove_file(path);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A release channel in `dist` offering framework `version` (its files
/// copied to `dist/framework/<version>/`), signed by `key`.
fn publish(
    dist: &Path,
    key: &Keypair,
    serial: u64,
    expires: u64,
    b: &Bundle,
) -> (Location, Framework) {
    let _ = std::fs::remove_dir_all(dist);
    let files = dist.join("framework").join(&b.version);
    std::fs::create_dir_all(&files).unwrap();
    for (name, bytes) in &b.files {
        std::fs::write(files.join(name), bytes).unwrap();
    }
    let fw = Framework::from_dir(&files, &b.version).unwrap();
    let channel = serde_json::json!({
        "format": 1, "name": "test", "serial": serial, "expires": expires, "plugins": [], "blocked": [], "framework": fw,
    });
    let bytes = serde_json::to_vec_pretty(&channel).unwrap();
    let path = dist.join("channel.json");
    std::fs::write(&path, &bytes).unwrap();
    key.sign_channel(&bytes)
        .save(&signing::sig_path_for(&path))
        .unwrap();
    (Location::Path(path), fw)
}

#[test]
fn a_newer_framework_downloads_from_a_signed_channel_and_nothing_else_does() {
    let dir = scratch("update");
    let install = Install::new(dir.join("root"));
    let project = Keypair::generate("Project").unwrap();
    let keys = vec![project.public()];
    let mut state = install.state().unwrap();
    state.current = Some("1.0.0".into());
    state.started("1.0.0");

    // Same version: nothing to do.
    let (loc, _) = publish(
        &dir.join("dist0"),
        &project,
        10,
        NOW + 1000,
        &bundle("1.0.0", b"host 1"),
    );
    assert_eq!(
        framework_update(&install, &mut state, &loc, &keys).unwrap(),
        Checked::UpToDate
    );

    // Newer: downloaded, checked, pending.
    let v11 = bundle("1.1.0", b"host 2");
    let (loc, fw) = publish(&dir.join("dist1"), &project, 11, NOW + 1000, &v11);
    assert_eq!(
        framework_update(&install, &mut state, &loc, &keys).unwrap(),
        Checked::Downloaded("1.1.0".into())
    );
    assert_eq!(state.pending.as_deref(), Some("1.1.0"));
    fw.check_dir(&install.version_dir("1.1.0")).unwrap();
    assert_eq!(state.channel_serial, 11);

    // Refused: an older serial, an expired channel, another key, a file
    // that doesn't match the listing. None of them changes what's pending.
    let v12 = bundle("1.2.0", b"host 3");
    let (loc, _) = publish(&dir.join("dist2"), &project, 9, NOW + 1000, &v12);
    assert!(
        framework_update(&install, &mut state, &loc, &keys)
            .unwrap_err()
            .to_string()
            .contains("roll back")
    );
    let (loc, _) = publish(&dir.join("dist3"), &project, 12, NOW - 1, &v12);
    assert!(
        framework_update(&install, &mut state, &loc, &keys)
            .unwrap_err()
            .to_string()
            .contains("expired")
    );
    let other = Keypair::generate("Someone").unwrap();
    let (loc, _) = publish(&dir.join("dist4"), &other, 13, NOW + 1000, &v12);
    assert!(
        framework_update(&install, &mut state, &loc, &keys)
            .unwrap_err()
            .to_string()
            .contains("not a project key")
    );
    let (loc, _) = publish(&dir.join("dist5"), &project, 14, NOW + 1000, &v12);
    std::fs::write(
        dir.join("dist5/framework/1.2.0/rekindle_host.dll"),
        b"tampered",
    )
    .unwrap();
    assert!(framework_update(&install, &mut state, &loc, &keys).is_err());
    assert!(!install.version_dir("1.2.0").exists());
    assert_eq!(state.pending.as_deref(), Some("1.1.0"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A release channel in `dist` offering `exe` as launcher `version`, at
/// `dist/launcher/<version>/rekindle.exe`.
#[cfg(windows)]
fn publish_launcher(
    dist: &Path,
    key: &Keypair,
    serial: u64,
    exe: &Path,
    version: &str,
) -> Location {
    let _ = std::fs::remove_dir_all(dist);
    let base = format!("launcher/{version}/");
    let files = dist.join(&base);
    std::fs::create_dir_all(&files).unwrap();
    std::fs::copy(exe, files.join("rekindle.exe")).unwrap();
    let listing = Framework::from_dir_at(&files, version, &base).unwrap();
    let channel = serde_json::json!({
        "format": 1, "name": "test", "serial": serial, "expires": NOW + 1000, "plugins": [], "blocked": [], "launcher": listing,
    });
    let bytes = serde_json::to_vec_pretty(&channel).unwrap();
    let path = dist.join("channel.json");
    std::fs::write(&path, &bytes).unwrap();
    key.sign_channel(&bytes)
        .save(&signing::sig_path_for(&path))
        .unwrap();
    Location::Path(path)
}

fn framework_update(
    install: &Install,
    state: &mut rekindle_launcher::state::State,
    loc: &Location,
    keys: &[signing::PublicKey],
) -> anyhow::Result<Checked> {
    let head = update::channel(state, loc, keys, NOW)?;
    update::framework(install, state, loc, &head)
}

#[cfg(windows)]
fn launcher_update(
    install: &Install,
    state: &mut rekindle_launcher::state::State,
    loc: &Location,
    keys: &[signing::PublicKey],
) -> anyhow::Result<Option<String>> {
    let head = update::channel(state, loc, keys, NOW)?;
    update::launcher(install, state, loc, &head)
}

#[cfg(windows)]
#[test]
fn a_newer_launcher_replaces_the_running_one_after_it_starts() {
    let dir = scratch("launcher");
    let install = Install::new(dir.join("root"));
    let project = Keypair::generate("Project").unwrap();
    let keys = vec![project.public()];
    let real = PathBuf::from(env!("CARGO_BIN_EXE_rekindle-launcher"));
    let version = rekindle_launcher::LAUNCHER_VERSION;

    // The installed program, running, as when `rekindle.exe --update` runs.
    std::fs::create_dir_all(install.root()).unwrap();
    std::fs::copy(std::env::current_exe().unwrap(), install.exe()).unwrap();
    let before = std::fs::read(install.exe()).unwrap();
    let mut running = std::process::Command::new(install.exe())
        .args(["--ignored", "--exact", "stand_in_for_the_running_program"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(500));
    let mut state = install.state().unwrap();
    state.launcher = Some("0.1.0".into());

    // A launcher that prints another version than its listing doesn't start
    // as that version: refused, never fetched again, the installed one stays.
    let loc = publish_launcher(&dir.join("dist0"), &project, 10, &real, "99.0.0");
    let e = launcher_update(&install, &mut state, &loc, &keys).unwrap_err();
    assert!(format!("{e:#}").contains("--version"), "{e:#}");
    assert!(state.bad_launchers.contains("99.0.0"));
    assert_eq!(
        launcher_update(&install, &mut state, &loc, &keys).unwrap(),
        None
    );
    assert_eq!(std::fs::read(install.exe()).unwrap(), before);
    assert!(!install.launcher_update_dir().exists());

    // The real one: checked, and in place of the running program.
    let loc = publish_launcher(&dir.join("dist1"), &project, 11, &real, version);
    assert_eq!(
        launcher_update(&install, &mut state, &loc, &keys)
            .unwrap()
            .as_deref(),
        Some(version)
    );
    assert_eq!(
        std::fs::read(install.exe()).unwrap(),
        std::fs::read(&real).unwrap()
    );
    assert_eq!(
        std::fs::read(install.old_exe()).unwrap(),
        before,
        "the running program moved aside"
    );
    assert_eq!(state.launcher.as_deref(), Some(version));
    assert_eq!(
        launcher_update(&install, &mut state, &loc, &keys).unwrap(),
        None,
        "not again"
    );

    // Once it isn't running, the next run's clean-up deletes the old one.
    let _ = running.kill();
    let _ = running.wait();
    install.gc(&state);
    assert!(!install.old_exe().exists());

    // A tampered file isn't run at all.
    let loc = publish_launcher(&dir.join("dist2"), &project, 12, &real, "99.1.0");
    std::fs::write(
        dir.join("dist2/launcher/99.1.0/rekindle.exe"),
        b"MZ tampered",
    )
    .unwrap();
    assert!(launcher_update(&install, &mut state, &loc, &keys).is_err());
    assert!(
        !state.bad_launchers.contains("99.1.0"),
        "a bad download isn't the version's fault"
    );
    assert_eq!(
        std::fs::read(install.exe()).unwrap(),
        std::fs::read(&real).unwrap()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
