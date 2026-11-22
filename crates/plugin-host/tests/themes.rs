//! Theme files end to end: a `.theme.json` installs, reviews, signs, turns on
//! and off, and loads exactly like a plugin, and runs no code. Trust doesn't
//! change: a third-party theme still needs developer mode, and only a theme
//! file's own bytes make something a theme.

use std::path::Path;
use std::sync::{Arc, Mutex};

use rekindle_plugin_host::capability::{Capability, Trust};
use rekindle_plugin_host::home::{Home, InstallState, TrustContext};
use rekindle_plugin_host::manifest;
use rekindle_plugin_host::model::{Color, LogLevel, ThemeValue};
use rekindle_plugin_host::review;
use rekindle_plugin_host::runtime::Status;
use rekindle_plugin_host::settings::SettingValue;
use rekindle_plugin_host::signing::Keypair;
use rekindle_plugin_host::theme;
use rekindle_plugin_host::{ClientBackend, MockBackend, Runtime, RuntimeConfig, SharedBackend};

mod common;
use common::{midnight, scratch};

const EXAMPLE_ID: &str = "dev.rekindle.midnight-file";
/// The id of the `midnight-theme` plugin, for a theme file standing in for it.
const PLUGIN_ID: &str = "dev.rekindle.midnight-theme";

fn example() -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/themes/midnight.theme.json"),
    )
    .unwrap()
}

/// A minimal theme file.
fn theme_file(id: &str, version: &str, tokens: &str) -> Vec<u8> {
    format!(r#"{{"rekindle-theme": 1, "id": "{id}", "name": "T", "version": "{version}", "tokens": {tokens}}}"#).into_bytes()
}

fn runtime(config: RuntimeConfig) -> (Runtime, Arc<Mutex<MockBackend>>) {
    let backend = Arc::new(Mutex::new(MockBackend::new()));
    let shared: SharedBackend = backend.clone();
    (Runtime::new(shared, config).unwrap(), backend)
}

fn color(hex: u32) -> ThemeValue {
    ThemeValue::Color(Color::hex(hex))
}

fn value(b: &Arc<Mutex<MockBackend>>, token: &str) -> Option<ThemeValue> {
    b.lock().unwrap().get_value(token)
}

fn home_with(dir: &Path, project: &Keypair) -> Home {
    Home::new(dir.join("home")).with_trust(TrustContext::with_keys(vec![project.public()]))
}

#[test]
fn the_example_is_clean() {
    let t = theme::parse(&example()).unwrap();
    assert!(t.warnings.is_empty(), "{:?}", t.warnings);
    assert_eq!(
        t.variants
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>(),
        ["midnight", "abyss", "dusk"]
    );

    // Every token it sets exists on the mock client, in every variant.
    let (mut rt, _) = runtime(RuntimeConfig::default());
    let id = rt.load(&example(), Trust::FirstParty).unwrap();
    for i in 0..3 {
        rt.set_setting(&id, theme::VARIANT_SETTING, SettingValue::Selected(i))
            .unwrap();
    }
    let warned: Vec<_> = rt
        .logs()
        .snapshot()
        .into_iter()
        .filter(|r| r.level == LogLevel::Warn)
        .map(|r| r.message)
        .collect();
    assert!(warned.is_empty(), "{warned:?}");
}

#[test]
fn applies_switches_variants_and_unloads() {
    let (mut rt, b) = runtime(RuntimeConfig::default());
    let default_low = value(&b, "background-base-low");
    let default_strong = value(&b, "text-strong");

    let id = rt.load(&example(), Trust::FirstParty).unwrap();
    assert_eq!(id, EXAMPLE_ID);

    // Listed like a plugin, but with nothing to call.
    let info = rt.plugin(&id).unwrap();
    assert_eq!(info.status, Status::Active);
    assert!(info.hooks.is_empty());
    assert_eq!(
        info.granted.iter().copied().collect::<Vec<_>>(),
        [Capability::Theming]
    );
    assert!(rt.manifest(&id).unwrap().theme);
    assert_eq!(rt.plugins().len(), 1);

    // The top level, and the default variant over it.
    assert_eq!(value(&b, "text-strong"), Some(color(0xf2f3f5)));
    assert_eq!(value(&b, "background-base-low"), Some(color(0x0b0d12)));
    assert_eq!(
        b.lock().unwrap().override_owner("background-base-low"),
        Some(EXAMPLE_ID)
    );

    // The variant is a setting on its generated page.
    let page = rt.settings_page(&id).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].spec.options, ["Midnight", "Abyss", "Dusk"]);
    assert_eq!(page.entries[0].value, SettingValue::Selected(0));

    rt.set_setting(&id, theme::VARIANT_SETTING, SettingValue::Selected(1))
        .unwrap();
    assert_eq!(value(&b, "background-base-low"), Some(color(0x000000)));
    assert_eq!(
        value(&b, "text-strong"),
        Some(color(0xf2f3f5)),
        "the top level stays"
    );
    rt.reset_setting(&id, theme::VARIANT_SETTING).unwrap();
    assert_eq!(value(&b, "background-base-low"), Some(color(0x0b0d12)));

    // Unloading reveals the client's own values.
    rt.unload(&id).unwrap();
    assert_eq!(value(&b, "background-base-low"), default_low);
    assert_eq!(value(&b, "text-strong"), default_strong);
    assert!(rt.plugin(&id).is_none());
    assert!(rt.unload(&id).is_err());
}

#[test]
fn a_variant_switch_resets_what_only_the_old_variant_set() {
    let (mut rt, b) = runtime(RuntimeConfig::default());
    let default_radius = value(&b, "radius-md");
    let file = br#"{"rekindle-theme": 1, "id": "dev.test.radius", "name": "R", "version": "1",
        "variants": [{"name": "square", "tokens": {"radius-md": 0}}, {"name": "plain"}]}"#;
    let id = rt.load(file, Trust::FirstParty).unwrap();
    assert_eq!(value(&b, "radius-md"), Some(ThemeValue::Length(0.0)));
    rt.set_setting(&id, theme::VARIANT_SETTING, SettingValue::Selected(1))
        .unwrap();
    assert_eq!(value(&b, "radius-md"), default_radius);
}

#[test]
fn reload_updates_a_theme_in_place() {
    let (mut rt, b) = runtime(RuntimeConfig::default());
    let default_muted = value(&b, "text-muted");
    let v1 = theme_file(
        "dev.test.reload",
        "1.0.0",
        r##"{"text-muted": "#111111", "text-strong": "#222222"}"##,
    );
    let v2 = theme_file(
        "dev.test.reload",
        "1.1.0",
        r##"{"text-strong": "#333333"}"##,
    );
    let id = rt.load(&v1, Trust::FirstParty).unwrap();
    assert!(rt.load(&v1, Trust::FirstParty).is_err(), "already loaded");

    rt.reload(&v2, Trust::FirstParty).unwrap();
    assert_eq!(rt.plugins().len(), 1);
    assert_eq!(rt.plugin(&id).unwrap().version, "1.1.0");
    assert_eq!(value(&b, "text-strong"), Some(color(0x333333)));
    assert_eq!(
        value(&b, "text-muted"),
        default_muted,
        "dropped by v2, so reset"
    );

    // A broken file is refused before anything running is touched.
    assert!(
        rt.reload(
            b"{\"rekindle-theme\": 1, \"id\": \"dev.test.reload\"}",
            Trust::FirstParty
        )
        .is_err()
    );
    assert_eq!(value(&b, "text-strong"), Some(color(0x333333)));
}

#[test]
fn a_theme_and_a_component_replace_each_other() {
    let (mut rt, b) = runtime(RuntimeConfig::default());
    let wasm = midnight();
    let file = theme_file(
        PLUGIN_ID,
        "2.0.0",
        r##"{"background-base-low": "#123456"}"##,
    );

    rt.load(&wasm, Trust::FirstParty).unwrap();
    assert!(!rt.plugin(PLUGIN_ID).unwrap().hooks.is_empty());
    assert!(
        rt.load(&file, Trust::FirstParty).is_err(),
        "the id is taken"
    );

    rt.reload(&file, Trust::FirstParty).unwrap();
    assert!(
        rt.plugin(PLUGIN_ID).unwrap().hooks.is_empty(),
        "now the theme"
    );
    assert_eq!(rt.plugins().len(), 1);
    assert_eq!(value(&b, "background-base-low"), Some(color(0x123456)));

    rt.reload(&wasm, Trust::FirstParty).unwrap();
    assert!(
        !rt.plugin(PLUGIN_ID).unwrap().hooks.is_empty(),
        "the component again"
    );
    assert_eq!(rt.plugins().len(), 1);
    assert_eq!(
        value(&b, "background-base-low"),
        Some(color(0x0b0d12)),
        "midnight's own value"
    );
}

#[test]
fn a_first_party_theme_installs_off_and_turns_on() {
    let dir = scratch("first-party");
    let project = Keypair::generate("Project").unwrap();
    let home = home_with(&dir, &project);
    let bytes = example();

    let (path, record) = home
        .install(&bytes, Some(&project.sign_plugin(&bytes)))
        .unwrap();
    assert_eq!(path.file_name().unwrap(), theme::FILE_NAME);
    assert_eq!((record.trust, record.enabled), (Trust::FirstParty, false));
    assert_eq!(
        record.capabilities.iter().copied().collect::<Vec<_>>(),
        [Capability::Theming]
    );
    assert_eq!(
        home.get(EXAMPLE_ID).unwrap().unwrap().state,
        InstallState::Off
    );

    home.set_enabled(EXAMPLE_ID, true).unwrap();
    let inst = home.get(EXAMPLE_ID).unwrap().unwrap();
    assert_eq!(inst.state, InstallState::Ready);
    assert_eq!(inst.file, path);

    let (mut rt, b) = runtime(home.runtime_config());
    let reports = home.load_into(&mut rt).unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].result, Ok(()));
    assert_eq!(value(&b, "background-base-low"), Some(color(0x0b0d12)));

    // The variant choice is kept in the home, like any plugin's settings.
    rt.set_setting(
        EXAMPLE_ID,
        theme::VARIANT_SETTING,
        SettingValue::Selected(2),
    )
    .unwrap();
    let (mut rt2, b2) = runtime(home.runtime_config());
    home.load_into(&mut rt2).unwrap();
    assert_eq!(value(&b2, "background-base-low"), Some(color(0x1d1826)));
}

#[test]
fn a_third_party_theme_still_needs_developer_mode_and_review() {
    let dir = scratch("third-party");
    let project = Keypair::generate("Project").unwrap();
    let home = home_with(&dir, &project);
    let bytes = example();

    let err = format!("{:#}", home.install(&bytes, None).unwrap_err());
    assert!(err.contains("developer mode"), "{err}");

    home.set_developer_mode(true).unwrap();
    let (path, record) = home.install(&bytes, None).unwrap();
    assert_eq!((record.trust, record.enabled), (Trust::ThirdParty, true));
    assert_eq!(
        home.get(EXAMPLE_ID).unwrap().unwrap().state,
        InstallState::Ready
    );

    // A changed file isn't what the user reviewed.
    let changed = String::from_utf8(bytes)
        .unwrap()
        .replace("#0b0d12", "#0b0d13");
    std::fs::write(&path, changed).unwrap();
    assert!(matches!(
        home.get(EXAMPLE_ID).unwrap().unwrap().state,
        InstallState::NeedsReview(_)
    ));

    // And with developer mode off again it doesn't load.
    home.install(&example(), None).unwrap();
    home.set_developer_mode(false).unwrap();
    assert_eq!(
        home.get(EXAMPLE_ID).unwrap().unwrap().state,
        InstallState::DeveloperModeOff
    );
}

#[test]
fn the_review_screen_says_it_runs_no_code() {
    let bytes = example();
    let m = manifest::extract(&bytes).unwrap().unwrap();
    let trust = Home::new(scratch("review").to_path_buf())
        .trust_of(&bytes, None)
        .unwrap();
    let screen = review::screen(&m, &trust, None);
    assert!(screen.theme);
    assert!(screen.headline.is_empty(), "nothing serious to lead with");
    assert!(review::render(&screen).contains(review::THEME_NOTE));
    assert_eq!(screen.abilities.len(), 1);
    assert_eq!(screen.abilities[0].cap, Capability::Theming);
    assert!(screen.hosts.is_empty());
}

#[test]
fn a_component_cannot_claim_to_be_a_theme() {
    let wasm = midnight();
    let mut m = manifest::extract(&wasm).unwrap().unwrap();
    assert!(!m.theme);
    m.theme = true;
    m.capabilities = vec![Capability::Theming];
    let disguised = manifest::replace(&wasm, &m).unwrap();

    let err = format!("{:#}", manifest::extract(&disguised).unwrap_err());
    assert!(err.contains("can't say it is a theme"), "{err}");
    let (mut rt, _) = runtime(RuntimeConfig::default());
    assert!(rt.load(&disguised, Trust::FirstParty).is_err());
    let dir = scratch("disguised");
    let home = Home::new(dir.join("home"));
    home.set_developer_mode(true).unwrap();
    assert!(home.install(&disguised, None).is_err());
}

#[test]
fn changing_kind_on_install_leaves_no_stale_file() {
    let dir = scratch("kinds");
    let project = Keypair::generate("Project").unwrap();
    let home = home_with(&dir, &project);
    let wasm = midnight();
    let file = theme_file(
        PLUGIN_ID,
        "2.0.0",
        r##"{"background-base-low": "#123456"}"##,
    );
    let plugin_dir = home.plugin_dir(PLUGIN_ID);

    home.install(&wasm, Some(&project.sign_plugin(&wasm)))
        .unwrap();
    assert!(plugin_dir.join("plugin.wasm").exists());

    home.install(&file, Some(&project.sign_plugin(&file)))
        .unwrap();
    assert!(
        !plugin_dir.join("plugin.wasm").exists() && !plugin_dir.join("plugin.wasm.sig").exists()
    );
    let inst = home.get(PLUGIN_ID).unwrap().unwrap();
    assert!(inst.manifest.unwrap().theme);
    assert_eq!(inst.trust.unwrap().trust, Trust::FirstParty);

    home.install(&wasm, Some(&project.sign_plugin(&wasm)))
        .unwrap();
    assert!(!plugin_dir.join(theme::FILE_NAME).exists());
    assert!(
        !home
            .get(PLUGIN_ID)
            .unwrap()
            .unwrap()
            .manifest
            .unwrap()
            .theme
    );
}

#[test]
fn a_first_party_theme_updates_through_the_release_channel() {
    use rekindle_plugin_host::channel::{self, Location};
    use rekindle_plugin_host::signing;
    use rekindle_plugin_host::updates::{self, Outcome};
    const NOW: u64 = 1_800_000_000;

    let dir = scratch("channel");
    let project = Keypair::generate("Project").unwrap();
    let home = home_with(&dir, &project);
    let id = "dev.test.channel";
    let v1 = theme_file(
        id,
        "1.0.0",
        r##"{"text-strong": "#111111", "text-muted": "#222222"}"##,
    );
    let v2 = theme_file(id, "1.1.0", r##"{"text-strong": "#333333"}"##);
    home.install(&v1, Some(&project.sign_plugin(&v1))).unwrap();
    home.set_enabled(id, true).unwrap();
    let (mut rt, b) = runtime(home.runtime_config());
    let default_muted = value(&b, "text-muted");
    assert!(home.load_into(&mut rt).unwrap()[0].result.is_ok());

    // The project publishes 1.1.0 next to a channel listing it.
    let dist = dir.join("dist");
    std::fs::create_dir_all(&dist).unwrap();
    let path = dist.join("channel-test.theme.json");
    std::fs::write(&path, &v2).unwrap();
    project
        .sign_plugin(&v2)
        .save(&signing::sig_path_for(&path))
        .unwrap();
    let c = channel::build(&channel::BuildOptions {
        dir: &dist,
        key: &project,
        name: "test".into(),
        serial: 1,
        expires: NOW + 14 * 86_400,
        plugin_keys: vec![project.public()],
        community: None,
        keep_community: vec![],
        blocked: vec![],
        framework: None,
        launcher: None,
    })
    .unwrap();
    assert_eq!(c.plugins.len(), 1, "the theme file is listed");
    c.write_signed(&dist.join("channel.json"), &project)
        .unwrap();

    let r = updates::check(&home, &Location::Path(dist.join("channel.json")), NOW).unwrap();
    assert_eq!(r.updated.len(), 1, "{r:?}");
    let inst = home.get(id).unwrap().unwrap();
    assert_eq!(
        (inst.state, inst.manifest.unwrap().version.as_str()),
        (InstallState::Ready, "1.1.0")
    );
    assert_eq!(inst.file.file_name().unwrap(), theme::FILE_NAME);

    // Swapped into the running client: only what 1.1.0 dropped is reset.
    assert_eq!(
        updates::apply(&home, &mut rt, &r),
        vec![Outcome::Updated {
            id: id.into(),
            to: "1.1.0".into()
        }]
    );
    assert_eq!(value(&b, "text-strong"), Some(color(0x333333)));
    assert_eq!(value(&b, "text-muted"), default_muted);
}
