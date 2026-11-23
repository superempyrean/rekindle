//! TypeScript plugins (and plain-JavaScript entries), built with jco (`jco componentize`).
//!
//! The project is a `package.json` with a `rekindle` table; the code starts at
//! `src/index.ts` (or `.js`). `rekindle build` generates an entry module that
//! imports the plugin's handlers (named exports, or a default-exported
//! object), fills in the ones it leaves out, and adapts the JS runtime to the
//! sandbox (see `templates/js/prelude.mjs`). jco bundles it with the plugin's
//! own modules and npm dependencies, and embeds a JS engine:
//!
//! - `quickjs` (default): small (~1.5 MB) and quick to load.
//! - `starlingmonkey` (SpiderMonkey): ~12 MB and slower to load; a faster
//!   interpreter for heavy work.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use super::guest::{self, ScriptMeta, WORLD};
use crate::manifest::Manifest;

/// The jco release these builds are made and tested with.
pub const JCO_VERSION: &str = "1.41.0";

/// The TypeScript the shared toolchain installs.
pub const TYPESCRIPT_VERSION: &str = "7.0.2";

/// jco's package file, relative to a `node_modules` parent.
const JCO_PACKAGE: &str = "node_modules/@bytecodealliance/jco/package.json";

/// The JavaScript toolchain (jco and TypeScript) shared by every project
/// that has no `node_modules` of its own: `cache/js-toolchain/<jco version>`
/// in the rekindle home.
pub fn shared_toolchain_dir() -> Result<PathBuf> {
    Ok(crate::home::Home::from_env()?
        .root()
        .join("cache")
        .join("js-toolchain")
        .join(JCO_VERSION))
}

/// Install the shared toolchain with npm, unless it is already installed.
/// Returns its directory.
pub fn install_shared_toolchain() -> Result<PathBuf> {
    let dir = shared_toolchain_dir()?;
    let done = dir.join(".installed");
    if done.is_file() {
        return Ok(dir);
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::write(
        dir.join("package.json"),
        format!(
            r#"{{"name":"rekindle-js-toolchain","private":true,"dependencies":{{"@bytecodealliance/jco":"{JCO_VERSION}","typescript":"{TYPESCRIPT_VERSION}"}}}}"#
        ),
    )?;
    eprintln!(
        "note: installing jco {JCO_VERSION} and TypeScript {TYPESCRIPT_VERSION} into {}, once for every plugin without its own node_modules",
        dir.display()
    );
    let mut npm = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "npm"]);
        c
    } else {
        Command::new("npm")
    };
    let status = npm
        .current_dir(&dir)
        .args(["install", "--no-audit", "--no-fund", "--loglevel", "error"])
        .status()
        .context(
            "running npm (JavaScript and TypeScript plugins need Node.js, which includes npm)",
        )?;
    if !status.success() {
        bail!(
            "npm couldn't install the JavaScript toolchain into {}",
            dir.display()
        );
    }
    std::fs::write(&done, JCO_VERSION)?;
    Ok(dir)
}

/// A toolchain file: the project's (or an ancestor's) `node_modules` first,
/// then the shared toolchain, installed first if `install`.
fn toolchain_file(dir: &Path, rel: &str, install: bool) -> Result<Option<PathBuf>> {
    if let Some(p) = guest::find_up(dir, &[rel]) {
        return Ok(Some(p));
    }
    let shared = if install {
        install_shared_toolchain()?
    } else {
        shared_toolchain_dir()?
    };
    let p = shared.join(rel);
    Ok(p.exists().then_some(p))
}

const PRELUDE: &str = include_str!("../../templates/js/prelude.mjs");
const ENTRY: &str = include_str!("../../templates/js/entry.mjs");

/// The `rekindle` SDK modules (`rekindle`, `rekindle/ui`, ...), written to
/// `<target>/rekindle/sdk` on each build: bundled from there, and found there
/// by the editor through the scaffolded tsconfig's `paths`. One module per
/// contract interface that needs a capability, so importing a module is what
/// imports the interface.
const SDK: [(&str, &str); 34] = [
    ("index.ts", include_str!("../../templates/sdk/js/index.ts")),
    (
        "globals.d.ts",
        include_str!("../../templates/sdk/js/globals.d.ts"),
    ),
    (
        "elements.ts",
        include_str!("../../templates/sdk/js/elements.ts"),
    ),
    (
        "events.ts",
        include_str!("../../templates/sdk/js/events.ts"),
    ),
    (
        "settings.ts",
        include_str!("../../templates/sdk/js/settings.ts"),
    ),
    ("time.ts", include_str!("../../templates/sdk/js/time.ts")),
    ("timer.ts", include_str!("../../templates/sdk/js/timer.ts")),
    ("state.ts", include_str!("../../templates/sdk/js/state.ts")),
    ("users.ts", include_str!("../../templates/sdk/js/users.ts")),
    (
        "messages.ts",
        include_str!("../../templates/sdk/js/messages.ts"),
    ),
    (
        "local-actions.ts",
        include_str!("../../templates/sdk/js/local-actions.ts"),
    ),
    (
        "reactions.ts",
        include_str!("../../templates/sdk/js/reactions.ts"),
    ),
    (
        "actions.ts",
        include_str!("../../templates/sdk/js/actions.ts"),
    ),
    (
        "presence.ts",
        include_str!("../../templates/sdk/js/presence.ts"),
    ),
    (
        "moderation.ts",
        include_str!("../../templates/sdk/js/moderation.ts"),
    ),
    (
        "history.ts",
        include_str!("../../templates/sdk/js/history.ts"),
    ),
    ("view.ts", include_str!("../../templates/sdk/js/view.ts")),
    ("ui.ts", include_str!("../../templates/sdk/js/ui.ts")),
    ("slots.ts", include_str!("../../templates/sdk/js/slots.ts")),
    (
        "decorations.ts",
        include_str!("../../templates/sdk/js/decorations.ts"),
    ),
    (
        "commands.ts",
        include_str!("../../templates/sdk/js/commands.ts"),
    ),
    (
        "compose.ts",
        include_str!("../../templates/sdk/js/compose.ts"),
    ),
    (
        "render.ts",
        include_str!("../../templates/sdk/js/render.ts"),
    ),
    (
        "notify.ts",
        include_str!("../../templates/sdk/js/notify.ts"),
    ),
    ("audio.ts", include_str!("../../templates/sdk/js/audio.ts")),
    ("files.ts", include_str!("../../templates/sdk/js/files.ts")),
    (
        "compose-control.ts",
        include_str!("../../templates/sdk/js/compose-control.ts"),
    ),
    (
        "tweaks.ts",
        include_str!("../../templates/sdk/js/tweaks.ts"),
    ),
    (
        "consts.ts",
        include_str!("../../templates/sdk/js/consts.ts"),
    ),
    ("theme.ts", include_str!("../../templates/sdk/js/theme.ts")),
    (
        "storage.ts",
        include_str!("../../templates/sdk/js/storage.ts"),
    ),
    ("http.ts", include_str!("../../templates/sdk/js/http.ts")),
    (
        "clipboard.ts",
        include_str!("../../templates/sdk/js/clipboard.ts"),
    ),
    (
        "overlay.ts",
        include_str!("../../templates/sdk/js/overlay.ts"),
    ),
];

/// The test client, `#rekindle/test` (the scaffold's package.json maps it to
/// `target/rekindle/test/index.mjs`). It runs in Node under `rekindle test`,
/// not in the plugin.
const TEST_CLIENT: [(&str, &str); 2] = [
    (
        "index.mjs",
        include_str!("../../templates/test/js/index.mjs"),
    ),
    (
        "index.d.mts",
        include_str!("../../templates/test/js/index.d.mts"),
    ),
];

/// Write the SDK to `<target>/rekindle/sdk` and the test client to
/// `<target>/rekindle/test`; returns the SDK's directory. Only files whose
/// content changed are written, so concurrent builds and the editor's file
/// watching stay quiet.
fn write_sdk(target_dir: &Path) -> Result<PathBuf> {
    let sdk = target_dir.join("rekindle").join("sdk");
    write_changed(&sdk, &SDK)?;
    write_changed(&target_dir.join("rekindle").join("test"), &TEST_CLIENT)?;
    guest::absolute(&sdk)
}

fn write_changed(dir: &Path, files: &[(&str, &str)]) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    for (name, content) in files {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(*content) {
            std::fs::write(&path, content)
                .with_context(|| format!("writing {}", path.display()))?;
        }
    }
    Ok(())
}

/// Where a build saves the plugin's source map: next to the component.
pub(super) fn source_map_path(target_dir: &Path, id: &str) -> PathBuf {
    target_dir.join("rekindle").join(format!("{id}.js.map"))
}

/// Write what an editor needs without building: the SDK, the test client,
/// and (if jco is installed) the contract's TypeScript declarations. Returns
/// whether the declarations were written.
pub(super) fn write_editor_files(dir: &Path, target_dir: &Path, m: &Manifest) -> Result<bool> {
    write_sdk(target_dir)?;
    let Ok(jco) = jco(dir) else { return Ok(false) };
    let build = guest::BuildDir::new(target_dir, m)?;
    write_types(dir, &jco, &build.wit(), target_dir)?;
    Ok(true)
}

/// The capabilities the bundle's `rekindle:plugin/*` imports need that the
/// manifest doesn't request, from the list the bundle config saved.
fn missing_from_bundle(imports: &Path, m: &Manifest) -> Vec<crate::capability::Capability> {
    let Ok(list) = std::fs::read(imports)
        .map_err(anyhow::Error::from)
        .and_then(|b| Ok(serde_json::from_slice::<Vec<String>>(&b)?))
    else {
        return Vec::new();
    };
    guest::missing_for_interfaces(
        list.iter()
            .filter_map(|i| i.strip_prefix("rekindle:plugin/"))
            .map(|i| i.split('@').next().unwrap_or(i)),
        m,
    )
}

/// Rewrite the map the bundle config saved: `sources` relative to the
/// project instead of the build directory, so stack frames read
/// `src/index.ts:13:5`.
fn finish_source_map(raw: &Path, build_dir: &Path, project: &Path, out: &Path) -> Result<()> {
    let mut map: serde_json::Value = serde_json::from_slice(&std::fs::read(raw)?)?;
    let project = super::dunce(project);
    let build_dir = super::dunce(build_dir);
    if let Some(sources) = map.get_mut("sources").and_then(|s| s.as_array_mut()) {
        for s in sources.iter_mut() {
            let Some(rel) = s.as_str() else { continue };
            let abs = super::dunce(&build_dir.join(rel));
            // The entry module and prelude are rekindle's; their frames are
            // dropped from stack traces.
            let shown = if abs.starts_with(&build_dir) {
                crate::sourcemap::INTERNAL.to_string()
            } else {
                abs.strip_prefix(&project)
                    .map(Path::to_path_buf)
                    .unwrap_or(abs)
                    .to_string_lossy()
                    .replace('\\', "/")
            };
            *s = serde_json::Value::String(shown);
        }
    }
    // Only positions are needed; the sources themselves are on disk.
    if let Some(obj) = map.as_object_mut() {
        obj.remove("sourcesContent");
    }
    std::fs::write(out, serde_json::to_vec(&map)?)
        .with_context(|| format!("writing {}", out.display()))
}

/// The JS engine jco embeds in the component (`rekindle.engine`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Engine {
    QuickJs,
    StarlingMonkey,
}

/// How to build a JS or TS plugin, from its `package.json`.
#[derive(Debug, Clone)]
pub(super) struct Config {
    /// The plugin's entry file.
    entry: PathBuf,
    engine: Engine,
}

#[derive(Deserialize)]
struct PackageJson {
    name: Option<String>,
    version: Option<String>,
    #[serde(default)]
    description: String,
    author: Option<Person>,
    #[serde(default)]
    contributors: Vec<Person>,
    rekindle: Option<ScriptMeta>,
}

/// npm's `"Name <email>"` string or `{ "name", "email" }` object.
#[derive(Deserialize)]
#[serde(untagged)]
enum Person {
    Line(String),
    Object { name: String, email: Option<String> },
}

impl Person {
    fn line(&self) -> String {
        match self {
            Person::Line(s) => s.clone(),
            Person::Object {
                name,
                email: Some(e),
            } => format!("{name} <{e}>"),
            Person::Object { name, email: None } => name.clone(),
        }
    }
}

/// Read `package.json` into a manifest and build config.
pub(super) fn read(package_json: &Path) -> Result<(Manifest, Config)> {
    let raw = std::fs::read(package_json)
        .with_context(|| format!("reading {}", package_json.display()))?;
    let pkg: PackageJson = serde_json::from_slice(&raw)
        .with_context(|| format!("{} is not valid package.json", package_json.display()))?;
    let meta = pkg
        .rekindle
        .with_context(|| format!("{} has no `rekindle` table", package_json.display()))?;
    let dir = package_json.parent().unwrap_or(Path::new("."));
    let name = pkg.name.as_deref().context("package.json has no `name`")?;
    let version = pkg
        .version
        .as_deref()
        .context("package.json has no `version`")?;
    let authors: Vec<String> = pkg
        .author
        .iter()
        .chain(&pkg.contributors)
        .map(Person::line)
        .collect();
    let manifest = guest::manifest_from(&meta, dir, name, version, &pkg.description, &authors)?;

    let entry = match &meta.entry {
        Some(e) => dir.join(e),
        None => [
            "src/index.ts",
            "src/index.js",
            "src/index.mts",
            "src/index.mjs",
        ]
        .iter()
        .map(|e| dir.join(e))
        .find(|p| p.exists())
        .with_context(|| {
            format!(
                "no entry file: add src/index.ts (or set `rekindle.entry` in {})",
                package_json.display()
            )
        })?,
    };
    let engine = match meta.engine.as_deref() {
        None | Some("quickjs") => Engine::QuickJs,
        Some("starlingmonkey") => Engine::StarlingMonkey,
        Some(other) => {
            bail!("unknown `rekindle.engine` `{other}` (expected `quickjs` or `starlingmonkey`)")
        }
    };
    Ok((manifest, Config { entry, engine }))
}

/// jco's CLI script: the project's (or an ancestor's) `node_modules`, else
/// the shared toolchain, which this installs if it has to.
fn jco(dir: &Path) -> Result<PathBuf> {
    #[derive(Deserialize)]
    struct Bin {
        bin: serde_json::Value,
    }
    let pkg = toolchain_file(dir, JCO_PACKAGE, true)?.context(
        "the shared JavaScript toolchain has no jco; delete its directory and build again",
    )?;
    let bin: Bin =
        serde_json::from_slice(&std::fs::read(&pkg)?).context("reading jco's package.json")?;
    let rel = match &bin.bin {
        serde_json::Value::String(s) => s.clone(),
        v => v
            .get("jco")
            .and_then(|s| s.as_str())
            .context("jco's package.json names no `jco` bin")?
            .to_string(),
    };
    let jco_dir = pkg
        .parent()
        .expect("find_up returns a file under node_modules");
    Ok(jco_dir.join(rel))
}

fn node(dir: &Path) -> Command {
    let mut c = Command::new("node");
    c.current_dir(dir);
    c
}

/// Build the component (no manifest section yet), and refresh the
/// TypeScript declarations (see [`write_types`]).
pub(super) fn build(
    dir: &Path,
    target_dir: &Path,
    m: &Manifest,
    cfg: &Config,
    quiet: bool,
    typecheck: bool,
) -> Result<Vec<u8>> {
    let jco = jco(dir)?;
    let build = guest::BuildDir::new(target_dir, m)?;
    let entry = guest::absolute(&cfg.entry)
        .with_context(|| format!("entry file {}", cfg.entry.display()))?;
    if let Some(name) = std::fs::read_to_string(&entry)
        .ok()
        .as_deref()
        .and_then(async_handler)
    {
        bail!(
            "`{name}` is async, but handlers must be synchronous: the client calls them and takes what they return, \
             so code after an `await` would never run"
        );
    }

    let b = build.path();
    std::fs::write(b.join("prelude.mjs"), PRELUDE)?;
    std::fs::write(
        b.join("entry.mjs"),
        entry_module(&guest::relative_import(b, &entry), &guest::exports(m)),
    )?;
    let sdk = write_sdk(target_dir)?;
    // The declarations first: the type check reads them, and so does the
    // editor.
    let types = write_types(dir, &jco, &build.wit(), target_dir);
    if let Err(e) = &types
        && !quiet
    {
        eprintln!("warning: couldn't write TypeScript declarations: {e:#}");
    }
    if typecheck && types.is_ok() {
        type_check(dir, &entry, quiet)?;
    }
    let raw_map = b.join("bundle.js.map");
    let imports = b.join("bundle.imports.json");
    std::fs::write(
        b.join("bundle.config.mjs"),
        bundle_config(&b.join("prelude.mjs"), &sdk, b, &raw_map, &imports),
    )?;
    let out = b.join("component.wasm");

    let mut cmd = node(dir);
    cmd.arg(&jco).arg("componentize").arg(b.join("entry.mjs"));
    cmd.arg("--wit")
        .arg(build.wit())
        .args(["--world-name", WORLD]);
    cmd.arg("--bundle")
        .arg("--bundle-config")
        .arg(b.join("bundle.config.mjs"));
    match cfg.engine {
        // Sync ABI (the host calls plugins synchronously); every WASI import
        // replaced by a trap stub, so none survive into the component.
        Engine::QuickJs => cmd.args([
            "--backend",
            "quickjs",
            "--backend-qjs-disable-async",
            "--backend-qjs-stub-wasi",
        ]),
        // `--disable all` builds a "pure" component with no WASI imports.
        Engine::StarlingMonkey => cmd.args(["--backend", "starlingmonkey", "--disable", "all"]),
    };
    cmd.arg("--out").arg(&out);
    if let Err(e) = guest::run(cmd, "jco componentize", m) {
        // The toolchain stops at the first interface the world lacks; the
        // bundle (which succeeded first) knows every one the plugin imports.
        let missing = missing_from_bundle(&imports, m);
        return Err(if guest::is_missing_capability(&e) && !missing.is_empty() {
            super::missing_capabilities(&missing)
        } else {
            e
        });
    }
    let component = std::fs::read(&out).with_context(|| format!("reading {}", out.display()))?;
    check_imports(&component)?;
    let map = source_map_path(target_dir, &m.id);
    if !raw_map.exists() {
        // An older build's map would point stack frames at the wrong lines;
        // if it can't be removed, it may not exist.
        let _ = std::fs::remove_file(&map);
    } else if let Err(e) = finish_source_map(&raw_map, b, dir, &map)
        && !quiet
    {
        eprintln!("warning: couldn't save the source map: {e:#}");
    }

    Ok(component)
}

/// The entry module for `plugin` (an import path), exporting the world's
/// hook interfaces (`exports`, as WIT names): the world says which exports
/// the component must have, and the module may have no others.
fn entry_module(plugin: &str, exports: &[&str]) -> String {
    let camel = |name: &str| {
        let mut out = String::new();
        let mut up = false;
        for c in name.chars() {
            if c == '-' {
                up = true;
            } else if up {
                out.extend(c.to_uppercase());
                up = false;
            } else {
                out.push(c);
            }
        }
        out
    };
    let hooks = serde_json::to_string(exports).expect("strings serialize");
    let names: Vec<String> = exports.iter().map(|e| camel(e)).collect();
    ENTRY
        .replace("{{PLUGIN}}", plugin)
        .replace("'{{HOOKS}}'", &hooks)
        .replace(
            "// {{EXPORTS}}",
            &format!("export {{ {} }};", names.join(", ")),
        )
}

/// The plugin's handlers (checked for `async` in the source). The entry
/// module has the same list.
const HANDLERS: [&str; 18] = [
    "activate",
    "deactivate",
    "onSettingsChanged",
    "onFeaturesChanged",
    "onEvents",
    "onEvent",
    "onUi",
    "onCommand",
    "onSlot",
    "onKeybind",
    "onSlashCommand",
    "onAutocomplete",
    "onTimer",
    "onCompose",
    "onUpload",
    "render",
    "onNotification",
    "onIntercept",
];

/// A handler declared `async` in the entry file (`export async function
/// onUi`, `async onUi(` in a default-exported object). Checked in the source
/// because the bundler lowers async functions, so the engine can't tell them
/// apart; the entry module also logs a handler that returns a Promise.
fn async_handler(src: &str) -> Option<&'static str> {
    let words: Vec<&str> = src
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .filter(|w| !w.is_empty())
        .collect();
    words.windows(3).find_map(|w| {
        let name = match *w {
            ["async", "function", name] => name,
            [_, "async", name] if src.contains(&format!("async {name}(")) => name,
            _ => return None,
        };
        HANDLERS.iter().copied().find(|h| *h == name)
    })
}

/// The Node globals the bundler binds to the prelude instead of to jco's
/// Node-compatibility modules (which import host interfaces rekindle doesn't
/// provide).
const PRELUDE_GLOBALS: [&str; 5] = [
    "console",
    "setTimeout",
    "setInterval",
    "clearTimeout",
    "clearInterval",
];

/// The bundler configuration: `rekindle:` imports are the component's own,
/// not modules to bundle; `rekindle` and `rekindle/*` are the SDK; and
/// `console`/timers come from the prelude.
///
/// It also saves the bundle's source map to `map` (with `sources` relative to
/// `out_dir`) and its external imports to `imports`, and drops the map from
/// the bundle's output, because jco refuses a bundle with any file besides
/// the code.
fn bundle_config(prelude: &Path, sdk: &Path, out_dir: &Path, map: &Path, imports: &Path) -> String {
    let js_path = |p: &Path| {
        serde_json::to_string(&p.to_string_lossy().replace('\\', "/")).expect("a string serializes")
    };
    let prelude = js_path(prelude);
    let inject: Vec<String> = PRELUDE_GLOBALS
        .iter()
        .map(|g| format!("    {g}: [{prelude}, '{g}'],"))
        .collect();
    format!(
        "import {{ writeFileSync }} from 'node:fs';\n\
         export default {{\n  external: [/^rekindle:/],\n  resolve: {{ alias: {{ rekindle: {} }} }},\n  transform: {{ inject: {{\n{}\n  }} }},\n\
         \x20 output: {{ dir: {}, sourcemap: true }},\n\
         \x20 plugins: [{{\n    name: 'rekindle-source-map',\n    generateBundle(_, bundle) {{\n\
         \x20     for (const [name, file] of Object.entries(bundle)) {{\n\
         \x20       if (file.type === 'chunk' && file.map) writeFileSync({}, JSON.stringify(file.map));\n\
         \x20       if (file.type === 'chunk') writeFileSync({}, JSON.stringify(file.imports));\n\
         \x20       if (file.type === 'asset' && name.endsWith('.map')) delete bundle[name];\n\
         \x20     }}\n    }},\n  }}],\n}};\n",
        js_path(sdk),
        inject.join("\n"),
        js_path(out_dir),
        js_path(map),
        js_path(imports),
    )
}

/// Refuse a component that imports anything beyond `rekindle:plugin`. jco
/// adds an import when the code uses a Node global or builtin it backs with a
/// host interface (`process`, `fetch`, `node:fs`, ...); the host would refuse
/// such a component at load, so say what pulled it in now.
fn check_imports(component: &[u8]) -> Result<()> {
    let foreign: Vec<String> = guest::component_imports(component)?
        .into_iter()
        .filter(|i| !i.starts_with("rekindle:plugin/"))
        .collect();
    if foreign.is_empty() {
        return Ok(());
    }
    let mut msg = format!(
        "the plugin imports {}, which rekindle plugins don't get: jco adds such an import when the code (or a \
         dependency) uses a Node.js global or builtin backed by a host interface, like `fetch`, `node:fs` or \
         `node:http`. Plugins get only the rekindle API; `console`, timers and pure-JS polyfills (`Buffer`, \
         `node:path`, ...) are provided.",
        foreign
            .iter()
            .map(|i| format!("`{i}`"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if foreign.iter().any(|i| i.starts_with("jco:node/http")) {
        msg.push_str(" For HTTP, use `rekindle/http` (`http.get(url)`) with the `network` capability and the host in `rekindle.network`.");
    }
    if foreign.iter().any(|i| i.starts_with("jco:node/fs")) {
        msg.push_str(" There's no filesystem: use `rekindle/storage` (the `storage` capability).");
    }
    bail!("{msg}")
}

/// `tsc --noEmit` on the project, with its own `tsconfig.json` and
/// TypeScript, if the entry is TypeScript. A project without either isn't
/// set up to be checked: it builds, with a note saying so.
fn type_check(dir: &Path, entry: &Path, quiet: bool) -> Result<()> {
    let ts = entry
        .extension()
        .is_some_and(|e| e == "ts" || e == "mts" || e == "tsx");
    if !ts {
        return Ok(());
    }
    let note = |why: &str| {
        if !quiet {
            eprintln!("note: {why}, so the plugin wasn't type-checked");
        }
    };
    if !dir.join("tsconfig.json").is_file() {
        note("the project has no tsconfig.json");
        return Ok(());
    }
    let Some(tsc) = toolchain_file(dir, "node_modules/typescript/bin/tsc", false)? else {
        note("TypeScript isn't installed");
        return Ok(());
    };
    let out = node(dir)
        .arg(tsc)
        .args(["--noEmit", "--pretty", "false", "-p", "."])
        .output()
        .context("running tsc")?;
    if out.status.success() {
        return Ok(());
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let errors: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let shown = errors
        .iter()
        .take(30)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let more = errors.len().saturating_sub(30);
    bail!(
        "the plugin has type errors (`rekindle build --no-check` builds it anyway):\n{shown}{}",
        if more > 0 {
            format!("\n... and {more} more")
        } else {
            String::new()
        }
    )
}

/// Write TypeScript declarations for every `rekindle:plugin/*` module to
/// `<target>/rekindle/types`, for the editor and `tsc`. They're for the whole
/// contract (the `plugin` world), not the plugin's cut-down one, so the SDK's
/// type-only references (a `Draft` in the `Plugin` interface) resolve
/// whatever the capabilities; the build itself still refuses an import of an
/// interface the manifest doesn't cover.
///
/// They are generated beside the directory and swapped in only if they
/// differ, so a build of an unchanged project doesn't touch the files
/// another build, `rekindle dev` or the editor is reading.
fn write_types(dir: &Path, jco: &Path, wit: &Path, target_dir: &Path) -> Result<()> {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let types = target_dir.join("rekindle").join("types");
    let fresh = target_dir.join("rekindle").join(format!(
        "types.new-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let mut cmd = node(dir);
    cmd.arg(jco)
        .arg("guest-types")
        .arg(wit)
        .args(["--world-name", "plugin"])
        .arg("--out-dir")
        .arg(&fresh);
    let out = cmd.output().context("running jco guest-types")?;
    // `fresh` has a name no other build uses, so one left behind is only
    // clutter.
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(&fresh);
        bail!(
            "jco guest-types failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    if same_tree(&fresh, &types) {
        let _ = std::fs::remove_dir_all(&fresh);
        return Ok(());
    }
    // An older build's world declarations would shadow these.
    if types.exists() {
        std::fs::remove_dir_all(&types).with_context(|| format!("clearing {}", types.display()))?;
    }
    std::fs::rename(&fresh, &types)
        .with_context(|| format!("moving the declarations to {}", types.display()))
}

/// Whether two directories hold the same files with the same contents.
fn same_tree(a: &Path, b: &Path) -> bool {
    fn files(root: &Path) -> Option<std::collections::BTreeMap<PathBuf, Vec<u8>>> {
        let mut out = std::collections::BTreeMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).ok()? {
                let path = e.ok()?.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.insert(
                        path.strip_prefix(root).ok()?.to_path_buf(),
                        std::fs::read(&path).ok()?,
                    );
                }
            }
        }
        Some(out)
    }
    matches!((files(a), files(b)), (Some(x), Some(y)) if x == y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(tag: &str, package_json: &str, files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rekindle-js-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("package.json"), package_json).unwrap();
        for f in files {
            std::fs::write(dir.join(f), "").unwrap();
        }
        dir
    }

    #[test]
    fn reads_package_json() {
        let dir = project(
            "read",
            r#"{ "name": "word-count", "version": "1.2.0", "description": "Counts.",
                 "author": { "name": "Ada", "email": "ada@example.com" }, "contributors": ["Bo <bo@example.com>"],
                 "rekindle": { "id": "dev.ada.words", "name": "Word Count", "capabilities": ["ui", "storage"] } }"#,
            &["src/index.js", "src/index.ts"],
        );
        let (m, cfg) = read(&dir.join("package.json")).unwrap();
        assert_eq!(
            (m.id.as_str(), m.name.as_str(), m.version.as_str()),
            ("dev.ada.words", "Word Count", "1.2.0")
        );
        assert_eq!(m.authors, ["Ada <ada@example.com>", "Bo <bo@example.com>"]);
        assert_eq!(m.description, "Counts.");
        // `.ts` wins over `.js`.
        assert_eq!(cfg.entry, dir.join("src/index.ts"));
        assert_eq!(cfg.engine, Engine::QuickJs);
    }

    #[test]
    fn the_async_check_knows_every_handler_the_entry_module_does() {
        let (_, rest) = ENTRY
            .split_once("const HANDLERS = [")
            .expect("entry.mjs lists its handlers");
        let (list, _) = rest.split_once("];").expect("the list ends");
        let entry: Vec<&str> = list
            .split(',')
            .map(|h| h.trim().trim_matches('\''))
            .filter(|h| !h.is_empty())
            .collect();
        assert_eq!(entry, HANDLERS);
    }

    #[test]
    fn finds_async_handlers() {
        assert_eq!(
            async_handler("export async function activate() { await x; }"),
            Some("activate")
        );
        assert_eq!(
            async_handler("export default definePlugin({\n  async onUi(ev) {}\n});"),
            Some("onUi")
        );
        assert_eq!(
            async_handler("export function onUi() {}\nasync function helper() {}"),
            None
        );
        assert_eq!(
            async_handler("const s = 'async function activate'; export function activate() {}"),
            Some("activate"),
            "conservative on strings"
        );
        assert_eq!(
            async_handler("export function onUi(ev) { load(); }\nasync function onUiHelper() {}"),
            None
        );
    }

    #[test]
    fn rejects_bad_metadata() {
        let bad = |tag: &str, rekindle: &str| {
            let dir = project(
                tag,
                &format!(r#"{{ "name": "x", "version": "0.1.0", "rekindle": {rekindle} }}"#),
                &["src/index.js"],
            );
            format!("{:#}", read(&dir.join("package.json")).unwrap_err())
        };
        assert!(
            bad(
                "cap",
                r#"{ "id": "dev.x.y", "capabilities": ["telepathy"] }"#
            )
            .contains("unknown capability")
        );
        assert!(
            bad("engine", r#"{ "id": "dev.x.y", "engine": "v8" }"#)
                .contains("unknown `rekindle.engine`")
        );
        assert!(
            bad("field", r#"{ "id": "dev.x.y", "capabilites": [] }"#).contains("unknown field")
        );
        assert!(
            bad("net", r#"{ "id": "dev.x.y", "capabilities": ["network"] }"#)
                .contains("no hosts declared")
        );
        let dir = project(
            "noentry",
            r#"{ "name": "x", "version": "0.1.0", "rekindle": { "id": "dev.x.y" } }"#,
            &[],
        );
        assert!(
            format!("{:#}", read(&dir.join("package.json")).unwrap_err()).contains("no entry file")
        );
    }
}
