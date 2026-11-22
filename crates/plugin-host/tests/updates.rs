//! End to end, the release channel: first-party plugins update themselves
//! (and roll back when an update doesn't start), an old, replayed, expired
//! or foreign channel is refused, and the blocklist turns plugins off.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rekindle_plugin_host::capability::{Capability, Trust};
use rekindle_plugin_host::channel::{self, Blocked, Channel, Location};
use rekindle_plugin_host::home::{Home, InstallState, TrustContext};
use rekindle_plugin_host::manifest;
use rekindle_plugin_host::runtime::Status;
use rekindle_plugin_host::signing::{self, Keypair};
use rekindle_plugin_host::updates::{self, Outcome};
use rekindle_plugin_host::{MockBackend, Runtime, SharedBackend};

mod common;
use common::{midnight, scratch};

const NOW: u64 = 1_800_000_000;
const DAY: u64 = 86_400;

/// Midnight with another version (and, optionally, other capabilities).
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

fn home_with(dir: &Path, project: &Keypair) -> Home {
    Home::new(dir.join("home")).with_trust(TrustContext::with_keys(vec![project.public()]))
}

fn runtime_for(home: &Home) -> (Runtime, Arc<Mutex<MockBackend>>) {
    let backend = Arc::new(Mutex::new(MockBackend::new()));
    let shared: SharedBackend = backend.clone();
    (
        Runtime::new(shared, home.runtime_config()).unwrap(),
        backend,
    )
}

/// Publish `plugins` (signed by `project`) and a channel listing them into
/// `dir`, as the project's release step would. Returns the channel's location.
fn publish(
    dir: &Path,
    project: &Keypair,
    serial: u64,
    plugins: &[&[u8]],
    blocked: Vec<Blocked>,
) -> Location {
    // Absent before the first publish.
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    for (i, wasm) in plugins.iter().enumerate() {
        let path = dir.join(format!("plugin-{i}.wasm"));
        std::fs::write(&path, wasm).unwrap();
        project
            .sign_plugin(wasm)
            .save(&signing::sig_path_for(&path))
            .unwrap();
    }
    let c = channel::build(&channel::BuildOptions {
        dir,
        key: project,
        name: "test".into(),
        serial,
        expires: NOW + 14 * DAY,
        plugin_keys: vec![project.public()],
        community: None,
        keep_community: vec![],
        blocked,
        framework: None,
        launcher: None,
    })
    .unwrap();
    let path = dir.join("channel.json");
    c.write_signed(&path, project).unwrap();
    Location::Path(path)
}

#[test]
fn first_party_plugins_update_themselves_while_running() {
    let dir = scratch("update");
    let project = Keypair::generate("Project").unwrap();
    let home = home_with(&dir, &project);
    let v1 = midnight_at("1.0.0", |_| {});
    home.install(&v1, Some(&project.sign_plugin(&v1))).unwrap();
    home.set_enabled(&id(), true).unwrap();
    let (mut rt, _b) = runtime_for(&home);
    assert!(home.load_into(&mut rt).unwrap()[0].result.is_ok());

    // Nothing newer: nothing to do.
    let loc = publish(&dir.join("dist1"), &project, 1, &[&v1], vec![]);
    let r = updates::check(&home, &loc, NOW).unwrap();
    assert!(r.updated.is_empty() && r.failed.is_empty(), "{r:?}");

    // 1.2.0 published (and an older 1.1.0 listed alongside): it installs on
    // disk, keeps 1.0.0 as previous.wasm, and swaps into the runtime.
    let v11 = midnight_at("1.1.0", |_| {});
    let v12 = midnight_at("1.2.0", |m| m.capabilities.push(Capability::Clipboard));
    let loc = publish(&dir.join("dist2"), &project, 2, &[&v11, &v12], vec![]);
    let r = updates::check(&home, &loc, NOW).unwrap();
    assert_eq!(r.updated.len(), 1, "{r:?}");
    assert_eq!(
        (r.updated[0].from.as_str(), r.updated[0].to.as_str()),
        ("1.0.0", "1.2.0")
    );
    assert!(home.plugin_dir(&id()).join("previous.wasm").exists());
    let inst = home.get(&id()).unwrap().unwrap();
    assert_eq!(
        inst.state,
        InstallState::Ready,
        "still on, still first-party"
    );
    assert_eq!(inst.manifest.unwrap().version, "1.2.0");

    let outcomes = updates::apply(&home, &mut rt, &r);
    assert_eq!(
        outcomes,
        vec![Outcome::Updated {
            id: id(),
            to: "1.2.0".into()
        }]
    );
    let info = rt.plugin(&id()).unwrap();
    assert_eq!(
        (info.version.as_str(), info.status),
        ("1.2.0", Status::Active)
    );
    assert!(info.granted.contains(&Capability::Clipboard));

    // Automatic updates off: newer versions are reported, not installed.
    home.set_manual_updates(true).unwrap();
    let v13 = midnight_at("1.3.0", |_| {});
    let loc = publish(&dir.join("dist3"), &project, 3, &[&v13], vec![]);
    let r = updates::check(&home, &loc, NOW).unwrap();
    assert!(r.updated.is_empty() && r.available.len() == 1, "{r:?}");
    assert_eq!(
        home.get(&id()).unwrap().unwrap().manifest.unwrap().version,
        "1.2.0"
    );
}

#[test]
fn an_update_that_does_not_start_rolls_back_and_is_not_offered_again() {
    let dir = scratch("rollback");
    let project = Keypair::generate("Project").unwrap();
    let home = home_with(&dir, &project);
    let v1 = midnight_at("1.0.0", |_| {});
    home.install(&v1, Some(&project.sign_plugin(&v1))).unwrap();
    home.set_enabled(&id(), true).unwrap();
    let (mut rt, _b) = runtime_for(&home);
    assert!(home.load_into(&mut rt).unwrap()[0].result.is_ok());

    // A broken 2.0.0: its code uses theming, its manifest no longer asks.
    let broken = midnight_at("2.0.0", |m| {
        m.capabilities.retain(|c| *c != Capability::Theming);
    });
    let loc = publish(&dir.join("dist"), &project, 1, &[&broken], vec![]);
    let r = updates::check(&home, &loc, NOW).unwrap();
    assert_eq!(r.updated.len(), 1, "{r:?}");

    let outcomes = updates::apply(&home, &mut rt, &r);
    assert!(
        matches!(&outcomes[..], [Outcome::RolledBack { why, .. }] if why.contains("theming")),
        "{outcomes:?}"
    );
    // The running version was never disturbed, and the disk is back too.
    let info = rt.plugin(&id()).unwrap();
    assert_eq!(
        (info.version.as_str(), info.status),
        ("1.0.0", Status::Active)
    );
    let inst = home.get(&id()).unwrap().unwrap();
    assert_eq!(inst.manifest.unwrap().version, "1.0.0");
    assert_eq!(inst.state, InstallState::Ready);
    assert!(
        inst.record
            .unwrap()
            .bad_updates
            .contains(&signing::sha256_hex(&broken))
    );

    // The same channel again: the bad file isn't offered again.
    let c = home.cached_channel().unwrap();
    assert!(updates::pending(&home, &c).unwrap().is_empty());
}

#[test]
fn a_channel_cached_halfway_is_read_from_its_temporary_file() {
    let dir = scratch("channel-crash");
    let project = Keypair::generate("Project").unwrap();
    let home = home_with(&dir, &project);
    let (b10, s10) = Channel::new("test", 10, NOW + DAY).sign(&project);
    home.accept_channel(&b10, &s10, NOW).unwrap();

    // `accept_channel` stopped after saving the new signature, before
    // renaming the new channel into place.
    let c11 = Channel::new("test", 11, NOW + DAY);
    let (b11, s11) = c11.sign(&project);
    let path = home.root().join("channel").join("channel.json");
    std::fs::write(path.with_extension("json.tmp"), &b11).unwrap();
    s11.save(&signing::sig_path_for(&path)).unwrap();

    assert_eq!(home.cached_channel().unwrap(), c11);
    assert_eq!(std::fs::read(&path).unwrap(), b11);
}

#[test]
fn old_replayed_expired_and_foreign_channels_are_refused() {
    let dir = scratch("channel");
    let project = Keypair::generate("Project").unwrap();
    let someone = Keypair::generate("Someone").unwrap();
    let home = home_with(&dir, &project);
    let sign = |c: &Channel, k: &Keypair| c.sign(k);

    let c10 = Channel::new("test", 10, NOW + DAY);
    let (b10, s10) = sign(&c10, &project);
    home.accept_channel(&b10, &s10, NOW).unwrap();
    // The same channel again is fine (an unchanged check).
    home.accept_channel(&b10, &s10, NOW).unwrap();
    assert_eq!(home.config().unwrap().channel_serial, 10);

    let (b9, s9) = sign(&Channel::new("test", 9, NOW + DAY), &project);
    let err = home.accept_channel(&b9, &s9, NOW).unwrap_err();
    assert!(format!("{err:#}").contains("roll back"), "{err:#}");

    let (b10b, s10b) = sign(&Channel::new("other", 10, NOW + DAY), &project);
    assert!(
        home.accept_channel(&b10b, &s10b, NOW).is_err(),
        "two channels with one serial"
    );

    let (b11, s11) = sign(&Channel::new("test", 11, NOW + DAY), &project);
    let err = home.accept_channel(&b11, &s11, NOW + 2 * DAY).unwrap_err();
    assert!(format!("{err:#}").contains("expired"), "{err:#}");

    let (b12, s12) = sign(&Channel::new("test", 12, NOW + DAY), &someone);
    assert!(
        home.accept_channel(&b12, &s12, NOW).is_err(),
        "not a project key"
    );
    // The compiled-in project key isn't the test key either.
    assert!(
        Home::new(dir.join("other-home"))
            .accept_channel(&b10, &s10, NOW)
            .is_err()
    );

    assert_eq!(home.config().unwrap().channel_serial, 10);
    assert_eq!(home.cached_channel().unwrap(), c10);

    // A channel listing an unsigned plugin can't be built.
    let dist = dir.join("dist");
    std::fs::create_dir_all(&dist).unwrap();
    std::fs::write(dist.join("a.wasm"), midnight()).unwrap();
    let opts = channel::BuildOptions {
        dir: &dist,
        key: &project,
        name: "test".into(),
        serial: 1,
        expires: NOW + DAY,
        plugin_keys: vec![project.public()],
        community: None,
        keep_community: vec![],
        blocked: vec![],
        framework: None,
        launcher: None,
    };
    assert!(channel::build(&opts).is_err());
}

#[test]
fn the_blocklist_turns_plugins_off_of_either_kind() {
    let dir = scratch("blocklist");
    let project = Keypair::generate("Project").unwrap();
    let home = home_with(&dir, &project);
    let v1 = midnight_at("1.0.0", |_| {});
    home.install(&v1, Some(&project.sign_plugin(&v1))).unwrap();
    home.set_enabled(&id(), true).unwrap();
    let (mut rt, _b) = runtime_for(&home);
    assert!(home.load_into(&mut rt).unwrap()[0].result.is_ok());

    let block = |wasm: &[u8]| Blocked {
        sha256: signing::sha256_hex(wasm),
        reason: "it leaks your messages".into(),
        id: None,
    };
    let loc = publish(&dir.join("dist1"), &project, 1, &[], vec![block(&v1)]);
    let r = updates::check(&home, &loc, NOW).unwrap();
    assert_eq!(r.blocked, vec![(id(), "it leaks your messages".into())]);
    let outcomes = updates::apply(&home, &mut rt, &r);
    assert_eq!(
        outcomes,
        vec![Outcome::Stopped {
            id: id(),
            why: "it leaks your messages".into()
        }]
    );
    assert!(rt.plugin(&id()).is_none());

    // Off at the next start too, offline (from the cached channel), and it
    // can't be turned on or installed again.
    let state = home.get(&id()).unwrap().unwrap().state;
    assert_eq!(
        state,
        InstallState::Blocked("it leaks your messages".into())
    );
    assert!(state.describe().contains("turned off by rekindle"));
    let (mut rt, _b) = runtime_for(&home);
    assert!(home.load_into(&mut rt).unwrap()[0].result.is_err());
    assert!(home.set_enabled(&id(), true).is_err());
    assert!(home.install(&v1, Some(&project.sign_plugin(&v1))).is_err());

    // Third-party, developer mode on: blocked all the same, and never
    // updated from the channel (a changed file needs the user's review).
    home.uninstall(&id(), false).unwrap();
    home.set_developer_mode(true).unwrap();
    let third = midnight_at("1.0.1", |_| {});
    home.install(&third, None).unwrap();
    let newer = midnight_at("5.0.0", |_| {});
    let loc = publish(&dir.join("dist2"), &project, 2, &[&newer], vec![block(&v1)]);
    let r = updates::check(&home, &loc, NOW).unwrap();
    assert!(r.updated.is_empty() && r.blocked.is_empty(), "{r:?}");
    assert_eq!(
        home.get(&id()).unwrap().unwrap().trust.unwrap().trust,
        Trust::ThirdParty
    );
    let loc = publish(&dir.join("dist3"), &project, 3, &[], vec![block(&third)]);
    let r = updates::check(&home, &loc, NOW).unwrap();
    assert_eq!(r.blocked.len(), 1, "{r:?}");
}
