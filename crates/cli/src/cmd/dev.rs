//! `rekindle dev`: run a plugin crate against the simulated client, with a live
//! browser preview and hot reload. `rekindle test`: the same client, headless,
//! while the project's own tests drive it over the preview's API.
//!
//! The serving core ([`Session`] + [`serve`]) is independent of where plugins
//! come from: `dev` adds one plugin with a rebuild source, and `run` adds each
//! installed plugin with its own trust and no source.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use rekindle_plugin_host::builder::{self, BuildOptions};
use rekindle_plugin_host::capability::{Capability, Trust};
use rekindle_plugin_host::home::InstallState;
use rekindle_plugin_host::runtime::StorageLocation;

mod api;
mod server;
mod session;
mod term;
mod watch;

use server::{ServeOptions, serve, serve_for, serve_until_closed};
use session::{DEV_TRUST, Session, compile};

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Run a plugin against the simulated client with a browser preview; rebuilds and hot-reloads on change.
    Dev {
        /// Plugin project directory (Rust or JS/TS).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Port for the preview server (loopback only).
        #[arg(long, default_value_t = 7777)]
        port: u16,
        /// Rust: build with the debug profile.
        #[arg(long)]
        debug: bool,
        /// Start with empty plugin storage (it otherwise persists across runs, in target/rekindle/dev-storage).
        #[arg(long)]
        fresh: bool,
        /// Open the preview in your browser.
        #[arg(long)]
        open: bool,
    },
    /// Build a plugin, run it in the simulated client, and run the project's tests against it.
    Test {
        /// Plugin project directory (Rust or JS/TS).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Rust: build with the debug profile.
        #[arg(long)]
        debug: bool,
        /// Passed to the test runner (`cargo test` or `node --test`), after `--`.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Build a plugin, run it in the simulated client, print `READY <port> <plugin id>` and serve the
    /// preview's API until stdin closes. `rekindle-test` starts this when the tests weren't run by
    /// `rekindle test`.
    #[command(hide = true)]
    TestServer {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long)]
        debug: bool,
    },
    /// Run all installed plugins that are on in the simulated client preview.
    Run {
        /// Port for the preview server (loopback only).
        #[arg(long, default_value_t = 7777)]
        port: u16,
        /// Open the preview in your browser.
        #[arg(long)]
        open: bool,
    },
}

pub(crate) fn run(c: Commands) -> Result<()> {
    match c {
        Commands::Dev {
            path,
            port,
            debug,
            fresh,
            open,
        } => dev(path, port, debug, fresh, open),
        Commands::Test { path, debug, args } => test(path, debug, &args),
        Commands::TestServer { path, debug } => test_server(path, debug),
        Commands::Run { port, open } => run_installed(port, open),
    }
}

fn test(path: PathBuf, debug: bool, args: &[String]) -> Result<()> {
    let project = builder::read_project(&path)
        .with_context(|| format!("reading the plugin project at {}", path.display()))?;
    let m = &project.manifest;
    let mut runner = test_runner(&project, args)?;
    let mut session = test_session(&project, path, debug)?;

    let status = serve_for(&mut session, |port| {
        runner
            .env("REKINDLE_TEST_URL", format!("http://127.0.0.1:{port}"))
            .env("REKINDLE_TEST_PLUGIN", &m.id);
        runner.spawn().context("starting the test runner")
    })?;
    if status.success() {
        return Ok(());
    }
    let logs = session.recent_logs(40);
    if !logs.is_empty() {
        term::line("");
        term::warn("the plugin's last log lines:");
        for r in &logs {
            term::log(r);
        }
    }
    std::process::exit(status.code().unwrap_or(1));
}

fn test_server(path: PathBuf, debug: bool) -> Result<()> {
    let project = builder::read_project(&path)
        .with_context(|| format!("reading the plugin project at {}", path.display()))?;
    let mut session = test_session(&project, path, debug)?;
    serve_until_closed(&mut session, &project.manifest.id)
}

/// Build the plugin and load it into a fresh simulated client with
/// in-memory storage, for its tests.
fn test_session(project: &builder::Project, path: PathBuf, debug: bool) -> Result<Session> {
    let m = &project.manifest;
    term::note(&format!("building {}…", m.id));
    let built = compile(&BuildOptions {
        path,
        release: !debug,
        quiet: true,
        typecheck: true,
    })
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Some(w) = &built.warnings {
        term::warn("build warnings:");
        term::block(w);
    }
    let mut session = Session::new(
        format!("{} (rekindle test)", m.name),
        StorageLocation::Memory,
    )?;
    session.set_echo_logs(false);
    session.set_source_map(&m.id, built.source_map.clone());
    if let Err(e) = session.add(&m.id, DEV_TRUST, Ok(built.bytes), None) {
        bail!("the plugin didn't load, so there's nothing to test: {e}");
    }
    Ok(session)
}

/// The project's test runner: `cargo test`, or Node's (for
/// `tests/**/*.test.*`), one test at a time, because the tests share one
/// simulated client.
fn test_runner(project: &builder::Project, args: &[String]) -> Result<Command> {
    let dir = &project.dir;
    let mut cmd = match project.language {
        builder::Language::Rust => {
            let mut c = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
            c.arg("test").env("RUST_TEST_THREADS", "1");
            c
        }
        builder::Language::TypeScript => {
            let mut files = Vec::new();
            collect_tests(&dir.join("tests"), &mut files);
            files.sort();
            if files.is_empty() {
                bail!(
                    "no tests in {}: name them `*.test.ts` or `*.test.js`",
                    dir.join("tests").display()
                );
            }
            let mut c = Command::new("node");
            c.args(["--test", "--test-concurrency=1"]);
            if files
                .iter()
                .any(|f| f.extension().is_some_and(|e| e == "ts" || e == "mts"))
            {
                c.args(strip_types_flags()?);
            }
            // Node reads its options before the files.
            c.args(args)
                .args(files.iter().map(|f| f.strip_prefix(dir).unwrap_or(f)));
            c.current_dir(dir);
            return Ok(c);
        }
    };
    cmd.args(args).current_dir(dir);
    Ok(cmd)
}

fn collect_tests(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() && name != "node_modules" {
            collect_tests(&p, out);
        } else if [".test.ts", ".test.mts", ".test.js", ".test.mjs"]
            .iter()
            .any(|s| name.ends_with(s))
        {
            out.push(p);
        }
    }
}

/// The flags this Node needs to run TypeScript tests: none from 22.18 and
/// 23.6, where type stripping is on by default; the experimental flag from
/// 22.6.
fn strip_types_flags() -> Result<Vec<&'static str>> {
    let out = Command::new("node")
        .arg("--version")
        .output()
        .context("running node (is Node.js installed?)")?;
    let v = String::from_utf8_lossy(&out.stdout);
    let mut parts = v
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(|p| p.parse::<u32>().unwrap_or(0));
    let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    Ok(match (major, minor) {
        (m, _) if m >= 24 => vec![],
        (23, n) if n >= 6 => vec![],
        (22, n) if n >= 18 => vec![],
        (23, _) | (22, 6..) => vec![
            "--experimental-strip-types",
            "--disable-warning=ExperimentalWarning",
        ],
        _ => bail!(
            "TypeScript tests need Node.js 22.6 or later, and this is {}; update Node, or write the tests as `.test.js`",
            v.trim()
        ),
    })
}

fn dev(path: PathBuf, port: u16, debug: bool, fresh: bool, open: bool) -> Result<()> {
    let project = builder::read_project(&path)
        .with_context(|| format!("reading the plugin project at {}", path.display()))?;
    let rust = project.language == builder::Language::Rust;
    let (m, target_dir) = (project.manifest, project.target_dir);

    term::heading(&format!(
        "rekindle dev: {} {} ({})",
        m.name, m.version, m.id
    ));
    if !m.description.is_empty() {
        term::dim(&format!("  {}", m.description));
    }
    if m.capabilities.is_empty() {
        term::line("  requests: (no capabilities)");
    } else {
        term::line("  requests:");
        for cap in &m.capabilities {
            term::line(&format!("    - {cap}: {}", cap.describe()));
        }
    }
    if m.requests(Capability::Network) {
        for h in &m.network {
            term::line(&format!(
                "    network host: {h}{}",
                super::author::host_note(h)
            ));
        }
    }
    term::dim(
        "  it holds everything its manifest requests; network stays limited to the hosts listed there",
    );

    let storage = target_dir.join("rekindle").join("dev-storage");
    if fresh && storage.exists() {
        std::fs::remove_dir_all(&storage)
            .with_context(|| format!("clearing {}", storage.display()))?;
    }
    term::dim(&format!(
        "  storage: {}{}",
        storage.display(),
        if fresh {
            " (cleared)"
        } else {
            " (kept between runs; --fresh clears it)"
        }
    ));
    let mut session = Session::new(
        format!("{} (rekindle dev)", m.name),
        StorageLocation::Dir(storage),
    )?;

    let opts = BuildOptions {
        path,
        release: !debug,
        quiet: true,
        typecheck: true,
    };
    let profile = if !rust {
        String::new()
    } else if debug {
        " (debug)".into()
    } else {
        " (release)".into()
    };
    term::note(&format!("building {}{profile}…", m.id));
    let first = compile(&opts).map(|b| {
        if let Some(w) = &b.warnings {
            term::warn("build warnings:");
            term::block(w);
        }
        session.set_source_map(&m.id, b.source_map);
        b.bytes
    });
    if let Err(e) = &first {
        term::error("initial build failed. Fix it and save; the preview will pick it up.");
        term::block(e);
    }
    // Failures are already reported (terminal + preview); keep serving.
    let _ = session.add(&m.id, DEV_TRUST, first, Some(opts));

    serve(
        session,
        &ServeOptions {
            port,
            open_browser: open,
        },
    )
}

fn run_installed(port: u16, open: bool) -> Result<()> {
    let home = rekindle_plugin_host::home::Home::from_env()?;
    term::heading(&format!(
        "rekindle run: installed plugins in {}",
        home.root().display()
    ));
    let installed = home.installed()?;
    if installed.is_empty() {
        term::warn("no plugins installed; use `rekindle install <plugin.wasm>`");
    }
    let third_party = installed
        .iter()
        .filter(|i| {
            i.is_ready()
                && i.trust
                    .as_ref()
                    .is_some_and(|t| t.trust == Trust::ThirdParty)
        })
        .count();
    if third_party > 0 {
        term::warn(&format!(
            "developer mode is on: {third_party} plugin(s) from outside rekindle will run"
        ));
    }
    let mut session = Session::new(
        "rekindle: installed plugins",
        StorageLocation::Dir(home.storage_dir()),
    )?;
    for inst in &installed {
        let name = inst
            .manifest
            .as_ref()
            .map_or_else(|| inst.id.clone(), |m| format!("{} {}", m.name, m.version));
        match (&inst.state, &inst.trust) {
            (InstallState::Ready, Some(trust)) => {
                let verified = inst.read_verified().map_err(|e| format!("{e:#}"));
                let added = match verified {
                    Ok((bytes, resolved)) => session.add(&inst.id, resolved, Ok(bytes), None),
                    Err(e) => session.add(&inst.id, trust.trust, Err(e), None),
                };
                match added {
                    Ok(()) => term::dim(&format!("    {}", trust.describe())),
                    Err(e) => term::error(&format!("  {name} ({}) failed to load: {e}", inst.id)),
                }
            }
            (InstallState::Refused(_), _) => {
                term::error(&format!("  {name}: {}", inst.state.describe()));
            }
            (InstallState::NeedsReview(_), _) => {
                term::warn(&format!("  {name}: {}", inst.state.describe()));
            }
            (state, _) => term::dim(&format!("  {name}: {}", state.describe())),
        }
    }
    serve(
        session,
        &ServeOptions {
            port,
            open_browser: open,
        },
    )
}
