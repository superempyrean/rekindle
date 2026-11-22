//! Placing rekindle beside the client, and checking it comes up.
//!
//! rekindle loads through the side-load shim (`dwmapi.dll`): placed beside
//! the client exe, the client's own loader maps it on every launch and it
//! loads the host. So there is no
//! launcher in the client's launch path, and nothing to resume: the user
//! opens the client as they always have.
//!
//! The host and the data it reads stay in the install's version folders.
//! On a launch, the shim reads `state.json` and loads the host from the
//! version it picks (`rekindle_install::State::launch`), so a version the
//! host downloads while the client runs takes effect on the next launch.
//!
//! What this module does instead:
//! - [`place`] copies one version's shim into the client's own directory.
//! - [`check`] runs the client off the desktop through the shim, with the
//!   host of one version folder, and reads the host's report, so a version
//!   can be proven before it is kept. The run opens no window, takes no
//!   focus, and ends itself.
//! - [`open`] starts the client for the user. The shim is already in place, so
//!   this is a plain launch.
//!
//! A version whose host cannot come up leaves the client working: the shim
//! forwards `dwmapi` and loads nothing, so the client runs as it did before
//! rekindle. [`check`] is how the installer notices and falls back anyway.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use rekindle_install::{env, healthy};

/// The side-load shim the client's loader maps. Its file name is what the
/// client imports (`dwmapi`), so our copy beside the exe resolves first.
pub const SHIM_DLL: &str = "dwmapi.dll";

/// The host library the shim loads.
pub const HOST_DLL: &str = rekindle_install::HOST_DLL;

/// The recovered data the host reads, beside it.
pub const DATA: &[&str] = &["layout.json", "anchors.json", "palette.json"];

/// Every file a placed version is made of: the shim, the host, the data.
pub fn version_files() -> Vec<&'static str> {
    let mut v = vec![SHIM_DLL, HOST_DLL];
    v.extend_from_slice(DATA);
    v
}

/// The directory the shim is placed in: the client's own directory, where its
/// loader resolves `dwmapi` from.
pub fn client_dir(client: &Path) -> Result<&Path> {
    client
        .parent()
        .with_context(|| format!("{} has no parent directory", client.display()))
}

/// How long to wait for the off-screen check to report before calling it a
/// failure. Generous; the report normally lands in well under a second.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// Write `path` through `<path>.tmp`, so a reader never sees half a file.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
}

/// A path in the temp directory no other run uses: `rekindle-<tag>-<pid>-<nanos>`.
pub(crate) fn temp_path(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("rekindle-{tag}-{}-{nanos}", std::process::id()))
}

/// Copy a version's shim from `version_dir` into the client's directory, so
/// the client's loader maps it on its next launch, and remove the host and
/// data an older rekindle placed there. Touches nothing else in the
/// directory.
pub fn place(client: &Path, version_dir: &Path) -> Result<()> {
    let dir = client_dir(client)?;
    let src = version_dir.join(SHIM_DLL);
    let bytes = std::fs::read(&src).with_context(|| format!("reading {}", src.display()))?;
    write_atomic(&dir.join(SHIM_DLL), &bytes)?;
    // Most of these were never there; one that can't be removed is only
    // unused, since the shim loads the host from the version's folder.
    for name in version_files().into_iter().filter(|n| *n != SHIM_DLL) {
        let _ = std::fs::remove_file(dir.join(name));
    }
    Ok(())
}

/// Remove rekindle's placed files from the client's directory, leaving the
/// client stock. Missing files are not an error.
pub fn unplace(client: &Path) -> Result<()> {
    let dir = client_dir(client)?;
    // Each file is removed if it is there; a missing one is the goal.
    for name in version_files() {
        let _ = std::fs::remove_file(dir.join(name));
    }
    Ok(())
}

/// What [`check`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// rekindle came up. The host's report.
    Up(String),
    /// rekindle reported from inside the client that it couldn't start. The
    /// version is at fault.
    Refused(String),
    /// No report: the client couldn't start, ended first, or the check timed
    /// out. That can be the client or the machine at that moment as much as
    /// the version.
    NoReport(String),
}

/// Run the client off the desktop through the shim, with the host and
/// data in `version_dir`, and read the host's report. The shim must already
/// be placed (see [`place`]). Opens no window, takes no focus, and ends the
/// client itself.
pub fn check(client: &Path, version_dir: &Path) -> Verdict {
    match run_check(client, version_dir) {
        Ok(report) => match healthy(&report) {
            Ok(()) => Verdict::Up(report),
            Err(e) => Verdict::Refused(format!("{e:#}")),
        },
        Err(e) => Verdict::NoReport(format!("{e:#}")),
    }
}

/// The host's report, or why there was none. The client has ended either way.
fn run_check(client: &Path, version_dir: &Path) -> Result<String> {
    let dir = client_dir(client)?;
    if !dir.join(SHIM_DLL).is_file() {
        bail!("the shim {} is not beside {}", SHIM_DLL, client.display());
    }
    let report = temp_path("check-report");
    let handshake = temp_path("check-handshake");
    let trace = temp_path("check-trace");

    let mut command = std::process::Command::new(client);
    command
        .env(env::CHECK, &report)
        .env(env::HANDSHAKE, &handshake)
        .env(env::TRACE, &trace)
        .env(env::HOST_DIR, version_dir);
    // The report comes through a file. Inherited output handles could be a
    // pipe nobody reads any more, and the host panics writing to one.
    let null = std::process::Stdio::null;
    command.stdin(null()).stdout(null()).stderr(null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // A client built as a console program gets no console window. Run
        // from the setup window, it would get one.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("starting {} for the check", client.display()))?;

    let start = Instant::now();
    let out = loop {
        if let Ok(text) = std::fs::read_to_string(&report)
            && !text.trim().is_empty()
        {
            break Ok(text);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                // The client ended. One more read, then give up.
                match std::fs::read_to_string(&report) {
                    Ok(text) if !text.trim().is_empty() => break Ok(text),
                    _ => {
                        break Err(anyhow::anyhow!(
                            "the client ended ({}) before rekindle reported",
                            exit_code(status)
                        ));
                    }
                }
            }
            Ok(None) => {}
            Err(e) => break Err(anyhow::Error::new(e).context("waiting for the check")),
        }
        if start.elapsed() >= CHECK_TIMEOUT {
            break Err(anyhow::anyhow!(
                "rekindle didn't report within {CHECK_TIMEOUT:?}"
            ));
        }
        std::thread::sleep(Duration::from_millis(30));
    };
    // The shim ends the client itself, so the kill usually finds it gone.
    let _ = child.kill();
    let _ = child.wait();
    let shim = std::fs::read_to_string(&trace).unwrap_or_default();
    // Whichever of them were written. One that can't be removed stays in
    // the temp directory, under a name no other check uses.
    for f in [&report, &handshake, &trace] {
        let _ = std::fs::remove_file(f);
    }
    out.map_err(|e| {
        anyhow::anyhow!(
            "{e} ({:.1} s); {}",
            start.elapsed().as_secs_f32(),
            shim_summary(&shim)
        )
    })
}

/// The shim's trace on one line, without its timestamps.
fn shim_summary(trace: &str) -> String {
    let steps: Vec<&str> = trace
        .lines()
        .filter_map(|l| l.split_once(' ').map(|(_, step)| step.trim()))
        .filter(|s| !s.is_empty())
        .collect();
    if steps.is_empty() {
        if crate::win::prefers_system32() {
            return "the shim didn't load in the client: rekindle was started by a program that makes Windows load DLLs from System32 first \
                    (browsers do), and the client inherits that, so it loads System32's dwmapi.dll. Run the setup from Explorer."
                .into();
        }
        return "the shim didn't load in the client".into();
    }
    format!("shim: {}", steps.join("; "))
}

/// An exit status as the launcher log shows it. Windows' NTSTATUS crash codes
/// read better in hex (`0xc0000005`).
fn exit_code(status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(c) if c as u32 > 0xffff => format!("exit code {:#x}", c as u32),
        Some(c) => format!("exit code {c}"),
        None => "no exit code".into(),
    }
}

/// Start the client for the user. The shim is already beside it, so this is a
/// plain launch: the client's loader maps rekindle on its own.
pub fn open(client: &Path) -> Result<()> {
    std::process::Command::new(client)
        .spawn()
        .with_context(|| format!("starting {}", client.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_without_a_report_says_how_far_the_shim_got() {
        assert_eq!(shim_summary(""), "the shim didn't load in the client");
        let trace = "1794864299111 ctor: driving=true\n1794864299113 gate: waiting for host\n1794864300879 host: loaded\n";
        assert_eq!(
            shim_summary(trace),
            "shim: ctor: driving=true; gate: waiting for host; host: loaded"
        );
    }

    #[test]
    fn placing_copies_the_shim_and_clears_an_older_layout() {
        let dir =
            std::env::temp_dir().join(format!("rekindle-launch-place-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (version, client_dir) = (dir.join("versions").join("1.0.0"), dir.join("client"));
        std::fs::create_dir_all(&version).unwrap();
        std::fs::create_dir_all(&client_dir).unwrap();
        for name in version_files() {
            std::fs::write(version.join(name), name).unwrap();
            // What a launcher before the version folders placed.
            std::fs::write(client_dir.join(name), "old").unwrap();
        }
        std::fs::write(client_dir.join("client.exe"), "MZ").unwrap();
        place(&client_dir.join("client.exe"), &version).unwrap();
        let mut left: Vec<_> = std::fs::read_dir(&client_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["client.exe", SHIM_DLL]);
        assert_eq!(
            std::fs::read_to_string(client_dir.join(SHIM_DLL)).unwrap(),
            SHIM_DLL
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_files_are_the_shim_host_and_data() {
        let files = version_files();
        assert!(files.contains(&SHIM_DLL) && files.contains(&HOST_DLL));
        assert!(DATA.iter().all(|d| files.contains(d)));
    }
}
