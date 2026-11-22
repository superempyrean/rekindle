//! Authoring commands: `new`, `build`, `sdk`, `inspect` (which also checks a theme file).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Subcommand, ValueEnum};
use rekindle_plugin_host::builder::{self, BuildOptions};
use rekindle_plugin_host::capability::Capability;
use rekindle_plugin_host::manifest::is_discord_host;
use rekindle_plugin_host::runtime;
use rekindle_plugin_host::theme;

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Scaffold a new plugin project.
    New {
        /// Project name (also the directory), e.g. `my-theme`.
        name: String,
        /// The language to write it in.
        #[arg(long, value_enum, default_value_t = Lang::Rust)]
        lang: Lang,
        /// What it starts out doing.
        #[arg(long, value_enum, default_value_t = Template::Blank)]
        template: Template,
        /// Plugin id; defaults to `dev.local.<name>`.
        #[arg(long)]
        id: Option<String>,
        /// Parent directory to create the crate in.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// Build a plugin project into a signed-ready component.
    Build {
        /// Plugin project directory (or its Cargo.toml or package.json).
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Debug profile (bigger, slower, better panics).
        #[arg(long)]
        debug: bool,
        /// Build a TypeScript plugin without type-checking it.
        #[arg(long)]
        no_check: bool,
    },
    /// Write the SDK, test client and type declarations an editor needs, without building.
    Sdk {
        /// Plugin project directory (or its package.json).
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Show a component's manifest, imports and required capabilities, or
    /// check a theme file and show what it sets.
    Inspect {
        /// A built `.wasm`, a `.theme.json`, or a plugin project directory
        /// (inspects its last build).
        wasm: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Lang {
    Rust,
    #[value(name = "ts", alias = "typescript")]
    TypeScript,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Template {
    /// Logs on activate; nothing else.
    Blank,
    /// Recolors a few theme tokens.
    Theme,
    /// A settings panel driving the theme.
    Panel,
    /// Reacts to client events.
    Events,
    /// Fetches from an allowlisted HTTPS host.
    Network,
}

impl Template {
    /// The template's `src/lib.rs` or `src/index.ts`.
    fn source(self, lang: Lang) -> &'static str {
        match (lang, self) {
            (Lang::Rust, Template::Blank) => include_str!("../../templates/blank.rs"),
            (Lang::Rust, Template::Theme) => include_str!("../../templates/theme.rs"),
            (Lang::Rust, Template::Panel) => include_str!("../../templates/panel.rs"),
            (Lang::Rust, Template::Events) => include_str!("../../templates/events.rs"),
            (Lang::Rust, Template::Network) => include_str!("../../templates/network.rs"),
            (Lang::TypeScript, Template::Blank) => include_str!("../../templates/new/ts/blank.ts"),
            (Lang::TypeScript, Template::Theme) => include_str!("../../templates/new/ts/theme.ts"),
            (Lang::TypeScript, Template::Panel) => include_str!("../../templates/new/ts/panel.ts"),
            (Lang::TypeScript, Template::Events) => {
                include_str!("../../templates/new/ts/events.ts")
            }
            (Lang::TypeScript, Template::Network) => {
                include_str!("../../templates/new/ts/network.ts")
            }
        }
    }

    /// The scaffold's tests: they check what the template does.
    fn test_source(self, lang: Lang) -> &'static str {
        match (lang, self) {
            (Lang::Rust, Template::Blank) => {
                include_str!("../../templates/new/tests/rust/blank.rs")
            }
            (Lang::Rust, Template::Theme) => {
                include_str!("../../templates/new/tests/rust/theme.rs")
            }
            (Lang::Rust, Template::Panel) => {
                include_str!("../../templates/new/tests/rust/panel.rs")
            }
            (Lang::Rust, Template::Events) => {
                include_str!("../../templates/new/tests/rust/events.rs")
            }
            (Lang::Rust, Template::Network) => {
                include_str!("../../templates/new/tests/rust/network.rs")
            }
            (Lang::TypeScript, Template::Blank) => {
                include_str!("../../templates/new/tests/ts/blank.ts")
            }
            (Lang::TypeScript, Template::Theme) => {
                include_str!("../../templates/new/tests/ts/theme.ts")
            }
            (Lang::TypeScript, Template::Panel) => {
                include_str!("../../templates/new/tests/ts/panel.ts")
            }
            (Lang::TypeScript, Template::Events) => {
                include_str!("../../templates/new/tests/ts/events.ts")
            }
            (Lang::TypeScript, Template::Network) => {
                include_str!("../../templates/new/tests/ts/network.ts")
            }
        }
    }

    fn capabilities(self) -> &'static [&'static str] {
        match self {
            Template::Blank => &[],
            Template::Theme => &["theming"],
            Template::Panel => &["theming", "ui", "storage"],
            Template::Events => &["read-client", "ui"],
            Template::Network => &["network", "ui"],
        }
    }

    fn network(self) -> &'static [&'static str] {
        match self {
            Template::Network => &["httpbin.org"],
            _ => &[],
        }
    }
}

pub(crate) fn run(c: Commands) -> Result<()> {
    match c {
        Commands::New {
            name,
            lang,
            template,
            id,
            dir,
        } => new(&name, lang, template, id, &dir),
        Commands::Build {
            path,
            debug,
            no_check,
        } => {
            let out = builder::build(&BuildOptions {
                path,
                release: !debug,
                quiet: false,
                typecheck: !no_check,
            })?;
            let size = out.bytes.len();
            println!(
                "built {} {} ({} KiB)",
                out.manifest.id,
                out.manifest.version,
                size / 1024
            );
            println!("  -> {}", out.wasm.display());
            Ok(())
        }
        Commands::Sdk { path } => {
            println!("{}", builder::write_editor_files(&path)?);
            Ok(())
        }
        Commands::Inspect { wasm, json } => inspect(&wasm, json),
    }
}

/// Print the next steps after `rekindle new`, as aligned shell lines.
fn next_steps(root: &Path, steps: &[(&str, &str)]) {
    println!();
    println!("  cd {}", root.display());
    for (cmd, what) in steps {
        println!("  {cmd:<18} # {what}");
    }
}

/// Where the SDK and host crates live, baked in at compile time so scaffolded
/// crates work from a source checkout without anything being published.
/// A downloaded `rekindle` was built somewhere else, so the path is missing.
fn repo_crate(name: &str) -> Result<String> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
    let Ok(p) = p.canonicalize() else {
        bail!(
            "Rust plugins build against the SDK in a rekindle checkout, and this rekindle wasn't built from one (or the \
             checkout moved). Install rekindle from a checkout with `cargo install --path crates/cli`, or write \
             the plugin in TypeScript (`--lang ts`)"
        )
    };
    let s = p.to_string_lossy();
    Ok(s.strip_prefix(r"\\?\").unwrap_or(&s).replace('\\', "/"))
}

/// A display name from a project name: `my-url-tool` -> `My URL Tool`.
fn title_of(name: &str) -> String {
    const UPPER: [&str; 10] = [
        "js", "ts", "ui", "api", "url", "urls", "id", "ids", "css", "html",
    ];
    name.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            if UPPER.contains(&w) {
                return w
                    .to_uppercase()
                    .replace("URLS", "URLs")
                    .replace("IDS", "IDs");
            }
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn toml_list(items: &[&str]) -> String {
    let quoted: Vec<String> = items.iter().map(|s| format!("\"{s}\"")).collect();
    format!("[{}]", quoted.join(", "))
}

fn new(name: &str, lang: Lang, template: Template, id: Option<String>, dir: &Path) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        bail!("project name must be lowercase letters, digits, `-` or `_`");
    }
    let root = dir.join(name);
    if root.exists() {
        bail!("{} already exists", root.display());
    }
    let id = id.unwrap_or_else(|| format!("dev.local.{}", name.replace('_', "-")));
    rekindle_plugin_host::manifest::validate_id(&id)?;
    let title = title_of(name);
    match lang {
        Lang::Rust => new_rust(name, template, &id, &title, &root),
        Lang::TypeScript => new_typescript(name, template, &id, &title, &root),
    }
}

/// Every scaffold's .gitignore starts with this: `rekindle keygen` run in a
/// project would otherwise leave a secret key ready to commit.
const GITIGNORE_KEYS: &str = "# Signing keys: never commit the secret half.\n*.secret.json\n";

/// Scaffold a Rust plugin crate.
fn new_rust(name: &str, template: Template, id: &str, title: &str, root: &Path) -> Result<()> {
    let network = template.network();
    let network_line = if network.is_empty() {
        String::new()
    } else {
        format!("network = {}\n", toml_list(network))
    };
    let cargo_toml = format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
description = "A rekindle plugin."
publish = false

[lib]
crate-type = ["cdylib"]

# Identity and permissions. Users see `capabilities` and `network` when they
# install the plugin; ask only for what you use.
[package.metadata.rekindle]
id = "{id}"
name = "{title}"
capabilities = {caps}
{network_line}
[dependencies]
rekindle-sdk = {{ path = "{sdk}" }}

[dev-dependencies]
rekindle-test = {{ path = "{test}" }}

[profile.release]
opt-level = "s"
lto = true
strip = true

# Keep this crate out of any enclosing workspace.
[workspace]
"#,
        caps = toml_list(template.capabilities()),
        sdk = repo_crate("sdk")?,
        test = repo_crate("test-client")?,
    );

    write_files(
        root,
        &[
            ("Cargo.toml", cargo_toml),
            (
                "src/lib.rs",
                template.source(Lang::Rust).replace("{{TITLE}}", title),
            ),
            ("tests/plugin.rs", template.test_source(Lang::Rust).into()),
            (".gitignore", GITIGNORE_KEYS.to_string() + "/target\n"),
        ],
    )?;

    println!("created {} ({id})", root.display());
    let wasm = format!("produce target/rekindle/{id}.wasm");
    next_steps(
        root,
        &[
            ("rekindle dev", "live preview with hot reload"),
            ("rekindle test", "run tests/plugin.rs"),
            ("rekindle build", &wasm),
        ],
    );
    Ok(())
}

/// Scaffold a TypeScript plugin.
fn new_typescript(
    name: &str,
    template: Template,
    id: &str,
    title: &str,
    root: &Path,
) -> Result<()> {
    let id_json = serde_json::to_string(id)?;
    let title_json = serde_json::to_string(title)?;
    let caps_json = serde_json::to_string(template.capabilities())?;
    let network = if template.network().is_empty() {
        String::new()
    } else {
        format!(
            ",\n    \"network\": {}",
            serde_json::to_string(template.network())?
        )
    };
    let package_json = format!(
        r##"{{
  "name": "{name}",
  "version": "0.1.0",
  "description": "A rekindle plugin.",
  "private": true,
  "type": "module",
  "rekindle": {{
    "id": {id_json},
    "name": {title_json},
    "capabilities": {caps_json}{network}
  }},
  "imports": {{
    "#rekindle/test": "./target/rekindle/test/index.mjs"
  }},
  "scripts": {{
    "build": "rekindle build",
    "test": "rekindle test",
    "check": "tsc --noEmit",
    "postinstall": "rekindle sdk || echo rekindle is not installed, so the SDK types were not written"
  }},
  "devDependencies": {{
    "@bytecodealliance/jco": "{jco}",
    "typescript": "7.0.2"
  }}
}}
"##,
        jco = builder::js::JCO_VERSION,
    );
    write_files(
        root,
        &[
            ("package.json", package_json),
            (
                ".gitignore",
                GITIGNORE_KEYS.to_string() + "/target\n/node_modules\n",
            ),
            (
                "tsconfig.json",
                include_str!("../../templates/new/tsconfig.json").into(),
            ),
            (
                "src/index.ts",
                template
                    .source(Lang::TypeScript)
                    .replace("{{TITLE}}", title),
            ),
            (
                "tests/plugin.test.ts",
                template.test_source(Lang::TypeScript).into(),
            ),
        ],
    )?;
    // The SDK now, so the editor resolves `rekindle` imports before the
    // first build (the toolchain's declarations follow `npm install`).
    if let Err(e) = builder::write_editor_files(root) {
        eprintln!("warning: couldn't write the SDK: {e:#}");
    }

    println!("created {} ({id})", root.display());
    // A toolchain an ancestor directory already has (an npm workspace) is
    // used as it is.
    let shared =
        builder::guest::find_up(root, &["node_modules/@bytecodealliance/jco/package.json"]);
    if let Some(found) = &shared {
        let base = found
            .ancestors()
            .find(|a| a.ends_with("node_modules"))
            .and_then(Path::parent)
            .unwrap_or(found);
        let base = builder::guest::absolute(base).unwrap_or_else(|_| base.to_path_buf());
        println!(
            "  uses the toolchain already installed in {}",
            base.display()
        );
    }
    let wasm = format!("produce target/rekindle/{id}.wasm");
    let mut steps = vec![
        ("rekindle dev", "live preview with hot reload"),
        ("rekindle test", "run the tests in tests/"),
        ("rekindle build", wasm.as_str()),
    ];
    if shared.is_none() {
        steps.insert(
            0,
            (
                "npm install",
                "install the toolchain (or share one: put plugins in an npm workspace)",
            ),
        );
    }
    next_steps(root, &steps);
    Ok(())
}

/// Write each `(path, content)` under `root`, creating directories.
fn write_files(root: &Path, files: &[(&str, String)]) -> Result<()> {
    for (path, content) in files {
        let path = root.join(path);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        std::fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

fn inspect(wasm: &Path, json: bool) -> Result<()> {
    let built;
    let wasm = if wasm.is_dir() {
        let p = builder::read_project(wasm)?;
        built = p
            .target_dir
            .join("rekindle")
            .join(format!("{}.wasm", p.manifest.id));
        if !built.exists() {
            bail!(
                "{} isn't built yet: run `rekindle build` there first",
                wasm.display()
            );
        }
        built.as_path()
    } else {
        wasm
    };
    let bytes = std::fs::read(wasm).with_context(|| format!("reading {}", wasm.display()))?;
    if theme::is_theme(&bytes) {
        return inspect_theme(&bytes, json);
    }
    let engine = runtime::engine()?;
    let insp = runtime::inspect_artifact(&engine, &bytes)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&insp)?);
        return Ok(());
    }
    let m = &insp.manifest;
    println!("{} {} ({})", m.name, m.version, m.id);
    if !m.description.is_empty() {
        println!("  {}", m.description);
    }
    if !m.authors.is_empty() {
        println!("  by {}", m.authors.join(", "));
    }
    println!("  built against rekindle:plugin@{}", m.wit);
    println!("requests:");
    for cap in &m.capabilities {
        let used = match cap.interface() {
            None => "",
            Some(_) if insp.imported.contains(cap) => "",
            Some(_) => "  (declared but unused)",
        };
        println!("  - {cap}: {}{used}", cap.describe());
    }
    if m.requests(Capability::Network) {
        println!("network hosts:");
        for h in &m.network {
            println!("  - {h}{}", host_note(h));
        }
    }
    let hooks = insp.effective_hooks();
    println!("hooks (what the host calls into):");
    for h in &hooks {
        let what = match *h {
            "lifecycle" => "activate / deactivate / settings and feature changes",
            "event-handler" => "client events it subscribes to",
            "ui-handler" => "clicks, commands, keybinds, slash commands on its own UI",
            "timer-handler" => "its own timers",
            "compose-handler" => "the messages you send, before they go out",
            "renderer" => "how messages look (render isolate: can't keep or send what it sees)",
            "notification-filter" => "the client's notifications (render isolate)",
            _ => "",
        };
        println!("  - {h}: {what}");
    }
    let idle: Vec<&str> = insp
        .hooks
        .iter()
        .map(String::as_str)
        .filter(|h| !hooks.contains(h))
        .collect();
    if !idle.is_empty() {
        println!(
            "  (also exports {}, never called: their capabilities aren't requested)",
            idle.join(", ")
        );
    }
    if !m.settings.is_empty() {
        println!("settings:");
        for s in &m.settings {
            println!("  - {} ({:?}): {}", s.key, s.kind, s.label);
        }
    }
    println!("imports:");
    for i in &insp.imports {
        println!("  - {i}");
    }
    Ok(())
}

/// `inspect` for a theme file: what it sets, and what looks like a mistake.
fn inspect_theme(bytes: &[u8], json: bool) -> Result<()> {
    let t = theme::parse(bytes)?;
    if json {
        let out = serde_json::json!({ "manifest": t.manifest(), "warnings": t.warnings });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!("{} {} ({})", t.name, t.version, t.id);
    if !t.description.is_empty() {
        println!("  {}", t.description);
    }
    if !t.authors.is_empty() {
        println!("  by {}", t.authors.join(", "));
    }
    println!("  a theme: it runs no code, and holds only `theming`");
    let summary = |l: &theme::Layer| {
        let mut parts = vec![format!("{} tokens", l.tokens.len())];
        if l.tint.is_some() {
            parts.push("a tint".into());
        }
        if !l.surfaces.is_empty() {
            parts.push(format!("{} surfaces", l.surfaces.len()));
        }
        parts.join(", ")
    };
    println!("sets: {}", summary(&t.base));
    if !t.variants.is_empty() {
        println!("variants (picked on its settings page):");
        for (i, v) in t.variants.iter().enumerate() {
            let default = if i == t.default_variant {
                "  (default)"
            } else {
                ""
            };
            println!(
                "  - {} \"{}\": {}{default}",
                v.name,
                v.label,
                summary(&v.layer)
            );
        }
    }
    if let Some(u) = &t.updates {
        println!("updates from {}", u.host());
    }
    for w in &t.warnings {
        println!("warning: {w}");
    }
    Ok(())
}

/// What a listing adds after a network host: a Discord host is reached
/// without the user's credentials.
pub(super) fn host_note(h: &str) -> &'static str {
    if is_discord_host(h) {
        "  [Discord host, unauthenticated]"
    } else {
        ""
    }
}
