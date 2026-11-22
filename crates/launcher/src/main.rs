//! `rekindle.exe`: what the user double-clicks. See the library docs.
//!
//! rekindle loads through the side-load shim beside the client, so this
//! program is an installer and repair tool, not a launcher:
//!
//! - no arguments, run from a download: the setup window (install, or
//!   update, repair, open and uninstall when it's installed already).
//! - no arguments, installed: open the client (the shim is already beside
//!   it). A window shows only if the client can't be opened.
//! - `--uninstall`: what Settings › Apps runs (the uninstall window).
//! - `--check [--client <exe>] [--framework <dir>]`: place a version beside
//!   the client, run the client off the desktop through the shim, report
//!   whether rekindle comes up, and end it (no window).
//! - `--update`: check the release channel, download a newer rekindle and a
//!   newer `rekindle.exe`, and place the best version that comes up beside
//!   the client.
//! - `--version`: print `rekindle-launcher <version>`.
//! - `--pack --dir <files> --version <v> --out <setup.exe>`: the release
//!   step: append rekindle's files to this program.
//! - `--preview [installed|uninstall|plain|problem]`: open a window whose
//!   buttons only pretend, to look at it without installing anything.
//!
//! The windows are `window.rs`. Where one can't open (no OpenGL), the same
//! steps run as message boxes.

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rekindle_launcher::install::{self, Setup};
use rekindle_launcher::launch;
use rekindle_launcher::payload::{self, Bundle};
use rekindle_launcher::state::Install;
#[cfg(windows)]
use rekindle_launcher::ui::Outcome;
use rekindle_launcher::ui::{CLIENT, Job, Model};
use rekindle_launcher::win::{self, Answer, Icon};
use rekindle_launcher::{place_best, update};
use rekindle_release::signing;

/// Environment variable naming the client program (else the one recorded
/// at install, else the user picks it).
const CLIENT_ENV: &str = "REKINDLE_CLIENT";

/// The first argument of a copy started through Explorer, so it never
/// starts another.
const RELAUNCHED: &str = "--relaunched";

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let relaunched = args.first().is_some_and(|a| a == RELAUNCHED);
    if relaunched {
        args.remove(0);
    }
    // Opened from a browser, this process loads DLLs from System32 first and
    // every client it starts would too, so the shim beside the client never
    // loads. Start again through Explorer, which is how a double-click in
    // the Downloads folder runs it. If that fails, carry on: the check says
    // why rekindle didn't come up.
    let windowed = args.is_empty() || args[0] == "--uninstall";
    if windowed
        && !relaunched
        && win::prefers_system32()
        && let Ok(me) = std::env::current_exe()
    {
        let again = std::iter::once(RELAUNCHED)
            .chain(args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        if win::start_through_explorer(&me, &again, win::SW_SHOWNORMAL).is_ok() {
            return;
        }
    }
    let cli = !args.is_empty() && args[0] != "--uninstall";
    if cli {
        win::attach_console();
    }
    if let Err(e) = run(&args) {
        if cli {
            eprintln!("rekindle: {e:#}");
        } else if show(Model::problem(&e), Real::none()).is_err() {
            win::tell("rekindle", &format!("{e:#}"), Icon::Error);
        }
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> Result<()> {
    let install = Install::new(Install::default_root()?);
    match args.first().map(String::as_str) {
        None => {
            let me = std::env::current_exe().context("locating this program")?;
            if same_file(&me, &install.exe()) {
                return launch_flow(&install);
            }
            let bytes = std::fs::read(&me).with_context(|| format!("reading {}", me.display()))?;
            match payload::read(&bytes)? {
                Some((exe, bundle)) => setup_flow(&install, &exe, &bundle),
                None if install.is_installed() => launch_flow(&install),
                None => bail!(
                    "This copy of rekindle has nothing to install. Download rekindle-setup.exe again."
                ),
            }
        }
        Some("--uninstall") => uninstall_flow(&install),
        Some("--check") => check_flow(&install, &args[1..]),
        Some("--update") => update_flow(&install),
        Some("--version") => {
            println!("rekindle-launcher {}", rekindle_launcher::LAUNCHER_VERSION);
            Ok(())
        }
        Some("--pack") => pack_flow(&args[1..]),
        Some("--preview") => preview(args.get(1).map(String::as_str)),
        Some(other) => bail!("unknown option `{other}`"),
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

/// The client named by `$REKINDLE_CLIENT`, else the one recorded at
/// install, if it is still there.
fn known_client(install: &Install) -> Result<Option<PathBuf>> {
    if let Some(c) = std::env::var_os(CLIENT_ENV).filter(|c| !c.is_empty()) {
        return Ok(Some(c.into()));
    }
    Ok(install.state()?.client.filter(|c| c.is_file()))
}

/// The client: [`known_client`], else the one the user picks.
fn find_client(install: &Install) -> Result<PathBuf> {
    if let Some(c) = known_client(install)? {
        return Ok(c);
    }
    win::tell(
        "rekindle",
        &format!(
            "rekindle needs to know where {CLIENT} is. Choose its program in the next window."
        ),
        Icon::Info,
    );
    pick_client().with_context(|| format!("rekindle can't start without knowing where {CLIENT} is"))
}

/// The "open file" dialog for the client's program.
fn pick_client() -> Option<PathBuf> {
    win::pick_program(&format!("Where is {CLIENT}?"))
}

/// Show a window; `Err` if none could be made.
fn show(model: Model, backend: Real) -> std::result::Result<(), String> {
    #[cfg(windows)]
    {
        rekindle_launcher::window::run(model, backend, pick_client)
    }
    #[cfg(not(windows))]
    {
        let _ = (model, backend);
        Err("no window on this platform".into())
    }
}

/// What the window's buttons do. Only Windows has the window.
#[cfg_attr(
    not(windows),
    expect(dead_code, reason = "only the Windows window runs jobs")
)]
struct Real {
    install: Option<Install>,
    exe: Vec<u8>,
    bundle: Option<Bundle>,
}

impl Real {
    fn none() -> Self {
        Self {
            install: None,
            exe: Vec::new(),
            bundle: None,
        }
    }

    #[cfg_attr(
        not(windows),
        expect(dead_code, reason = "only the Windows window runs jobs")
    )]
    fn run_job(&mut self, job: &Job, client: Option<PathBuf>) -> Result<Option<String>> {
        let install = self.install.as_ref().context("nothing to do")?;
        match job {
            Job::Install => {
                let bundle = self
                    .bundle
                    .as_ref()
                    .context("this copy of rekindle has nothing to install")?;
                let client = match client {
                    Some(c) => c,
                    None => find_client(install)?,
                };
                install::install(&Setup {
                    install,
                    exe: &self.exe,
                    bundle,
                    client: &client,
                    uninstall_key: Some(win::UNINSTALL_KEY),
                })?;
                install.log(&format!("installed {}", bundle.version));
                place(install, &client)
            }
            Job::Launch => {
                let client = match client {
                    Some(c) => c,
                    None => find_client(install)?,
                };
                launch::open(&client)?;
                Ok(None)
            }
            Job::Uninstall { delete_plugins } => {
                uninstall(install, *delete_plugins)?;
                Ok(None)
            }
        }
    }
}

#[cfg(windows)]
impl rekindle_launcher::window::Backend for Real {
    fn run(&mut self, job: &Job, client: Option<PathBuf>) -> Outcome {
        match self.run_job(job, client) {
            Ok(None) => Outcome::Done,
            Ok(Some(why)) => Outcome::Plain(why),
            Err(e) => Outcome::Failed(e),
        }
    }
}

/// Place the best version that comes up beside the client, with a check and a
/// fallback. `Ok(None)` if one came up; `Ok(Some(why))` if none did (the shim
/// was removed, so the client runs stock).
fn place(install: &Install, client: &Path) -> Result<Option<String>> {
    let mut state = install.state()?;
    state.client = Some(client.to_path_buf());
    let placed = place_best(install, &mut state, client);
    install.save(&state)?;
    install.gc(&state);
    match placed.placed {
        Some(_) => Ok(None),
        None => Ok(Some(placed.failures.last().map_or_else(
            || "no version of rekindle is installed".into(),
            |f| f.why.clone(),
        ))),
    }
}

fn setup_flow(install: &Install, exe: &[u8], bundle: &Bundle) -> Result<()> {
    let installed = install.state()?.current;
    let model = Model::setup(&bundle.version, installed, known_client(install)?);
    let backend = Real {
        install: Some(install.clone()),
        exe: exe.to_vec(),
        bundle: Some(bundle.clone()),
    };
    if show(model, backend).is_ok() {
        return Ok(());
    }
    setup_dialogs(install, exe, bundle)
}

/// The setup as message boxes, where no window opens.
fn setup_dialogs(install: &Install, exe: &[u8], bundle: &Bundle) -> Result<()> {
    let root = install.root().display().to_string();
    let question = if install.is_installed() {
        format!("Update rekindle to {}?", bundle.version)
    } else {
        format!(
            "Install rekindle {}?\n\n\
             rekindle adds plugins to {CLIENT}. It installs for you only, in {root}, and places \
             itself next to {CLIENT} so it loads whenever you open {CLIENT}.\n\n\
             Nothing else starts by itself: open {CLIENT} the way you always do. \
             You can remove rekindle in Settings › Apps.",
            bundle.version
        )
    };
    if win::ask("rekindle", &question, false) != Answer::Yes {
        return Ok(());
    }
    let client = find_client(install)?;
    install::install(&Setup {
        install,
        exe,
        bundle,
        client: &client,
        uninstall_key: Some(win::UNINSTALL_KEY),
    })?;
    install.log(&format!("installed {}", bundle.version));
    match place(install, &client)? {
        None => {
            let open = win::ask(
                "rekindle",
                &format!(
                    "rekindle is installed.\n\nOpen {CLIENT} now? From now on, open {CLIENT} the way you always do and rekindle loads with it."
                ),
                false,
            );
            if open == Answer::Yes {
                launch::open(&client)?;
            }
        }
        Some(why) => {
            win::tell(
                "rekindle",
                &format!(
                    "rekindle is installed, but it couldn't come up with your {CLIENT} this time, so {CLIENT} will open without plugins.\n\n{why}"
                ),
                Icon::Warning,
            );
        }
    }
    Ok(())
}

/// Installed, run bare (there is no shortcut): open the client. The shim is
/// already beside it. Then check for a newer rekindle, which the next launch
/// of the client loads.
fn launch_flow(install: &Install) -> Result<()> {
    let client = find_client(install)?;
    launch::open(&client).with_context(|| format!("opening {CLIENT}"))?;
    if let Err(e) = check_for_update(install) {
        install.log(&format!("{e:#}"));
    }
    Ok(())
}

/// Check the release channel: replace this program with a newer one,
/// download a newer framework, and save the state.
fn check_for_update(install: &Install) -> Result<Option<update::Ran>> {
    let Some(location) = rekindle_release::channel_location(None).context("release channel")?
    else {
        return Ok(None);
    };
    update::run(install, &location, &signing::first_party_keys(), now()).map(Some)
}

fn update_flow(install: &Install) -> Result<()> {
    if !install.is_installed() {
        bail!("rekindle isn't installed in {}", install.root().display());
    }
    let ran = check_for_update(install)?;
    if let Some(v) = ran.as_ref().and_then(|r| r.launcher.as_deref()) {
        println!("rekindle-launcher: rekindle.exe updated to {v}");
    }
    match ran.map(|r| r.framework) {
        Some(update::Checked::Downloaded(v)) => {
            println!("rekindle-launcher: downloaded {v}; placing it");
        }
        Some(update::Checked::UpToDate) => println!(
            "rekindle-launcher: up to date (channel serial {})",
            install.state()?.channel_serial
        ),
        None => println!("rekindle-launcher: no release channel is configured"),
    }
    // Place the best version (the one just downloaded, if it comes up) beside
    // the client.
    let client = find_client(install)?;
    match place(install, &client)? {
        None => println!("rekindle-launcher: placed beside {}", client.display()),
        Some(why) => println!("rekindle-launcher: no version came up ({why}); {CLIENT} runs stock"),
    }
    Ok(())
}

/// Uninstall, and delete the user's plugins too if asked.
fn uninstall(install: &Install, delete_plugins: bool) -> Result<()> {
    let home = rekindle_release::plugin_home()?;
    install::uninstall(install, Some(win::UNINSTALL_KEY))?;
    if delete_plugins && home.is_dir() {
        std::fs::remove_dir_all(&home).with_context(|| format!("removing {}", home.display()))?;
    }
    Ok(())
}

fn uninstall_flow(install: &Install) -> Result<()> {
    let backend = Real {
        install: Some(install.clone()),
        ..Real::none()
    };
    if show(Model::uninstall(), backend).is_ok() {
        return Ok(());
    }
    uninstall_dialogs(install)
}

/// Uninstalling as message boxes, where no window opens.
fn uninstall_dialogs(install: &Install) -> Result<()> {
    let home = rekindle_release::plugin_home()?;
    let answer = win::ask(
        "Uninstall rekindle",
        &format!(
            "This removes rekindle from next to {CLIENT}. {CLIENT} itself stays.\n\n\
             Also remove your plugins and their settings ({})?\n\n\
             Yes: remove everything.\nNo: keep my plugins, so reinstalling brings them back.\nCancel: don't uninstall.",
            home.display()
        ),
        true,
    );
    if answer == Answer::Cancel {
        return Ok(());
    }
    uninstall(install, answer == Answer::Yes)?;
    win::tell("rekindle", "rekindle was uninstalled.", Icon::Info);
    Ok(())
}

fn check_flow(install: &Install, args: &[String]) -> Result<()> {
    let client = match flag(args, "--client") {
        Some(c) => PathBuf::from(c),
        None => install
            .state()?
            .client
            .context("no client recorded; pass --client <exe>")?,
    };
    if let Some(dir) = flag(args, "--framework") {
        launch::place(&client, Path::new(dir))
            .with_context(|| format!("placing {dir} beside the client"))?;
        return match launch::check(&client, Path::new(dir)) {
            launch::Verdict::Up(report) => {
                println!("{report}");
                println!("rekindle-launcher: ok, rekindle comes up from {dir}");
                Ok(())
            }
            launch::Verdict::Refused(why) | launch::Verdict::NoReport(why) => bail!("{why}"),
        };
    }
    let mut state = install.state()?;
    let placed = place_best(install, &mut state, &client);
    install.save(&state)?;
    for f in &placed.failures {
        println!(
            "{} didn't come up{}: {}",
            f.version,
            if f.marked_bad { " (marked bad)" } else { "" },
            f.why
        );
    }
    match placed.placed {
        Some((v, report)) => {
            println!("{report}");
            println!("rekindle-launcher: ok, version {v} comes up");
            Ok(())
        }
        None => bail!("no installed version of rekindle comes up"),
    }
}

/// The windows with buttons that wait a moment and pretend to succeed.
fn preview(page: Option<&str>) -> Result<()> {
    struct Pretend;
    #[cfg(windows)]
    impl rekindle_launcher::window::Backend for Pretend {
        fn run(&mut self, _job: &Job, _client: Option<PathBuf>) -> Outcome {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            Outcome::Done
        }
    }
    let model = match page {
        None => Model::setup("0.1.0", None, None),
        Some("installed") => {
            Model::setup("0.2.0", Some("0.1.0".into()), Some("Discord.exe".into()))
        }
        Some("uninstall") => Model::uninstall(),
        Some("problem") => {
            Model::problem(&anyhow::anyhow!("access denied").context("writing state.json"))
        }
        Some(other) => bail!("unknown page `{other}` (installed, uninstall or problem)"),
    };
    #[cfg(windows)]
    return rekindle_launcher::window::run(model, Pretend, pick_client).map_err(anyhow::Error::msg);
    #[cfg(not(windows))]
    {
        let _ = (model, Pretend);
        bail!("no window on this platform")
    }
}

fn pack_flow(args: &[String]) -> Result<()> {
    let dir = PathBuf::from(flag(args, "--dir").context("--pack needs --dir <rekindle's files>")?);
    let version = flag(args, "--version")
        .context("--pack needs --version <semver>")?
        .to_string();
    let out = PathBuf::from(flag(args, "--out").context("--pack needs --out <setup.exe>")?);
    let me = std::env::current_exe()
        .and_then(std::fs::read)
        .context("reading this program")?;
    let exe = match payload::read(&me)? {
        Some((exe, _)) => exe,
        None => me,
    };
    let mut files = Vec::new();
    for e in std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
        let e = e?;
        if e.file_type()?.is_file() {
            files.push((
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path())?,
            ));
        }
    }
    files.sort();
    for needed in [launch::SHIM_DLL, launch::HOST_DLL] {
        if !files.iter().any(|(n, _)| n == needed) {
            bail!("{} has no {}", dir.display(), needed);
        }
    }
    let bundle = Bundle { version, files };
    std::fs::write(&out, payload::pack(&exe, &bundle)?)
        .with_context(|| format!("writing {}", out.display()))?;
    println!(
        "rekindle-launcher: wrote {} (rekindle {}, {} file(s))",
        out.display(),
        bundle.version,
        bundle.files.len()
    );
    Ok(())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
