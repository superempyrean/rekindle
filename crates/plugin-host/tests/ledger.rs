//! The ledger against real components: what the probe, Misbehave and
//! Theme Gallery did, what is flagged against their manifests, and the
//! days written to an installed plugin's `ledger.json`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rekindle_plugin_host::backend::MockBackend;
use rekindle_plugin_host::capability::Trust;
use rekindle_plugin_host::runtime::ledger::{self, Days, ElementVerb, Flag};
use rekindle_plugin_host::runtime::{Runtime, RuntimeConfig};
use rekindle_plugin_host::testing::{self, Harness};

const PROBE: &str = "test.rekindle.probe";

fn crate_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// An empty directory for the test called `name`.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rekindle-ledger-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn probe() -> Harness {
    let mut b = MockBackend::new();
    b.set_elements(&["composer.gift", "search.entry"]);
    Harness::builder(crate_path("tests/fixtures/probe"))
        .backend(b)
        .http_get("https://api.example.com/ok", 200, "ok")
        .start()
        .unwrap()
}

#[test]
fn counts_calls_elements_and_hosts() {
    let mut h = probe();
    h.command("elements");
    h.command("fetch");
    h.command("fetch-off-list");
    let a = h.runtime().activity(PROBE).unwrap();
    assert!(a.calls["elements"] >= 5, "{a:?}");
    assert!(a.calls["http"] >= 2, "{a:?}");
    assert_eq!(a.elements[&ElementVerb::Press]["composer.gift"], 2);
    assert_eq!(a.elements[&ElementVerb::Insert]["composer.gift"], 1);
    assert_eq!(a.elements[&ElementVerb::Hide]["nope"], 1);
    assert_eq!(a.requests["api.example.com"], 1);
    assert_eq!(a.refused_hosts["evil.example.net"], 1);
    // The ledger keeps the host, not the URL.
    let json = serde_json::to_string(&a).unwrap();
    assert!(
        !json.contains("https://") && !json.contains("/ok"),
        "{json}"
    );

    let m = h.runtime().manifest(PROBE).unwrap().clone();
    let flags = ledger::flags(&m, &a);
    assert!(flags.contains(&Flag::Host {
        host: "evil.example.net".into(),
        count: 1
    }));
    // The probe declares no elements, so each one it touched is flagged.
    assert!(flags.contains(&Flag::Element {
        verb: ElementVerb::Hide,
        name: "nope".into(),
        count: 1
    }));
    assert!(
        flags
            .iter()
            .all(|f| !matches!(f, Flag::Host { host, .. } if host == "api.example.com"))
    );
}

#[test]
fn an_action_outside_a_gesture_is_flagged() {
    // Misbehave reacts from `activate`, where no gesture is being delivered.
    let mut h = Harness::for_crate(crate_path("tests/fixtures/misbehave")).unwrap();
    let id = h.id().to_string();
    let a = h.runtime().activity(&id).unwrap();
    assert_eq!(a.ungestured["react"], 1, "{a:?}");
    assert!(!a.actions.contains_key("react"));
    let m = h.runtime().manifest(&id).unwrap().clone();
    assert!(ledger::flags(&m, &a).contains(&Flag::NoGesture {
        action: "react".into(),
        count: 1
    }));
}

#[test]
fn a_declared_element_is_not_flagged() {
    let mut b = MockBackend::new();
    b.set_elements(&["composer.gift"]);
    let mut h = Harness::builder(crate_path("../../plugins/rust/declutter"))
        .backend(b)
        .start()
        .unwrap();
    let id = h.id().to_string();
    let a = h.runtime().activity(&id).unwrap();
    assert_eq!(a.elements[&ElementVerb::Hide]["composer.gift"], 1);
    let m = h.runtime().manifest(&id).unwrap().clone();
    assert!(ledger::flags(&m, &a).is_empty());
}

#[test]
fn theme_gallery_requests_are_counted_by_host() {
    const URL: &str =
        "https://www.thecolorapi.com/scheme?hex=5865f2&mode=analogic&count=5&format=json";
    let mut h = Harness::builder(crate_path("../../plugins/rust/theme-gallery"))
        .http_get(URL, 503, "")
        .start()
        .unwrap();
    h.click("gallery", "generate");
    let id = h.id().to_string();
    let a = h.runtime().activity(&id).unwrap();
    assert_eq!(a.requests.get("www.thecolorapi.com"), Some(&1), "{a:?}");
    let m = h.runtime().manifest(&id).unwrap().clone();
    assert!(ledger::flags(&m, &a).is_empty());
}

#[test]
fn an_installed_plugins_days_are_written_and_survive_a_reload() {
    let dir = scratch("ledger-days");
    std::fs::create_dir_all(dir.join(PROBE)).unwrap();
    let mut b = MockBackend::new();
    b.set_elements(&["composer.gift"]);
    let backend = Arc::new(Mutex::new(b));
    let config = RuntimeConfig {
        ledger: Some(dir.clone()),
        ..Default::default()
    };
    let wasm = testing::build(crate_path("tests/fixtures/probe")).unwrap();
    let path = dir.join(PROBE).join(ledger::FILE_NAME);
    {
        let mut rt = Runtime::new(backend.clone(), config).unwrap();
        rt.load(&wasm.bytes, Trust::FirstParty).unwrap();
        rt.run_command(PROBE, "elements").unwrap();
        rt.flush_activity();
        let days = Days::load(&path);
        let today = days.week(ledger::today());
        assert_eq!(today.elements[&ElementVerb::Press]["composer.gift"], 2);

        // A reload keeps the session's counts, and the next flush adds only
        // what came after the last.
        rt.reload(&wasm.bytes, Trust::FirstParty).unwrap();
        rt.run_command(PROBE, "elements").unwrap();
        assert_eq!(
            rt.activity(PROBE).unwrap().elements[&ElementVerb::Press]["composer.gift"],
            4
        );
        // Dropping the runtime writes the rest.
    }
    let week = Days::load(&path).week(ledger::today());
    assert_eq!(week.elements[&ElementVerb::Press]["composer.gift"], 4);
    assert_eq!(Days::load(&path).days.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_plugin_that_is_not_installed_writes_nothing() {
    let dir = scratch("ledger-none");
    let config = RuntimeConfig {
        ledger: Some(dir.clone()),
        ..Default::default()
    };
    let wasm = testing::build(crate_path("tests/fixtures/probe")).unwrap();
    let mut rt = Runtime::new(Arc::new(Mutex::new(MockBackend::new())), config).unwrap();
    rt.load(&wasm.bytes, Trust::FirstParty).unwrap();
    rt.run_command(PROBE, "elements").unwrap();
    rt.flush_activity();
    assert!(!dir.join(PROBE).exists());
    assert!(rt.activity(PROBE).unwrap().calls["elements"] > 0);
    drop(rt);
    let _ = std::fs::remove_dir_all(&dir);
}
