//! End to end, trust for plugins the community index doesn't list: a plugin
//! signed with a project key is first-party (installs off, the user turns it
//! on, nothing to approve); any other is third-party and needs developer
//! mode, a review of that exact file, and a new review whenever the file
//! changes. `community.rs` covers listed plugins.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rekindle_plugin_host::capability::{Capability, Trust};
use rekindle_plugin_host::home::{Home, InstallState, TrustContext, TrustSource};
use rekindle_plugin_host::manifest::{self, Manifest};
use rekindle_plugin_host::review;
use rekindle_plugin_host::runtime::Status;
use rekindle_plugin_host::signing::{self, Keypair};
use rekindle_plugin_host::{MockBackend, Runtime, SharedBackend};

mod common;
use common::{midnight, scratch};

/// The same plugin with a bumped version: different bytes, same id.
fn midnight_v2() -> Vec<u8> {
    let wasm = midnight();
    let mut m = manifest::extract(&wasm).unwrap().unwrap();
    m.version = "9.9.9".into();
    m.capabilities.push(Capability::Clipboard);
    manifest::replace(&wasm, &m).unwrap()
}

fn runtime_for(home: &Home) -> (Runtime, Arc<Mutex<MockBackend>>) {
    let backend = Arc::new(Mutex::new(MockBackend::new()));
    let shared: SharedBackend = backend.clone();
    (
        Runtime::new(shared, home.runtime_config()).unwrap(),
        backend,
    )
}

/// A home whose project key is `project` instead of the compiled-in one.
fn home_with(dir: &Path, project: &Keypair) -> Home {
    Home::new(dir.join("home")).with_trust(TrustContext::with_keys(vec![project.public()]))
}

#[test]
fn first_party_installs_off_and_just_turns_on() {
    let dir = scratch("first-party");
    let project = Keypair::generate("Project").unwrap();
    let home = home_with(&dir, &project);
    let wasm = midnight();
    let id = manifest::extract(&wasm).unwrap().unwrap().id;
    let sig = project.sign_plugin(&wasm);

    let r = home.trust_of(&wasm, Some(&sig)).unwrap();
    assert_eq!(r.trust, Trust::FirstParty);
    assert!(matches!(r.source, TrustSource::ProjectKey { .. }));

    // No developer mode needed, nothing to review; it starts off.
    let (_, record) = home.install(&wasm, Some(&sig)).unwrap();
    assert!(!record.enabled);
    assert_eq!(home.installed().unwrap()[0].state, InstallState::Off);
    let (mut rt, _b) = runtime_for(&home);
    assert!(home.load_into(&mut rt).unwrap()[0].result.is_err());
    assert!(rt.plugins().is_empty());

    home.set_enabled(&id, true).unwrap();
    assert_eq!(home.installed().unwrap()[0].state, InstallState::Ready);
    let (mut rt, _b) = runtime_for(&home);
    let reports = home.load_into(&mut rt).unwrap();
    assert!(reports[0].result.is_ok(), "{:?}", reports[0].result);
    assert_eq!(reports[0].trust, Some(Trust::FirstParty));
    let info = rt.plugin(&id).unwrap();
    assert_eq!(
        (info.status.clone(), info.trust),
        (Status::Active, Trust::FirstParty)
    );
    assert!(info.granted.contains(&Capability::Theming));
    drop(rt);

    // An update signed by the project replaces it with no questions, and
    // stays on.
    let v2 = midnight_v2();
    let (_, record) = home.install(&v2, Some(&project.sign_plugin(&v2))).unwrap();
    assert!(record.enabled);
    assert_eq!(home.installed().unwrap()[0].state, InstallState::Ready);

    // Swapping in an unsigned file by hand: no longer first-party, needs a
    // review, not loaded silently, even with developer mode on.
    home.set_developer_mode(true).unwrap();
    std::fs::write(home.plugin_dir(&id).join("plugin.wasm"), &wasm).unwrap();
    std::fs::remove_file(signing::sig_path_for(
        &home.plugin_dir(&id).join("plugin.wasm"),
    ))
    .unwrap();
    let state = home.installed().unwrap()[0].state.clone();
    assert!(
        matches!(&state, InstallState::NeedsReview(why) if why.contains("first-party when installed")),
        "{state:?}"
    );
    assert!(home.set_enabled(&id, true).is_err());

    assert!(home.uninstall(&id, true).unwrap());
    assert!(home.installed().unwrap().is_empty());
}

#[test]
fn third_party_needs_developer_mode_and_a_review_of_each_file() {
    let dir = scratch("third-party");
    let project = Keypair::generate("Project").unwrap();
    let someone = Keypair::generate("Someone").unwrap();
    let home = home_with(&dir, &project);
    let wasm = midnight();
    let id = manifest::extract(&wasm).unwrap().unwrap().id;

    // Signed by a key that isn't a project key: third-party, like unsigned.
    let sig = someone.sign_plugin(&wasm);
    let r = home.trust_of(&wasm, Some(&sig)).unwrap();
    assert_eq!(r.trust, Trust::ThirdParty);
    assert_eq!(
        r.source,
        TrustSource::OtherKey {
            key_id: someone.key_id()
        }
    );
    assert_eq!(
        home.trust_of(&wasm, None).unwrap().source,
        TrustSource::Unsigned
    );

    // Developer mode is off by default: installing is refused.
    assert!(!home.developer_mode().unwrap());
    let err = home.install(&wasm, None).unwrap_err();
    assert!(format!("{err:#}").contains("developer mode"), "{err:#}");

    // On: it installs (after the review screen, which the CLI shows) and is on.
    home.set_developer_mode(true).unwrap();
    let (_, record) = home.install(&wasm, None).unwrap();
    assert!(record.enabled);
    assert_eq!(record.sha256, signing::sha256_hex(&wasm));
    let (mut rt, _b) = runtime_for(&home);
    let reports = home.load_into(&mut rt).unwrap();
    assert!(reports[0].result.is_ok(), "{:?}", reports[0].result);
    assert_eq!(rt.plugin(&id).unwrap().trust, Trust::ThirdParty);
    drop(rt);

    // Developer mode off: still installed, not loaded.
    home.set_developer_mode(false).unwrap();
    assert_eq!(
        home.installed().unwrap()[0].state,
        InstallState::DeveloperModeOff
    );
    let (mut rt, _b) = runtime_for(&home);
    assert!(home.load_into(&mut rt).unwrap()[0].result.is_err());
    assert!(rt.plugins().is_empty());
    home.set_developer_mode(true).unwrap();
    assert_eq!(home.installed().unwrap()[0].state, InstallState::Ready);

    // A changed file: needs a new review before it loads.
    let v2 = midnight_v2();
    std::fs::write(home.plugin_dir(&id).join("plugin.wasm"), &v2).unwrap();
    let state = home.installed().unwrap()[0].state.clone();
    assert!(
        matches!(&state, InstallState::NeedsReview(why) if why.contains("changed since you reviewed")),
        "{state:?}"
    );
    let (mut rt, _b) = runtime_for(&home);
    assert!(home.load_into(&mut rt).unwrap()[0].result.is_err());

    // The review of the update says what is new.
    let m2 = manifest::extract(&v2).unwrap().unwrap();
    let previous = home.record(&id).unwrap();
    let screen = review::screen(&m2, &home.trust_of(&v2, None).unwrap(), previous.as_ref());
    let text = review::render(&screen);
    assert!(
        text.contains(review::THIRD_PARTY_WARNING) && text.contains("Updating from"),
        "{text}"
    );
    assert!(
        screen
            .abilities
            .iter()
            .any(|a| a.cap == Capability::Clipboard && a.new),
        "{text}"
    );
    home.install(&v2, None).unwrap();
    assert_eq!(home.installed().unwrap()[0].state, InstallState::Ready);

    // Turning it off is remembered across reinstalls.
    home.set_enabled(&id, false).unwrap();
    home.install(&v2, None).unwrap();
    assert_eq!(home.installed().unwrap()[0].state, InstallState::Off);
}

#[test]
fn tampered_wasm_with_signature_is_a_hard_error() {
    let dir = scratch("tamper");
    let project = Keypair::generate("Project").unwrap();
    let home = home_with(&dir, &project);
    let wasm = midnight();
    let sig = project.sign_plugin(&wasm);
    let mut tampered = wasm.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0xff;

    let err = home.trust_of(&tampered, Some(&sig)).unwrap_err();
    assert!(
        format!("{err:#}").contains("modified after signing"),
        "{err:#}"
    );
    // A mismatching signature by an unknown key is not ignored either.
    let other = Keypair::generate("Other").unwrap().sign_plugin(&wasm);
    assert!(home.trust_of(&tampered, Some(&other)).is_err());
    assert!(home.install(&tampered, Some(&sig)).is_err());

    // Tampering with an installed plugin on disk: refused at load.
    home.install(&wasm, Some(&sig)).unwrap();
    let id = manifest::extract(&wasm).unwrap().unwrap().id;
    home.set_enabled(&id, true).unwrap();
    std::fs::write(home.plugin_dir(&id).join("plugin.wasm"), &tampered).unwrap();
    let installed = home.installed().unwrap();
    assert!(
        matches!(installed[0].state, InstallState::Refused(_)),
        "{:?}",
        installed[0].state
    );
    let (mut rt, _b) = runtime_for(&home);
    assert!(home.load_into(&mut rt).unwrap()[0].result.is_err());
}

#[test]
fn the_compiled_in_project_key_is_the_only_first_party_key() {
    let dir = scratch("compiled-in");
    let home = Home::new(dir.join("home"));
    let wasm = midnight();
    let sig = Keypair::generate("Not the project")
        .unwrap()
        .sign_plugin(&wasm);
    assert_eq!(
        home.trust_of(&wasm, Some(&sig)).unwrap().trust,
        Trust::ThirdParty
    );
    assert_eq!(home.trust_of(&wasm, None).unwrap().trust, Trust::ThirdParty);
}

#[test]
fn the_review_screen_leads_with_what_matters() {
    // A manifest-only component: theming + network, imports nothing.
    let empty_component = [0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];
    let m = Manifest {
        schema: manifest::SCHEMA,
        id: "dev.example.fetcher".into(),
        name: "Fetcher".into(),
        version: "0.1.0".into(),
        description: String::new(),
        authors: vec![],
        capabilities: vec![
            Capability::Theming,
            Capability::Network,
            Capability::ReadMessages,
        ],
        network: vec!["api.example.com".into(), "discord.com".into()],
        wit: manifest::rekindle_wit_version().into(),
        settings: vec![],
        strings: BTreeMap::default(),
        compose_uploads: false,
        updates: None,
        theme: false,
    };
    let wasm = manifest::embed(&empty_component, &m).unwrap();
    let dir = scratch("review");
    let home = Home::new(dir.join("home"));
    let r = home.trust_of(&wasm, None).unwrap();
    let text = review::render(&review::screen(&m, &r, None));
    assert!(
        text.contains(
            "It can read your messages and send them to api.example.com and discord.com."
        ),
        "{text}"
    );
    assert!(text.contains(review::DISCORD_WARNING), "{text}");
}
