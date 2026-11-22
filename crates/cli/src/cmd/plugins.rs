//! Installing and managing plugins: `install`, `enable`, `disable`,
//! `developer-mode`, `list`, `uninstall`, `update`. And the project's
//! signing and release tools: `keygen`, `sign`, `verify`, `channel`.
//!
//! These are for plugin authors and the project. Users never need them: the
//! client does the same through its settings, and checks for updates itself.
//!
//! Everything that touches the user's rekindle home uses `$REKINDLE_HOME`, else
//! `~/.rekindle`.

use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand, ValueEnum};
use rekindle_plugin_host::capability::{Trust, resolve_grants};
use rekindle_plugin_host::channel::{self, Blocked, Location};
use rekindle_plugin_host::home::{DEVELOPER_MODE_WARNING, Home, InstallState};
use rekindle_plugin_host::manifest;
use rekindle_plugin_host::review;
use rekindle_plugin_host::runtime;
use rekindle_plugin_host::signing::{self, Keypair, PublicKey, Signature};
use rekindle_plugin_host::theme;
use rekindle_plugin_host::updates;

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Install a plugin or a theme. A rekindle plugin installs turned off
    /// (turn it on with `enable`); a third-party one needs developer mode and
    /// shows what it can do first.
    Install {
        /// The plugin's `.wasm` file, or a `.theme.json`.
        wasm: PathBuf,
        /// Signature file (default: `<WASM>.sig` if it exists).
        #[arg(long)]
        sig: Option<PathBuf>,
        /// Turn it on right away.
        #[arg(long)]
        enable: bool,
        /// Don't ask before installing a third-party plugin (scripts).
        #[arg(long, short)]
        yes: bool,
    },
    /// Turn an installed plugin on.
    Enable { id: String },
    /// Turn an installed plugin off (it stays installed).
    Disable { id: String },
    /// Allow plugins that aren't part of rekindle. Off by default.
    DeveloperMode {
        /// `on` or `off`; omit to show the current state.
        state: Option<OnOff>,
        /// Don't ask before turning it on (scripts).
        #[arg(long, short)]
        yes: bool,
    },
    /// List installed plugins and whether they will load.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Remove an installed plugin.
    Uninstall {
        id: String,
        /// Also delete the plugin's saved settings.
        #[arg(long)]
        purge: bool,
    },
    /// Generate an Ed25519 signing key: writes `<OUT>.secret.json` (keep it
    /// secret) and `<OUT>.public.json`.
    Keygen {
        /// Path stem for the two key files, e.g. `keys/alice`.
        #[arg(long)]
        out: PathBuf,
        /// Display name for the key.
        #[arg(long)]
        name: Option<String>,
    },
    /// Sign a built plugin; writes a detached `<WASM>.sig`. Signed with a
    /// rekindle project key, the plugin is first-party.
    Sign {
        wasm: PathBuf,
        /// Secret key file from `rekindle keygen`.
        #[arg(long)]
        key: PathBuf,
        /// Where to write the signature (default: `<WASM>.sig`).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Check a plugin's signature and show whether it is first-party.
    Verify {
        wasm: PathBuf,
        /// Signature file (default: `<WASM>.sig` if it exists).
        #[arg(long)]
        sig: Option<PathBuf>,
        /// Verify against this public key file instead of the project keys.
        #[arg(long)]
        key: Option<PathBuf>,
    },
    /// Check the release channel and install first-party plugin updates
    /// (a running client does this itself, every few hours).
    Update {
        /// Only list what would be updated.
        #[arg(long)]
        check: bool,
        /// Channel to check (path or https URL; default: `$REKINDLE_CHANNEL`,
        /// then `channel` in config.json, then the official one).
        #[arg(long)]
        channel: Option<String>,
    },
    /// Build and check the signed release channel.
    Channel {
        #[command(subcommand)]
        cmd: ChannelCmd,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum OnOff {
    On,
    Off,
}

#[derive(Subcommand)]
pub(crate) enum ChannelCmd {
    /// Build and sign `channel.json` from a directory of project-signed
    /// `.wasm` files (each with its `.sig`), to host next to them.
    // Boxed: its arguments are several times the size of `Verify`'s.
    Build(Box<ChannelBuild>),
    /// Check a channel's signature (path or https URL) and list it.
    Verify {
        channel: String,
        /// Verify against this public key file instead of the project keys.
        #[arg(long)]
        key: Option<PathBuf>,
    },
}

#[derive(Args)]
pub(crate) struct ChannelBuild {
    dir: PathBuf,
    /// Project secret key (signs the channel).
    #[arg(long)]
    key: PathBuf,
    /// Output path (default: `<DIR>/channel.json`; the signature goes next to it).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Display name.
    #[arg(long)]
    name: Option<String>,
    /// Serial (default: the current unix time). Must exceed the last
    /// published one.
    #[arg(long)]
    serial: Option<u64>,
    /// Days until the channel expires and offers no more updates.
    #[arg(long, default_value_t = 90)]
    valid_days: u64,
    /// Block a plugin file: `<sha256>=<reason>` (repeatable).
    #[arg(long)]
    block: Vec<String>,
    /// rekindle's own files for the launcher: a flat directory (the host
    /// library and its recovered data), copied to
    /// `<DIR>/framework/<VERSION>/` and listed. Needs `--framework-version`.
    #[arg(long, requires = "framework_version")]
    framework: Option<PathBuf>,
    #[arg(long)]
    framework_version: Option<String>,
    /// The launcher (`rekindle.exe`) for installs to replace themselves
    /// with, copied to `<DIR>/launcher/<VERSION>/rekindle.exe` and
    /// listed. Needs `--launcher-version`, the version it prints for
    /// `--version`.
    #[arg(long, requires = "launcher_version")]
    launcher: Option<PathBuf>,
    #[arg(long)]
    launcher_version: Option<String>,
    /// Community plugins to add to the index: a directory of `.wasm` and
    /// `.theme.json` files, each with a `<file>.listing.json`
    /// (`rekindle index build` writes them).
    #[arg(long)]
    community: Option<PathBuf>,
    /// The channel published before this one (path or https URL). Its
    /// community entries and blocklist are kept, because the index never
    /// drops a version and a block isn't lifted by leaving it out. Its
    /// framework and launcher are kept too, unless `--framework` or
    /// `--launcher` gives a new one. A
    /// plugin version it already published keeps the published file.
    #[arg(long)]
    previous: Option<String>,
}

pub(crate) fn run(c: Commands) -> Result<()> {
    match c {
        Commands::Install {
            wasm,
            sig,
            enable,
            yes,
        } => install(&wasm, sig, enable, yes),
        Commands::Enable { id } => set_enabled(&id, true),
        Commands::Disable { id } => set_enabled(&id, false),
        Commands::DeveloperMode { state, yes } => developer_mode(state, yes),
        Commands::List { json } => list(json),
        Commands::Uninstall { id, purge } => {
            let home = Home::from_env()?;
            if home.uninstall(&id, purge)? {
                println!(
                    "uninstalled {id}{}",
                    if purge { " (settings deleted)" } else { "" }
                );
                Ok(())
            } else {
                bail!("{id} is not installed")
            }
        }
        Commands::Keygen { out, name } => keygen(&out, name),
        Commands::Sign { wasm, key, out } => sign(&wasm, &key, out),
        Commands::Verify { wasm, sig, key } => verify(&wasm, sig, key),
        Commands::Update { check, channel } => update(check, channel),
        Commands::Channel {
            cmd: ChannelCmd::Build(b),
        } => channel_build(*b),
        Commands::Channel {
            cmd: ChannelCmd::Verify { channel, key },
        } => channel_verify(&channel, key),
    }
}

/// Ask on stdin. `expected` is what the user must type; `None` is a y/N.
fn ask(question: &str, expected: Option<&str>) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("stdin is not a terminal; pass --yes to answer non-interactively");
    }
    let stdin = std::io::stdin();
    let hint = match expected {
        Some(e) => format!("type `{e}` to continue: "),
        None => "[y/N] ".into(),
    };
    print!("{question} {hint}");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    if stdin.lock().read_line(&mut answer)? == 0 {
        bail!("aborted (end of input)");
    }
    let answer = answer.trim();
    Ok(match expected {
        Some(e) => answer.eq_ignore_ascii_case(e),
        None => matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes"),
    })
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

/// The explicit signature, else `<wasm>.sig` if it exists.
fn load_sig(wasm: &Path, explicit: Option<PathBuf>) -> Result<Option<Signature>> {
    if let Some(p) = explicit {
        Ok(Some(Signature::load(&p)?))
    } else {
        let p = signing::sig_path_for(wasm);
        p.exists().then(|| Signature::load(&p)).transpose()
    }
}

fn install(path: &Path, sig: Option<PathBuf>, enable: bool, yes: bool) -> Result<()> {
    let bytes = read(path)?;
    let sig = load_sig(path, sig)?;
    install_bytes(&bytes, sig.as_ref(), enable, yes)
}

/// Install a plugin's bytes, showing the review screen first for one that
/// isn't first-party.
pub(crate) fn install_bytes(
    bytes: &[u8],
    sig: Option<&Signature>,
    enable: bool,
    yes: bool,
) -> Result<()> {
    let home = Home::from_env()?;
    let resolution = home.trust_of(bytes, sig)?;
    // Compiling to inspect it also leaves it in the home's compile cache,
    // so the client doesn't compile it at its next launch.
    let engine = runtime::engine()?;
    let cache = runtime::compiled::CompileCache::new(home.compile_cache_dir());
    let insp = runtime::inspect_artifact_cached(&engine, Some(&cache), bytes)?;
    let m = &insp.manifest;
    if theme::is_theme(bytes) {
        for w in theme::parse(bytes)?.warnings {
            eprintln!("warning: {w}");
        }
    }

    if resolution.trust != Trust::FirstParty {
        if resolution.trust == Trust::ThirdParty && !home.developer_mode()? {
            bail!(
                "{} is not part of rekindle ({}).\nInstalling plugins from outside rekindle needs developer mode: `rekindle developer-mode on`",
                m.id,
                resolution.describe()
            );
        }
        let previous = home.record(&m.id)?;
        let screen = review::screen(m, &resolution, previous.as_ref());
        print!("{}", review::render(&screen));
        println!();
        if !yes && !ask("Install it?", None)? {
            bail!("not installed");
        }
    }
    let (dest, record) = home.install(bytes, sig)?;
    if enable && !record.enabled {
        home.set_enabled(&m.id, true)?;
    }
    let on = enable || record.enabled;
    println!(
        "installed {} {} ({}) -> {}",
        m.id,
        m.version,
        resolution.trust.as_str(),
        dest.display()
    );
    if on {
        println!("  it is on");
    } else {
        println!("  it is off; turn it on with `rekindle enable {}`", m.id);
    }
    Ok(())
}

fn set_enabled(id: &str, on: bool) -> Result<()> {
    let home = Home::from_env()?;
    home.set_enabled(id, on)?;
    let inst = home.get(id)?.context("not installed")?;
    match (&inst.state, on) {
        (InstallState::DeveloperModeOff, true) => {
            println!(
                "{id} is on, but it is third-party and developer mode is off, so it won't run until you turn developer mode on"
            );
        }
        (_, true) => println!("{id} is on"),
        (_, false) => println!("{id} is off"),
    }
    Ok(())
}

fn third_party_on(home: &Home) -> Result<usize> {
    Ok(home
        .installed()?
        .iter()
        .filter(|i| {
            i.trust
                .as_ref()
                .is_some_and(|t| t.trust == Trust::ThirdParty)
                && i.record.as_ref().is_some_and(|r| r.enabled)
        })
        .count())
}

fn developer_mode(state: Option<OnOff>, yes: bool) -> Result<()> {
    let home = Home::from_env()?;
    let now = home.developer_mode()?;
    match state {
        None => {
            println!("developer mode is {}", if now { "on" } else { "off" });
            if now {
                println!(
                    "  {} plugin(s) from outside rekindle are on",
                    third_party_on(&home)?
                );
            }
        }
        Some(OnOff::On) if now => println!("developer mode is already on"),
        Some(OnOff::On) => {
            println!("{DEVELOPER_MODE_WARNING}");
            println!();
            if !yes && !ask("Turn developer mode on?", Some("I understand"))? {
                bail!("developer mode is still off");
            }
            home.set_developer_mode(true)?;
            println!(
                "developer mode is on. `rekindle developer-mode off` turns every plugin from outside rekindle off at once."
            );
        }
        Some(OnOff::Off) => {
            home.set_developer_mode(false)?;
            let n = third_party_on(&home)?;
            println!("developer mode is off");
            if n > 0 {
                println!("  {n} plugin(s) from outside rekindle won't run (they stay installed)");
            }
        }
    }
    Ok(())
}

fn list(json: bool) -> Result<()> {
    let home = Home::from_env()?;
    let installed = home.installed()?;
    let developer_mode = home.developer_mode()?;
    if json {
        let rows: Vec<serde_json::Value> = installed
            .iter()
            .map(|i| {
                let grants = i
                    .manifest
                    .as_ref()
                    .map(|m| resolve_grants(&m.capabilities, &m.network));
                serde_json::json!({
                    "id": i.id,
                    "version": i.manifest.as_ref().map(|m| m.version.clone()),
                    "file": i.file,
                    "trust": i.trust,
                    "enabled": i.record.as_ref().map(|r| r.enabled),
                    "capabilities": grants.as_ref().map(|g| g.granted.clone()),
                    "hosts": grants.as_ref().map(|g| g.network_allowlist.clone()),
                    "state": i.state,
                })
            })
            .collect();
        let out = serde_json::json!({ "developer_mode": developer_mode, "plugins": rows });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!(
        "developer mode: {}",
        if developer_mode { "on" } else { "off" }
    );
    if installed.is_empty() {
        println!("no plugins installed ({})", home.plugins_dir().display());
    }
    for i in &installed {
        let version = i.manifest.as_ref().map_or("?", |m| m.version.as_str());
        let trust = i.trust.as_ref().map_or("-", |t| t.trust.as_str());
        println!("{} {version}  [{trust}]  {}", i.id, i.state.describe());
        if let Some(m) = &i.manifest {
            let g = resolve_grants(&m.capabilities, &m.network);
            let caps: Vec<&str> = g.granted.iter().map(|c| c.as_str()).collect();
            println!(
                "  can: {}",
                if caps.is_empty() {
                    "nothing beyond running".into()
                } else {
                    caps.join(", ")
                }
            );
            if !g.network_allowlist.is_empty() {
                println!(
                    "  contacts: {}",
                    g.network_allowlist
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    }
    Ok(())
}

fn keygen(out: &Path, name: Option<String>) -> Result<()> {
    let name = name.unwrap_or_else(|| {
        out.file_name().map_or_else(
            || "rekindle key".into(),
            |n| n.to_string_lossy().into_owned(),
        )
    });
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let kp = Keypair::generate(name)?;
    let (secret, public) = kp.save(out)?;
    println!("generated key {} ({})", kp.key_id(), kp.name);
    println!(
        "  secret: {}  <- SECRET: never commit or share this file",
        secret.display()
    );
    println!("  public: {}", public.display());
    Ok(())
}

fn sign(wasm: &Path, key: &Path, out: Option<PathBuf>) -> Result<()> {
    let bytes = read(wasm)?;
    let m = manifest::extract(&bytes)?
        .context("not a rekindle plugin (no manifest); build it with `rekindle build`")?;
    m.validate()?;
    let kp = Keypair::load(key)?;
    let sig = kp.sign_plugin(&bytes);
    let out = out.unwrap_or_else(|| signing::sig_path_for(wasm));
    sig.save(&out)?;
    let fp = if is_project_key(&kp) {
        " (a project key: first-party)"
    } else {
        ""
    };
    println!(
        "signed {} {} with {} ({}){fp}",
        m.id,
        m.version,
        kp.key_id(),
        kp.name
    );
    println!("  sha256 {}", sig.sha256);
    println!("  -> {}", out.display());
    Ok(())
}

fn is_project_key(kp: &Keypair) -> bool {
    signing::first_party_keys()
        .iter()
        .any(|k| k.key_id() == kp.key_id())
}

fn verify(wasm: &Path, sig: Option<PathBuf>, key: Option<PathBuf>) -> Result<()> {
    let bytes = read(wasm)?;
    let sig = load_sig(wasm, sig)?;
    if let Some(key) = key {
        let sig = sig.context("no signature found (pass --sig)")?;
        let pk = PublicKey::load(&key)?;
        pk.verify_plugin(&bytes, &sig)?;
        println!("signature OK: signed by {} ({})", pk.name, pk.key_id());
        return Ok(());
    }
    let r = Home::from_env()?.trust_of(&bytes, sig.as_ref())?;
    println!("{}", r.describe());
    println!("  sha256 {}", r.sha256);
    Ok(())
}

fn update(check_only: bool, location: Option<String>) -> Result<()> {
    let home = Home::from_env()?;
    let location = match location {
        Some(l) => Some(Location::parse(&l)?),
        None => home.channel_location()?,
    };
    if location.is_none() && !home.developer_mode()? {
        bail!("no release channel is configured yet (pass --channel)");
    }
    if check_only {
        let location = location.context("no release channel is configured yet (pass --channel)")?;
        let (bytes, sig) = channel::fetch(&location)?;
        let c = home.accept_channel(&bytes, &sig, updates::now())?;
        let pending = updates::pending(&home, &c)?;
        if pending.is_empty() {
            println!("everything is up to date (channel serial {})", c.serial);
        }
        for (inst, entry) in pending {
            let from = inst.manifest.map(|m| m.version).unwrap_or_default();
            println!("{} {from} -> {}", inst.id, entry.plugin().version);
        }
        return Ok(());
    }
    let r = updates::check_all(&home, location.as_ref(), updates::now());
    for e in &r.errors {
        println!("could not check: {e}");
    }
    for u in &r.reviews {
        println!(
            "ready for review: {} {} -> {} (not part of rekindle; review it in rekindle's settings)",
            u.id, u.from, u.to
        );
    }
    for u in &r.updated {
        println!("updated {} {} -> {}", u.id, u.from, u.to);
    }
    for u in &r.available {
        println!(
            "available: {} {} -> {} (automatic updates are off)",
            u.id, u.from, u.to
        );
    }
    for (id, why) in &r.failed {
        println!("could not update {id}: {why}");
    }
    for (id, why) in &r.blocked {
        println!("{id} is turned off by rekindle: {why}");
    }
    if r.updated.is_empty()
        && r.available.is_empty()
        && r.failed.is_empty()
        && r.reviews.is_empty()
        && r.errors.is_empty()
    {
        println!("everything is up to date (channel serial {})", r.serial);
    }
    Ok(())
}

fn print_channel(c: &channel::Channel) {
    let left = c.expires.saturating_sub(updates::now());
    println!("serial {}, expires in {} day(s)", c.serial, left / 86_400);
    println!("{} plugin(s):", c.plugins.len());
    for e in &c.plugins {
        println!(
            "  {} {} ({}), plugin API {}",
            e.id, e.version, e.name, e.wit
        );
    }
    if !c.community.is_empty() {
        println!("{} community plugin version(s):", c.community.len());
        for e in &c.community {
            println!(
                "  {} {} ({}) by {}, plugin API {}",
                e.plugin.id, e.plugin.version, e.plugin.name, e.author, e.plugin.wit
            );
        }
    }
    if let Some(f) = &c.framework {
        println!("framework {}: {} file(s)", f.version, f.files.len());
    }
    if let Some(l) = &c.launcher {
        println!("launcher {}", l.version);
    }
    for b in &c.blocked {
        println!(
            "  blocked {}{}: {}",
            b.sha256,
            b.id.as_deref()
                .map(|i| format!(" ({i})"))
                .unwrap_or_default(),
            b.reason
        );
    }
}

/// Copy the files a previous channel at `loc` lists in `f` (and their zstd
/// copies) into `dir`, checked against the listing.
fn copy_listed(loc: &Location, dir: &Path, f: &channel::Framework) -> Result<()> {
    let dest = dir.join(&f.base);
    std::fs::create_dir_all(&dest).with_context(|| format!("creating {}", dest.display()))?;
    for file in &f.files {
        let (from, to) = (
            loc.join(&format!("{}{}", f.base, file.name))?,
            dest.join(&file.name),
        );
        if !to.exists() {
            std::fs::write(&to, from.read(256 * 1024 * 1024)?)
                .with_context(|| format!("copying {from}"))?;
        }
        if signing::sha256_hex(&std::fs::read(&to)?) != file.sha256 {
            bail!(
                "{} doesn't match the previous channel's listing",
                to.display()
            );
        }
        // The listing names the zstd copy too, so it is uploaded with the rest.
        if let Some(size) = file.zst {
            let (from, to) = (
                from.with_suffix(".zst"),
                dest.join(format!("{}.zst", file.name)),
            );
            if !to.exists() {
                std::fs::write(&to, from.read(size)?).with_context(|| format!("copying {from}"))?;
            }
            if std::fs::metadata(&to)?.len() != size {
                bail!(
                    "{} doesn't match the previous channel's listing",
                    to.display()
                );
            }
        }
    }
    Ok(())
}

fn channel_build(b: ChannelBuild) -> Result<()> {
    let ChannelBuild {
        dir,
        key,
        out,
        name,
        serial,
        valid_days,
        block,
        framework,
        framework_version,
        launcher,
        launcher_version,
        community,
        previous,
    } = b;
    let kp = Keypair::load(&key)?;
    let mut plugin_keys = signing::first_party_keys();
    plugin_keys.push(kp.public());
    let serial = serial.unwrap_or_else(updates::now);
    let mut blocked = block
        .iter()
        .map(|b| parse_block(b))
        .collect::<Result<Vec<_>>>()?;
    let mut framework = match (framework, framework_version) {
        (Some(src), Some(version)) => Some(publish_framework(&dir, &src, &version)?),
        _ => None,
    };
    let mut launcher = match (launcher, launcher_version) {
        (Some(exe), Some(version)) => Some(publish_launcher(&dir, &exe, &version)?),
        _ => None,
    };
    let mut published = None;
    let mut keep_community = Vec::new();
    if let Some(loc) = previous {
        let loc = Location::parse(&loc)?;
        let (bytes, sig) = channel::fetch(&loc)?;
        let (prev, _) = channel::verify(&bytes, &sig, &plugin_keys)
            .with_context(|| format!("the previous channel {loc}"))?;
        if prev.serial >= serial {
            bail!(
                "the previous channel has serial {}, not lower than this one ({serial})",
                prev.serial
            );
        }
        copy_community(&loc, &dir, &prev.community)?;
        for b in prev.blocked {
            if !blocked.iter().any(|x| x.sha256 == b.sha256) {
                blocked.push(b);
            }
        }
        if framework.is_none()
            && let Some(f) = prev.framework
        {
            copy_listed(&loc, &dir, &f)?;
            framework = Some(f);
        }
        if launcher.is_none()
            && let Some(l) = prev.launcher
        {
            copy_listed(&loc, &dir, &l)?;
            launcher = Some(l);
        }
        published = Some((loc, prev.plugins));
        keep_community = prev.community;
    }
    let mut c = channel::build(&channel::BuildOptions {
        dir: &dir,
        key: &kp,
        name: name.unwrap_or_else(|| "rekindle".into()),
        serial,
        expires: updates::now() + valid_days * 86_400,
        plugin_keys,
        blocked,
        community: community.as_deref(),
        keep_community,
        framework,
        launcher,
    })?;
    if let Some((loc, published)) = &published {
        let kept = channel::keep_published(&dir, loc, published, &mut c.plugins)?;
        if !kept.is_empty() {
            println!(
                "note: these builds differ from the published files, which are kept: {}. To publish a change, bump the plugin's version.",
                kept.join(", ")
            );
        }
    }
    let out = out.unwrap_or_else(|| dir.join("channel.json"));
    let sig = c.write_signed(&out, &kp)?;
    print_built(&c, &kp, &out, &sig);
    Ok(())
}

/// What `channel build` wrote, and a note if the key isn't a project key.
fn print_built(c: &channel::Channel, kp: &Keypair, out: &Path, sig: &Path) {
    if !is_project_key(kp) {
        println!(
            "note: {} is not a rekindle project key, so rekindle won't take updates or a blocklist from this channel \
             (it is fine as your own plugin's `updates` channel)",
            kp.key_id()
        );
    }
    println!(
        "built channel `{}` signed by {} ({})",
        c.name,
        kp.key_id(),
        kp.name
    );
    print_channel(c);
    println!("  -> {}", out.display());
    println!("  -> {}", sig.display());
}

/// `--block <sha256>=<reason>`.
fn parse_block(arg: &str) -> Result<Blocked> {
    let (sha256, reason) = arg
        .split_once('=')
        .context("--block takes `<sha256>=<reason>`")?;
    Ok(Blocked {
        sha256: sha256.trim().to_ascii_lowercase(),
        reason: reason.trim().into(),
        id: None,
    })
}

/// Copy the files in `src` to `<dir>/framework/<version>/`, compress them,
/// and list them.
fn publish_framework(dir: &Path, src: &Path, version: &str) -> Result<channel::Framework> {
    let dest = dir.join("framework").join(version);
    std::fs::create_dir_all(&dest).with_context(|| format!("creating {}", dest.display()))?;
    for e in std::fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let e = e?;
        if e.file_type()?.is_file() {
            std::fs::copy(e.path(), dest.join(e.file_name()))
                .with_context(|| format!("copying {}", e.path().display()))?;
        }
    }
    channel::Framework::compress_dir(&dest)?;
    channel::Framework::from_dir(&dest, version)
}

/// Copy the launcher to `<dir>/launcher/<version>/`, compress it, and list it.
fn publish_launcher(dir: &Path, exe: &Path, version: &str) -> Result<channel::Framework> {
    let base = format!("launcher/{version}/");
    let dest = dir.join(&base);
    std::fs::create_dir_all(&dest).with_context(|| format!("creating {}", dest.display()))?;
    std::fs::copy(exe, dest.join(channel::LAUNCHER_FILE))
        .with_context(|| format!("copying {}", exe.display()))?;
    channel::Framework::compress_dir(&dest)?;
    channel::Framework::from_dir_at(&dest, version, &base)
}

/// Copy the community plugins a previous channel at `loc` lists into `dir`,
/// checked against the listing.
fn copy_community(
    loc: &Location,
    dir: &Path,
    community: &[channel::CommunityPlugin],
) -> Result<()> {
    for c in community {
        let (from, to) = (loc.join(&c.plugin.url)?, dir.join(&c.plugin.url));
        if !to.exists() {
            std::fs::create_dir_all(dir.join("plugins"))?;
            std::fs::write(&to, from.read(64 * 1024 * 1024)?)
                .with_context(|| format!("copying {from}"))?;
        }
        if signing::sha256_hex(&std::fs::read(&to)?) != c.plugin.sha256 {
            bail!(
                "{} doesn't match the previous channel's listing",
                to.display()
            );
        }
    }
    Ok(())
}

fn channel_verify(loc: &str, key: Option<PathBuf>) -> Result<()> {
    let (bytes, sig) = channel::fetch(&Location::parse(loc)?)?;
    let c = if let Some(k) = key {
        let pk = PublicKey::load(&k)?;
        let c = channel::verify_with(&bytes, &sig, &pk)?;
        println!("channel OK: signed by {} ({})", pk.name, pk.key_id());
        c
    } else {
        let (c, signer) = channel::verify(&bytes, &sig, &signing::first_party_keys())?;
        println!(
            "channel OK: signed by project key {} ({})",
            signer.name,
            signer.key_id()
        );
        c
    };
    print_channel(&c);
    Ok(())
}
