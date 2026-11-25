//! End to end, the community index: a plugin listed in the signed release
//! channel installs from the store without developer mode, after its review
//! screen; an update that asks for nothing new installs by itself, and one
//! that asks for more waits for review; the index's rules about namespaces
//! hold.

use std::path::Path;

use rekindle_plugin_host::capability::{Capability, Trust};
use rekindle_plugin_host::channel::{self, Blocked, Listing, Location, Source};
use rekindle_plugin_host::home::{Home, InstallState, TrustContext, TrustSource};
use rekindle_plugin_host::index;
use rekindle_plugin_host::manager::{Effect, Manager, PANEL};
use rekindle_plugin_host::manifest;
use rekindle_plugin_host::model::{Element, UiEvent, UiValue};
use rekindle_plugin_host::signing::Keypair;
use rekindle_plugin_host::{store, updates};

mod common;
use common::{midnight, scratch};

const NOW: u64 = 1_800_000_000;
const ID: &str = "dev.alice.midnight";

/// Midnight as Alice's plugin, at `version`.
fn alice(version: &str, edit: impl FnOnce(&mut manifest::Manifest)) -> Vec<u8> {
    alice_as(ID, version, edit)
}

fn alice_as(id: &str, version: &str, edit: impl FnOnce(&mut manifest::Manifest)) -> Vec<u8> {
    let wasm = midnight();
    let mut m = manifest::extract(&wasm).unwrap().unwrap();
    m.id = id.into();
    m.version = version.into();
    edit(&mut m);
    manifest::replace(&wasm, &m).unwrap()
}

fn source() -> Source {
    Source {
        repo: "https://example.com/alice/midnight.git".into(),
        commit: "c".repeat(40),
        path: String::new(),
    }
}

/// Publish a channel into `dir` whose community index lists `plugins`, as
/// `rekindle index build` and `channel build --community` would.
fn publish(
    dir: &Path,
    project: &Keypair,
    serial: u64,
    plugins: &[(&str, &[u8])],
    blocked: Vec<Blocked>,
) -> Location {
    // Absent before the first publish.
    let _ = std::fs::remove_dir_all(dir);
    let community = dir.join("community");
    std::fs::create_dir_all(&community).unwrap();
    for (i, (author, wasm)) in plugins.iter().enumerate() {
        let path = community.join(format!("plugin-{i}.wasm"));
        std::fs::write(&path, wasm).unwrap();
        let listing = Listing {
            author: author.to_string(),
            source: source(),
            any_host: false,
        };
        std::fs::write(
            channel::listing_path_for(&path),
            serde_json::to_vec(&listing).unwrap(),
        )
        .unwrap();
    }
    let c = channel::build(&channel::BuildOptions {
        dir,
        key: project,
        name: "test".into(),
        serial,
        expires: NOW + 14 * 86_400,
        plugin_keys: vec![project.public()],
        blocked,
        community: Some(&community),
        keep_community: vec![],
        framework: None,
        launcher: None,
    })
    .unwrap();
    let path = dir.join("channel.json");
    c.write_signed(&path, project).unwrap();
    Location::Path(path)
}

fn home_with(dir: &Path, project: &Keypair, channel: &Location) -> Home {
    let home =
        Home::new(dir.join("home")).with_trust(TrustContext::with_keys(vec![project.public()]));
    let Location::Path(p) = channel else {
        unreachable!()
    };
    home.set_channel(Some(p.display().to_string())).unwrap();
    home
}

fn click(m: &mut Manager, element: &str) -> Vec<Effect> {
    m.handle(&UiEvent {
        surface: PANEL.into(),
        element: element.into(),
        value: UiValue::Clicked,
    })
}

fn shown(m: &Manager) -> String {
    let lines: Vec<String> = m
        .panel(&[])
        .elements
        .iter()
        .flat_map(|e| match e {
            Element::Heading { text } | Element::Text { text } => vec![text.clone()],
            Element::Button { id, label, .. } => vec![format!("[{id}] {label}")],
            Element::Toggle { id, label, value } => vec![format!("[{id}={value}] {label}")],
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
        .collect();
    lines.join("\n")
}

/// Pick a tab of the grid: 0 Installed, 1 rekindle, 2 Community, 3 Settings.
fn tab(m: &mut Manager, index: u32) {
    m.handle(&UiEvent {
        surface: PANEL.into(),
        element: "tab".into(),
        value: UiValue::Selected(index),
    });
}

#[test]
fn a_listed_plugin_installs_from_the_store_without_developer_mode() {
    let dir = scratch("store");
    let project = Keypair::generate("Project").unwrap();
    let v1 = alice("1.0.0", |_| {});
    let loc = publish(&dir.join("dist1"), &project, 1, &[("alice", &v1)], vec![]);
    let home = home_with(&dir, &project, &loc);
    assert!(!home.developer_mode().unwrap());

    // Before an update check, the store is empty and the file is third-party.
    assert!(store::list(&home).is_empty());
    assert_eq!(home.trust_of(&v1, None).unwrap().trust, Trust::ThirdParty);
    assert!(
        home.install(&v1, None).is_err(),
        "third-party needs developer mode"
    );

    updates::check(&home, &loc, NOW).unwrap();
    let r = home.trust_of(&v1, None).unwrap();
    assert_eq!(r.trust, Trust::Community);
    assert_eq!(
        r.source,
        TrustSource::Community {
            author: "alice".into(),
            serial: 1
        }
    );

    // The Community tab lists it; installing goes through the review screen.
    let mut m = Manager::new(home.clone());
    assert!(
        shown(&m).contains("[tab=0] Installed | rekindle | Community | Settings"),
        "{}",
        shown(&m)
    );
    tab(&mut m, 2);
    let text = shown(&m);
    assert!(
        text.contains("by alice") && text.contains(&format!("[get:{ID}] Install…")),
        "{text}"
    );
    // Its card opens its page, which has the same button.
    click(&mut m, &format!("open:{ID}"));
    let text = shown(&m);
    assert!(
        text.contains("By alice, from the community index")
            && text.contains(&format!("[get:{ID}] Install…")),
        "{text}"
    );
    assert_eq!(
        click(&mut m, &format!("get:{ID}")),
        vec![Effect::Download { id: ID.into() }]
    );
    assert!(
        m.downloaded(ID, store::download(&home, ID)).is_empty(),
        "nothing is installed before the review"
    );
    let text = shown(&m);
    assert!(
        text.contains("from rekindle's community index") && text.contains("[install-reviewed]"),
        "{text}"
    );
    click(&mut m, "install-reviewed");

    let inst = home.get(ID).unwrap().unwrap();
    assert_eq!(
        (inst.state, inst.trust.unwrap().trust),
        (InstallState::Ready, Trust::Community)
    );
    // Installed, its page is open: the switch, and what it can do.
    let text = shown(&m);
    assert!(
        text.contains(&format!("[on:{ID}=true]"))
            && text.contains("It can: ")
            && text.contains(&format!("[remove:{ID}] Remove")),
        "{text}"
    );
    assert_eq!(m.detail_of(), Some(ID));
    click(&mut m, "back");
    tab(&mut m, 0);
    assert!(shown(&m).contains("· community"), "{}", shown(&m));
    tab(&mut m, 2);
    assert!(shown(&m).contains("| Installed"), "{}", shown(&m));

    // A download that isn't the listed file is refused.
    assert!(
        m.downloaded(
            ID,
            Ok((alice("1.0.0", |m| m.description = "changed".into()), None))
        )
        .is_empty()
    );
    assert!(shown(&m).contains("isn't the file the community index lists"));
}

#[test]
fn rekindles_own_plugins_install_from_their_tab_turned_off() {
    let dir = scratch("first-party-tab");
    let project = Keypair::generate("Project").unwrap();
    // A channel that lists Midnight as one of rekindle's own: the file and
    // its project-key signature, next to the channel.
    let dist = dir.join("dist");
    std::fs::create_dir_all(&dist).unwrap();
    let wasm = midnight();
    let id = manifest::extract(&wasm).unwrap().unwrap().id;
    let file = dist.join("midnight.wasm");
    std::fs::write(&file, &wasm).unwrap();
    project
        .sign_plugin(&wasm)
        .save(&rekindle_plugin_host::signing::sig_path_for(&file))
        .unwrap();
    let c = channel::build(&channel::BuildOptions {
        dir: &dist,
        key: &project,
        name: "test".into(),
        serial: 1,
        expires: NOW + 14 * 86_400,
        plugin_keys: vec![project.public()],
        blocked: vec![],
        community: None,
        keep_community: vec![],
        framework: None,
        launcher: None,
    })
    .unwrap();
    let path = dist.join("channel.json");
    c.write_signed(&path, &project).unwrap();
    let loc = Location::Path(path);
    let home = home_with(&dir, &project, &loc);

    // Nothing is listed before an update check has fetched the channel.
    let mut m = Manager::new(home.clone());
    tab(&mut m, 1);
    assert!(shown(&m).contains("Nothing to show yet"), "{}", shown(&m));
    updates::check(&home, &loc, NOW).unwrap();
    let text = shown(&m);
    assert!(
        text.contains(&format!("[open:{id}]")) && text.contains(&format!("[add:{id}] Install")),
        "{text}"
    );

    // Install: no review, and it starts off.
    assert_eq!(
        click(&mut m, &format!("add:{id}")),
        vec![Effect::Download { id: id.clone() }]
    );
    let fx = m.downloaded(&id, store::download(&home, &id));
    assert_eq!(
        fx,
        vec![Effect::Sync {
            ids: vec![id.clone()],
            reload: true
        }]
    );
    let inst = home.get(&id).unwrap().unwrap();
    assert_eq!(
        (inst.state, inst.trust.unwrap().trust),
        (InstallState::Off, Trust::FirstParty)
    );
    let text = shown(&m);
    assert!(
        text.contains(&format!("[on:{id}=false]")) && text.contains("Part of rekindle"),
        "{text}"
    );
    click(&mut m, "back");
    tab(&mut m, 1);
    let text = shown(&m);
    assert!(
        text.contains("| Installed") && !text.contains("[add:"),
        "{text}"
    );
}

#[test]
fn updates_that_ask_for_nothing_new_install_and_the_rest_wait_for_review() {
    let dir = scratch("updates");
    let project = Keypair::generate("Project").unwrap();
    let v1 = alice("1.0.0", |_| {});
    let loc = publish(&dir.join("dist1"), &project, 1, &[("alice", &v1)], vec![]);
    let home = home_with(&dir, &project, &loc);
    updates::check(&home, &loc, NOW).unwrap();
    home.install(&v1, None).unwrap();

    // 1.1.0 asks for the same: it installs by itself.
    let v11 = alice("1.1.0", |_| {});
    let loc = publish(
        &dir.join("dist2"),
        &project,
        2,
        &[("alice", &v1), ("alice", &v11)],
        vec![],
    );
    let r = updates::check(&home, &loc, NOW).unwrap();
    assert_eq!(r.updated.len(), 1, "{r:?}");
    let inst = home.get(ID).unwrap().unwrap();
    assert_eq!(
        (inst.state, inst.manifest.unwrap().version.as_str()),
        (InstallState::Ready, "1.1.0")
    );

    // 1.2.0 also asks for the clipboard: downloaded for review, not installed.
    let v12 = alice("1.2.0", |m| m.capabilities.push(Capability::Clipboard));
    let loc = publish(
        &dir.join("dist3"),
        &project,
        3,
        &[("alice", &v1), ("alice", &v11), ("alice", &v12)],
        vec![],
    );
    let r = updates::check(&home, &loc, NOW).unwrap();
    assert!(r.updated.is_empty() && r.reviews.len() == 1, "{r:?}");
    assert_eq!(
        home.get(ID).unwrap().unwrap().manifest.unwrap().version,
        "1.1.0"
    );
    let waiting = home
        .pending_review(ID)
        .unwrap()
        .expect("the update waits for review");
    assert_eq!(
        (waiting.manifest.version.as_str(), waiting.sig.is_none()),
        ("1.2.0", true)
    );
    assert!(
        updates::check(&home, &loc, NOW).unwrap().reviews.len() == 1,
        "still waiting, not downloaded again"
    );

    // The user reviews it from the panel, without developer mode.
    let mut m = Manager::new(home.clone());
    click(&mut m, &format!("open:{ID}"));
    let text = shown(&m);
    assert!(text.contains(&format!("[update:{ID}]")), "{text}");
    click(&mut m, &format!("update:{ID}"));
    assert!(shown(&m).contains("NEW"));
    click(&mut m, "install-reviewed");
    assert_eq!(
        home.get(ID).unwrap().unwrap().manifest.unwrap().version,
        "1.2.0"
    );
    assert!(home.pending_review(ID).unwrap().is_none());

    // Taking a version down is a block.
    let blocked = Blocked {
        sha256: rekindle_plugin_host::signing::sha256_hex(&v12),
        reason: "it broke".into(),
        id: None,
    };
    let loc = publish(
        &dir.join("dist4"),
        &project,
        4,
        &[("alice", &v1), ("alice", &v11), ("alice", &v12)],
        vec![blocked],
    );
    updates::check(&home, &loc, NOW).unwrap();
    assert_eq!(
        home.get(ID).unwrap().unwrap().state,
        InstallState::Blocked("it broke".into())
    );
}

#[test]
fn the_index_keeps_namespaces_apart() {
    let dir = scratch("namespaces");
    let project = Keypair::generate("Project").unwrap();
    let ok = alice("1.0.0", |_| {});
    let squat = alice_as("dev.alice.other", "1.0.0", |_| {});
    let first_party = alice_as("dev.rekindle.midnight-copy", "1.0.0", |_| {});
    let build = |plugins: &[(&str, &[u8])]| {
        let community = dir.join("community");
        // Absent before the first publish.
        let _ = std::fs::remove_dir_all(&community);
        std::fs::create_dir_all(&community).unwrap();
        for (i, (author, wasm)) in plugins.iter().enumerate() {
            let path = community.join(format!("p{i}.wasm"));
            std::fs::write(&path, wasm).unwrap();
            std::fs::write(
                channel::listing_path_for(&path),
                serde_json::to_vec(&Listing {
                    author: author.to_string(),
                    source: source(),
                    any_host: false,
                })
                .unwrap(),
            )
            .unwrap();
        }
        channel::build(&channel::BuildOptions {
            dir: &dir,
            key: &project,
            name: "test".into(),
            serial: 1,
            expires: NOW + 1,
            plugin_keys: vec![],
            blocked: vec![],
            community: Some(&community),
            keep_community: vec![],
            framework: None,
            launcher: None,
        })
    };
    build(&[("alice", &ok)]).unwrap();
    let err = build(&[("alice", &ok), ("mallory", &squat)]).unwrap_err();
    assert!(err.to_string().contains("belongs to alice"), "{err:#}");
    let err = build(&[("mallory", &first_party)]).unwrap_err();
    assert!(err.to_string().contains("first-party namespace"), "{err:#}");

    // `rekindle check` refuses the first-party namespace too.
    let findings = index::prereview(&first_party).unwrap();
    assert!(index::has_errors(&findings), "{findings:?}");
    assert!(!index::has_errors(&index::prereview(&ok).unwrap()));
}
