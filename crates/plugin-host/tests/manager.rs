//! rekindle's own settings panel, end to end against a real home and
//! runtime: everything a user does without a terminal. Installing from a
//! file, turning plugins on and off, developer mode behind its typed
//! confirmation, the review screen, removing, and third-party updates
//! offered for review from the author's own signed channel.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rekindle_plugin_host::capability::Capability;
use rekindle_plugin_host::channel::{self, Location};
use rekindle_plugin_host::home::{Home, InstallState, TrustContext};
use rekindle_plugin_host::manager::{self, Effect, Manager, PANEL, Synced};
use rekindle_plugin_host::manifest::{self, UpdateSource};
use rekindle_plugin_host::model::{Element, UiEvent, UiValue};
use rekindle_plugin_host::signing::{self, Keypair};
use rekindle_plugin_host::updates;
use rekindle_plugin_host::{MockBackend, Runtime, SharedBackend, Trust};

mod common;
use common::{midnight, scratch};

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 86_400;

fn midnight_at(version: &str, edit: impl FnOnce(&mut manifest::Manifest)) -> Vec<u8> {
    let wasm = midnight();
    let mut m = manifest::extract(&wasm).unwrap().unwrap();
    m.version = version.into();
    edit(&mut m);
    manifest::replace(&wasm, &m).unwrap()
}

fn id() -> String {
    manifest::extract(&midnight()).unwrap().unwrap().id
}

fn runtime_for(home: &Home) -> Runtime {
    let backend = Arc::new(Mutex::new(MockBackend::new()));
    let shared: SharedBackend = backend;
    Runtime::new(shared, home.runtime_config()).unwrap()
}

fn click(m: &mut Manager, element: &str) -> Vec<Effect> {
    m.handle(&UiEvent {
        surface: PANEL.into(),
        element: element.into(),
        value: UiValue::Clicked,
    })
}

fn toggle(m: &mut Manager, element: &str, on: bool) -> Vec<Effect> {
    m.handle(&UiEvent {
        surface: PANEL.into(),
        element: element.into(),
        value: UiValue::Toggled(on),
    })
}

fn type_text(m: &mut Manager, element: &str, text: &str) {
    m.handle(&UiEvent {
        surface: PANEL.into(),
        element: element.into(),
        value: UiValue::Text(text.into()),
    });
}

/// Every element's id and text, to look for things the user would see.
fn shown(m: &Manager) -> Vec<String> {
    m.panel(&[])
        .elements
        .iter()
        .flat_map(|e| match e {
            Element::Heading { text } | Element::Text { text } => vec![text.clone()],
            Element::Toggle { id, label, value } => vec![format!("[{id}={value}] {label}")],
            Element::Button { id, label, .. } | Element::TextInput { id, label, .. } => {
                vec![format!("[{id}] {label}")]
            }
            Element::Tabs { tabs, selected, .. } => {
                vec![format!("[tab={selected}] {}", tabs.join(" | "))]
            }
            Element::Cards { cards } => cards
                .iter()
                .flat_map(|c| {
                    let mut lines = vec![format!(
                        "[{}] {} | {} | {} | {}",
                        c.id, c.title, c.meta, c.text, c.state
                    )];
                    lines.extend(c.action.iter().map(|a| format!("[{}] {}", a.id, a.label)));
                    lines
                })
                .collect(),
            Element::Header {
                back,
                title,
                meta,
                toggle,
                action,
            } => {
                let mut lines = vec![format!("[{back}] Back"), title.clone(), meta.clone()];
                lines.extend(
                    toggle
                        .iter()
                        .map(|t| format!("[{}={}] {}", t.id, t.value, t.label)),
                );
                lines.extend(action.iter().map(|a| format!("[{}] {}", a.id, a.label)));
                lines
            }
            other => vec![format!("{other:?}")],
        })
        .collect()
}

/// Pick a tab of the grid: 0 Installed, 1 rekindle, 2 Community, 3 Settings.
fn tab(m: &mut Manager, index: u32) {
    m.handle(&UiEvent {
        surface: PANEL.into(),
        element: "tab".into(),
        value: UiValue::Selected(index),
    });
}

fn has(m: &Manager, needle: &str) -> bool {
    shown(m).iter().any(|l| l.contains(needle))
}

/// Apply what the manager asked for, as the client does.
fn run(home: &Home, rt: &mut Runtime, effects: Vec<Effect>) -> Vec<Synced> {
    let mut out = Vec::new();
    for e in effects {
        match e {
            Effect::Sync { ids, reload } => {
                for id in ids {
                    out.push(manager::sync(home, rt, &id, reload).unwrap());
                }
            }
            Effect::SyncAll => out.extend(
                manager::sync_all(home, rt, &[id()])
                    .into_iter()
                    .map(|(_, r)| r.unwrap()),
            ),
            other => panic!("unexpected effect {other:?}"),
        }
    }
    out
}

fn confirm_field(m: &Manager) -> String {
    m.panel(&[])
        .elements
        .iter()
        .find_map(|e| match e {
            Element::TextInput { id, .. } if id.starts_with("confirm-") => Some(id.clone()),
            _ => None,
        })
        .expect("the confirmation field")
}

#[test]
fn first_party_plugins_install_from_a_file_and_turn_on_and_off() {
    let dir = scratch("first-party");
    let project = Keypair::generate("Project").unwrap();
    let home =
        Home::new(dir.join("home")).with_trust(TrustContext::with_keys(vec![project.public()]));
    let mut rt = runtime_for(&home);
    let mut m = Manager::new(home.clone());
    assert!(has(&m, "No plugins installed."));
    assert_eq!(click(&mut m, "install"), vec![Effect::PickFile]);

    // The user picks a signed file: it installs at once, off.
    let wasm = midnight_at("1.0.0", |_| {});
    let file = dir.join("midnight.wasm");
    std::fs::write(&file, &wasm).unwrap();
    project
        .sign_plugin(&wasm)
        .save(&signing::sig_path_for(&file))
        .unwrap();
    let fx = m.open_file(&file);
    assert_eq!(run(&home, &mut rt, fx), vec![Synced::Unchanged]);
    // Its page opens, with the switch.
    assert!(has(&m, "It's off"), "{:?}", shown(&m));
    assert!(has(&m, &format!("[on:{}=false]", id())));
    assert!(has(&m, "Part of rekindle"), "{:?}", shown(&m));
    assert!(rt.plugin(&id()).is_none());

    // The grid has its card, with a button that does what the switch does.
    click(&mut m, "back");
    assert!(
        has(&m, &format!("[open:{}]", id())) && has(&m, "| Off"),
        "{:?}",
        shown(&m)
    );
    let fx = click(&mut m, &format!("enable:{}", id()));
    assert_eq!(run(&home, &mut rt, fx), vec![Synced::Started]);
    assert!(
        has(&m, "| On") && has(&m, &format!("[disable:{}] Turn off", id())),
        "{:?}",
        shown(&m)
    );
    let fx = click(&mut m, &format!("disable:{}", id()));
    assert_eq!(run(&home, &mut rt, fx), vec![Synced::Stopped]);
    click(&mut m, &format!("open:{}", id()));

    // On: it starts. Off: it stops.
    let fx = toggle(&mut m, &format!("on:{}", id()), true);
    assert_eq!(run(&home, &mut rt, fx), vec![Synced::Started]);
    assert!(rt.plugin(&id()).is_some());
    assert!(has(&m, &format!("[on:{}=true]", id())));
    let fx = toggle(&mut m, &format!("on:{}", id()), false);
    assert_eq!(run(&home, &mut rt, fx), vec![Synced::Stopped]);
    assert!(rt.plugin(&id()).is_none());

    // Remove asks first; cancel keeps it, remove takes it away.
    toggle(&mut m, &format!("on:{}", id()), true);
    run(
        &home,
        &mut rt,
        vec![Effect::Sync {
            ids: vec![id()],
            reload: false,
        }],
    );
    assert!(click(&mut m, &format!("remove:{}", id())).is_empty());
    assert!(has(&m, "[remove-confirm]"), "{:?}", shown(&m));
    click(&mut m, "cancel");
    assert!(home.get(&id()).unwrap().is_some());
    assert!(
        has(&m, &format!("[on:{}=true]", id())),
        "cancel goes back to its page: {:?}",
        shown(&m)
    );
    click(&mut m, &format!("remove:{}", id()));
    let fx = click(&mut m, "remove-confirm");
    assert_eq!(run(&home, &mut rt, fx), vec![Synced::Stopped]);
    assert!(home.get(&id()).unwrap().is_none());
    assert!(has(&m, "Removed"));
}

#[test]
fn third_party_needs_developer_mode_typed_confirmation_and_a_review() {
    let dir = scratch("third-party");
    let home = Home::new(dir.join("home")).with_trust(TrustContext::with_keys(vec![
        Keypair::generate("Project").unwrap().public(),
    ]));
    let mut rt = runtime_for(&home);
    let mut m = Manager::new(home.clone());
    let wasm = midnight_at("1.0.0", |m| m.capabilities.push(Capability::Clipboard));
    let file = dir.join("midnight.wasm");
    std::fs::write(&file, &wasm).unwrap();

    // Unsigned, so not part of rekindle: refused with developer mode off.
    assert!(m.open_file(&file).is_empty());
    assert!(has(&m, "needs developer mode"), "{:?}", shown(&m));
    assert!(home.get(&id()).unwrap().is_none());

    // Developer mode: the warning, and nothing until the phrase is typed.
    click(&mut m, "developer-mode-on");
    assert!(
        has(
            &m,
            "Nobody from rekindle or Discord will ever ask you to turn this on"
        ),
        "{:?}",
        shown(&m)
    );
    assert!(click(&mut m, "developer-mode-confirm").is_empty());
    assert!(!home.developer_mode().unwrap());
    let field = confirm_field(&m);
    type_text(&mut m, &field, "i understand");
    assert_eq!(
        click(&mut m, "developer-mode-confirm"),
        vec![Effect::SyncAll]
    );
    assert!(home.developer_mode().unwrap());
    // Turning it on again later starts with an empty field.
    assert!(has(&m, "[developer-mode-off]"));

    // The file now goes to the review screen; nothing installs until Install.
    assert!(m.open_file(&file).is_empty());
    assert!(
        has(&m, "This plugin is not part of rekindle"),
        "{:?}",
        shown(&m)
    );
    assert!(has(&m, "[install-reviewed]"));
    assert!(home.get(&id()).unwrap().is_none());
    click(&mut m, "cancel");
    assert!(home.get(&id()).unwrap().is_none());
    m.open_file(&file);
    let fx = click(&mut m, "install-reviewed");
    assert_eq!(
        run(&home, &mut rt, fx),
        vec![Synced::Started],
        "a reviewed third-party plugin starts on"
    );
    let inst = home.get(&id()).unwrap().unwrap();
    assert_eq!(inst.trust.unwrap().trust, Trust::ThirdParty);

    // Developer mode off stops it, and the list says why.
    let fx = click(&mut m, "developer-mode-off");
    assert_eq!(run(&home, &mut rt, fx), vec![Synced::Stopped]);
    assert!(
        has(&m, "Off while developer mode is off"),
        "{:?}",
        shown(&m)
    );
    click(&mut m, "back");
    tab(&mut m, 0);
    assert!(has(&m, "| Off: developer mode is off"), "{:?}", shown(&m));

    // Back on, with a fresh confirmation field.
    click(&mut m, "developer-mode-on");
    assert_ne!(confirm_field(&m), field);
    let field = confirm_field(&m);
    type_text(&mut m, &field, "I understand");
    let fx = click(&mut m, "developer-mode-confirm");
    assert_eq!(run(&home, &mut rt, fx), vec![Synced::Started]);

    // A file changed on disk needs a review before it runs again.
    let changed = midnight_at("1.0.1", |_| {});
    std::fs::write(home.plugin_dir(&id()).join("plugin.wasm"), &changed).unwrap();
    assert!(matches!(
        home.get(&id()).unwrap().unwrap().state,
        InstallState::NeedsReview(_)
    ));
    assert_eq!(
        manager::sync(&home, &mut rt, &id(), false).unwrap(),
        Synced::Stopped
    );
    click(&mut m, &format!("review:{}", id()));
    assert!(has(&m, "[install-reviewed]"), "{:?}", shown(&m));
    let fx = click(&mut m, "install-reviewed");
    assert_eq!(run(&home, &mut rt, fx), vec![Synced::Started]);
}

/// An author's channel in `dir`, listing `plugins` (each signed by
/// `signer`), the channel signed by `author`.
fn author_publish(
    dir: &Path,
    author: &Keypair,
    signer: &Keypair,
    serial: u64,
    plugins: &[&[u8]],
) -> Location {
    // Absent before the first publish.
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    for (i, wasm) in plugins.iter().enumerate() {
        let path = dir.join(format!("plugin-{i}.wasm"));
        std::fs::write(&path, wasm).unwrap();
        signer
            .sign_plugin(wasm)
            .save(&signing::sig_path_for(&path))
            .unwrap();
    }
    let c = channel::build(&channel::BuildOptions {
        dir,
        key: author,
        name: "author".into(),
        serial,
        expires: NOW + 14 * DAY,
        plugin_keys: vec![signer.public()],
        community: None,
        keep_community: vec![],
        blocked: vec![],
        framework: None,
        launcher: None,
    })
    .unwrap();
    let path = dir.join("channel.json");
    c.write_signed(&path, author).unwrap();
    Location::Path(path)
}

const AUTHOR_CHANNEL: &str = "https://plugins.example.com/channel.json";

/// Midnight at `version`, updated from `author`'s channel, asking for
/// `extra` on top of what it requests.
fn from_author(author: &Keypair, version: &str, extra: Option<Capability>) -> Vec<u8> {
    let source = UpdateSource {
        url: AUTHOR_CHANNEL.into(),
        key: author.public().to_base64(),
    };
    midnight_at(version, move |m| {
        m.updates = Some(source);
        m.capabilities.extend(extra);
    })
}

/// A home in developer mode with `v1` from `author` installed and running.
fn running_from_author(dir: &Path, author: &Keypair, v1: &[u8]) -> (Home, Runtime) {
    let home = Home::new(dir.join("home")).with_trust(TrustContext::with_keys(vec![
        Keypair::generate("Project").unwrap().public(),
    ]));
    home.set_developer_mode(true).unwrap();
    home.install(v1, Some(&author.sign_plugin(v1))).unwrap();
    let mut rt = runtime_for(&home);
    assert_eq!(
        manager::sync(&home, &mut rt, &id(), false).unwrap(),
        Synced::Started
    );
    (home, rt)
}

/// Check for reviews with the author's channel served from `served`.
fn check_author(home: &Home, served: Location) -> updates::Reviews {
    updates::check_reviews_with(home, NOW, move |url| {
        assert_eq!(url, AUTHOR_CHANNEL, "only the declared source is contacted");
        Ok(served.clone())
    })
    .unwrap()
}

#[test]
fn third_party_updates_are_offered_for_review_from_the_authors_signed_channel() {
    let dir = scratch("reviews");
    let author = Keypair::generate("Author").unwrap();
    let v1 = from_author(&author, "1.0.0", None);
    let (home, mut rt) = running_from_author(&dir, &author, &v1);

    // The install screen said where it checks.
    let screen = rekindle_plugin_host::review::screen(
        &manifest::extract(&v1).unwrap().unwrap(),
        &home.trust_of(&v1, None).unwrap(),
        None,
    );
    assert!(rekindle_plugin_host::review::render(&screen).contains("plugins.example.com"));

    // 1.1.0 asks for more: downloaded for review, not installed.
    let v11 = from_author(&author, "1.1.0", Some(Capability::Clipboard));
    let served = author_publish(&dir.join("dist1"), &author, &author, 3, &[&v1, &v11]);
    let r = check_author(&home, served);
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    assert_eq!(
        r.ready.iter().map(|u| u.to.as_str()).collect::<Vec<_>>(),
        ["1.1.0"]
    );
    assert_eq!(
        home.get(&id()).unwrap().unwrap().manifest.unwrap().version,
        "1.0.0",
        "nothing installed"
    );
    assert_eq!(rt.plugin(&id()).unwrap().version, "1.0.0");

    // The panel offers it; the review says what's new; Install swaps it in.
    let mut m = Manager::new(home.clone());
    assert!(
        has(&m, "update to review"),
        "its card says so: {:?}",
        shown(&m)
    );
    click(&mut m, &format!("open:{}", id()));
    assert!(
        has(&m, &format!("[update:{}] Review the update to", id())),
        "{:?}",
        shown(&m)
    );
    click(&mut m, &format!("update:{}", id()));
    assert!(has(&m, "Updating from 1.0.0"), "{:?}", shown(&m));
    assert!(has(&m, "NEW"), "{:?}", shown(&m));
    let fx = click(&mut m, "install-reviewed");
    assert_eq!(run(&home, &mut rt, fx), vec![Synced::Restarted]);
    assert_eq!(rt.plugin(&id()).unwrap().version, "1.1.0");
    let inst = home.get(&id()).unwrap().unwrap();
    assert_eq!(
        (inst.state, inst.trust.unwrap().trust),
        (InstallState::Ready, Trust::ThirdParty)
    );
    assert!(
        home.pending_review(&id()).unwrap().is_none(),
        "the downloaded file is gone"
    );
    assert!(!has(&m, "[update:"));

    // Declining drops the download (it is offered again at the next check).
    let v12 = from_author(&author, "1.2.0", None);
    let served = author_publish(&dir.join("dist2"), &author, &author, 4, &[&v12]);
    assert_eq!(check_author(&home, served).ready.len(), 1);
    m.refresh_reviews();
    click(&mut m, &format!("update:{}", id()));
    click(&mut m, "cancel");
    assert!(home.pending_review(&id()).unwrap().is_none());
}

#[test]
fn author_updates_signed_by_anyone_else_or_older_are_refused() {
    let dir = scratch("refused-reviews");
    let author = Keypair::generate("Author").unwrap();
    let v1 = from_author(&author, "1.0.0", None);
    let (home, _rt) = running_from_author(&dir, &author, &v1);
    let served = author_publish(&dir.join("dist1"), &author, &author, 3, &[&v1]);
    assert!(check_author(&home, served).ready.is_empty());

    // A channel or a file signed by anyone else is refused.
    let v12 = from_author(&author, "1.2.0", None);
    let other = Keypair::generate("Someone else").unwrap();
    let served = author_publish(&dir.join("dist2"), &other, &other, 5, &[&v12]);
    let r = check_author(&home, served);
    assert!(
        r.ready.is_empty() && r.failed[0].1.contains("isn't signed by the key"),
        "{:?}",
        r.failed
    );
    let served = author_publish(&dir.join("dist3"), &author, &other, 5, &[&v12]);
    let r = check_author(&home, served);
    assert!(
        r.ready.is_empty() && r.failed[0].1.contains("not signed by the author's key"),
        "{:?}",
        r.failed
    );
    assert!(home.pending_review(&id()).unwrap().is_none());

    // An older channel is a rollback.
    let served = author_publish(&dir.join("dist4"), &author, &author, 2, &[&v12]);
    let r = check_author(&home, served);
    assert!(
        r.failed[0].1.contains("refusing to roll back"),
        "{:?}",
        r.failed
    );

    // Developer mode off: no checks at all.
    home.set_developer_mode(false).unwrap();
    let r = updates::check_reviews_with(&home, NOW, |_| {
        panic!("contacted the author with developer mode off")
    })
    .unwrap();
    assert!(r.ready.is_empty() && r.failed.is_empty());
}

#[test]
fn compiled_plugins_are_kept_in_the_home() {
    let dir = scratch("compile-cache");
    let home = Home::new(dir.to_path_buf());
    let cache = home.compile_cache_dir();
    let files = || walk(&cache);
    let load = || {
        let backend: SharedBackend = Arc::new(Mutex::new(MockBackend::new()));
        let mut rt = Runtime::new(backend, home.runtime_config()).unwrap();
        rt.load(&midnight(), Trust::FirstParty).unwrap();
    };
    load();
    let after_first = files();
    assert!(
        !after_first.is_empty(),
        "nothing written to {}",
        cache.display()
    );
    load();
    let after_second = files();
    assert_eq!(
        after_second, after_first,
        "the second load reuses what the first compiled"
    );
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else if p.extension().is_some_and(|x| x == "cwasm") {
            out.push(p);
        }
    }
    out.sort();
    out
}

#[test]
fn the_developer_tab_reloads_shows_logs_and_runs_console_actions() {
    use rekindle_plugin_host::model::{LogLevel, LogRecord};

    let dir = scratch("developer-tab");
    let home = Home::new(dir.join("home"));
    let mut rt = runtime_for(&home);
    rt.load(&midnight(), Trust::FirstParty).unwrap();
    let mut m = Manager::new(home.clone());
    let dev = [manager::DevPlugin {
        id: id(),
        name: "Midnight".into(),
        path: "midnight.wasm".into(),
        on: true,
    }];
    let lines = |m: &Manager| -> Vec<String> {
        m.panel(&dev)
            .elements
            .iter()
            .map(|e| format!("{e:?}"))
            .collect()
    };

    // Without developer mode there is no Developer tab, and its controls
    // do nothing.
    assert!(!lines(&m).iter().any(|l| l.contains("Developer")));
    assert!(click(&mut m, &format!("reload:{}", id())).is_empty());
    assert!(click(&mut m, "console-run").is_empty());

    home.set_developer_mode(true).unwrap();
    tab(&mut m, 3);
    assert!(m.console_open());
    let shown = lines(&m);
    assert!(
        shown
            .iter()
            .any(|l| l.contains(&format!("reload:{}", id()))),
        "{shown:?}"
    );
    assert!(shown.iter().any(|l| l.contains("console-input")));

    // The log, filtered to one plugin.
    let rec = |plugin: &str, message: &str| LogRecord {
        plugin: plugin.into(),
        level: LogLevel::Info,
        message: message.into(),
        at_ms: 0,
    };
    m.set_logs(vec![rec("a.one", "from one"), rec("b.two", "from two")]);
    assert!(
        lines(&m)
            .iter()
            .any(|l| l.contains("from one") && l.contains("from two"))
    );
    m.handle(&UiEvent {
        surface: PANEL.into(),
        element: "console-filter".into(),
        value: UiValue::Selected(2),
    });
    let shown = lines(&m);
    assert!(shown.iter().any(|l| l.contains("from two")));
    assert!(!shown.iter().any(|l| l.contains("from one")), "{shown:?}");

    assert_eq!(
        click(&mut m, &format!("reload:{}", id())),
        vec![Effect::Reload { id: id() }]
    );

    // The console runs what was typed, against the runtime.
    type_text(&mut m, "console-input", r#"{"action":"plugins"}"#);
    let fx = click(&mut m, "console-run");
    let [Effect::Eval { command }] = fx.as_slice() else {
        panic!("{fx:?}")
    };
    let out = manager::console::run(&mut rt, command);
    assert!(out.contains(&id()) && out.contains("active"), "{out}");
    m.set_console_result(out);
    assert!(lines(&m).iter().any(|l| l.contains("active")));
    assert!(manager::console::run(&mut rt, "not json").starts_with("error:"));
    assert!(
        manager::console::run(&mut rt, r#"{"action":"command","plugin":"x.y","id":"z"}"#)
            .starts_with("error:")
    );

    // Developer mode off: the tab goes, and the grid falls back to Installed.
    home.set_developer_mode(false).unwrap();
    assert!(!m.console_open());
    assert!(!lines(&m).iter().any(|l| l.contains("console-input")));
}

#[test]
fn the_menu_opens_a_list_of_plugins_or_themes_and_back_returns() {
    let dir = scratch("lists");
    let project = Keypair::generate("Project").unwrap();
    let home =
        Home::new(dir.join("home")).with_trust(TrustContext::with_keys(vec![project.public()]));
    let install = |wasm: Vec<u8>| {
        let sig = project.sign_plugin(&wasm);
        home.install(&wasm, Some(&sig)).unwrap();
    };
    // Midnight sets colours, so it is a theme. Without theming, it isn't.
    install(midnight_at("1.0.0", |_| {}));
    install(midnight_at("1.0.0", |m| {
        m.id = "dev.example.plain".into();
        m.name = "Plain".into();
        m.capabilities.retain(|c| *c != Capability::Theming);
    }));
    home.set_enabled("dev.example.plain", true).unwrap();
    let mut m = Manager::new(home.clone());

    let menu = m.menu();
    let Element::Cards { cards } = &menu.elements[0] else {
        panic!("{:?}", menu.elements)
    };
    let rows: Vec<(&str, &str)> = cards
        .iter()
        .map(|c| (c.id.as_str(), c.meta.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("list:plugins", "1 installed, 1 on"),
            ("list:themes", "1 installed, 0 on")
        ]
    );
    assert!(matches!(
        &menu.elements[1],
        Element::Toggle { id, value: false, .. } if id == "developer-mode"
    ));

    click(&mut m, "list:themes");
    assert!(has(&m, "Themes") && has(&m, &format!("[open:{}]", id())));
    assert!(!has(&m, "[open:dev.example.plain]"), "{:?}", shown(&m));
    // A page opened from a list goes back to it, and the list to the grid.
    click(&mut m, &format!("open:{}", id()));
    assert!(has(&m, &format!("[on:{}=false]", id())));
    click(&mut m, "back");
    assert!(has(&m, &format!("[open:{}]", id())) && has(&m, "Themes"));
    click(&mut m, "back");
    assert!(has(&m, "[tab=0]"), "{:?}", shown(&m));
    click(&mut m, "list:plugins");
    assert!(has(&m, "[open:dev.example.plain]") && !has(&m, &format!("[open:{}]", id())));
    // Turning one off from the list keeps the list open.
    click(&mut m, "disable:dev.example.plain");
    assert!(has(&m, "1 installed, 0 on"), "{:?}", shown(&m));
    assert!(click(&mut m, "list:nothing").is_empty());
    assert!(has(&m, "no list `nothing`"));

    // The menu's switch asks before turning developer mode on.
    toggle(&mut m, "developer-mode", true);
    assert!(has(&m, "Turn on developer mode?"), "{:?}", shown(&m));
    assert!(!home.developer_mode().unwrap());
    home.set_developer_mode(true).unwrap();
    assert_eq!(
        toggle(&mut m, "developer-mode", false),
        vec![Effect::SyncAll]
    );
    assert!(!home.developer_mode().unwrap());
}

#[test]
fn a_plugins_page_shows_what_it_did_with_anything_undeclared_first() {
    use rekindle_plugin_host::runtime::ledger::{self, Activity, Days};

    let dir = scratch("activity");
    let project = Keypair::generate("Project").unwrap();
    let home =
        Home::new(dir.join("home")).with_trust(TrustContext::with_keys(vec![project.public()]));
    let wasm = midnight_at("1.0.0", |_| {});
    let sig = project.sign_plugin(&wasm);
    home.install(&wasm, Some(&sig)).unwrap();
    home.set_enabled(&id(), true).unwrap();
    let mut rt = runtime_for(&home);
    manager::sync(&home, &mut rt, &id(), false).unwrap();

    let mut m = Manager::new(home.clone());
    click(&mut m, &format!("open:{}", id()));
    assert!(
        has(&m, &format!("[activity:{}] What it did", id())),
        "{:?}",
        shown(&m)
    );
    click(&mut m, &format!("activity:{}", id()));
    assert_eq!(m.activity_of(), Some(id().as_str()));

    // The client writes the runtime's counts before it draws the page.
    rt.flush_activity();
    let lines = shown(&m);
    assert!(lines.iter().any(|l| l.starts_with("What ")), "{lines:?}");
    assert!(has(&m, "Calls to rekindle"), "{lines:?}");
    assert!(has(&m, "theming"), "{lines:?}");
    assert!(!has(&m, "Outside its manifest"), "{lines:?}");

    // A day on which it tried a host it doesn't list, and hid an element
    // it doesn't declare.
    let path = home.plugin_dir(&id()).join(ledger::FILE_NAME);
    let mut days = Days::load(&path);
    let mut odd = Activity::default();
    odd.refused_hosts.insert("evil.example.net".into(), 3);
    odd.elements
        .entry(ledger::ElementVerb::Hide)
        .or_default()
        .insert("composer.gift".into(), 1);
    days.record(ledger::today() - 1, &odd);
    days.save(&path).unwrap();
    assert!(has(&m, "Outside its manifest"), "{:?}", shown(&m));
    assert!(
        has(
            &m,
            "Tried to contact evil.example.net, which it doesn't list, 3 times."
        ),
        "{:?}",
        shown(&m)
    );
    assert!(has(&m, "Hid the gift button, which it doesn't list, once."));
    assert!(has(&m, "By day"));

    // Back goes to its page.
    click(&mut m, "back");
    assert_eq!(m.detail_of(), Some(id().as_str()));
    assert!(m.activity_of().is_none());
}
