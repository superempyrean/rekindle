//! End to end against real components: the sandbox's enforcement, and loading.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rekindle_plugin_host::builder::BuildOutput;
use rekindle_plugin_host::manifest;
use rekindle_plugin_host::model::{Color, LogLevel};
use rekindle_plugin_host::runtime::{Status, StorageLocation};
use rekindle_plugin_host::settings::SettingValue;
use rekindle_plugin_host::testing::{self, Harness};
use rekindle_plugin_host::{Capability, ClientBackend, MockBackend, Runtime, RuntimeConfig, Trust};

const MISBEHAVE: &str = "tests/fixtures/misbehave";

fn crate_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// The crate at `rel`, built once per test process.
fn built(rel: &str) -> Arc<BuildOutput> {
    testing::build(crate_path(rel)).unwrap()
}

fn misbehave() -> Harness {
    Harness::for_crate(crate_path(MISBEHAVE)).unwrap()
}

fn faulted(h: &Harness) -> String {
    match h.info().status {
        Status::Faulted(why) => why,
        Status::Active => panic!("expected a fault; logs:\n{}", h.log_text()),
    }
}

#[test]
fn infinite_loop_runs_out_of_fuel_and_is_cleaned_up() {
    let mut h = misbehave();
    assert_eq!(h.color("accent"), Some(Color::rgb(1, 2, 3)));
    let id = h.id().to_string();
    h.runtime().run_command(&id, "spin").unwrap();
    assert!(faulted(&h).contains("out of fuel"));
    // Its theme override and commands are gone.
    assert_ne!(h.color("accent"), Some(Color::rgb(1, 2, 3)));
    assert!(h.commands().is_empty());
    // Further calls are refused rather than re-entering a poisoned instance.
    assert!(h.runtime().run_command(&id, "spin").is_err());
}

#[test]
fn an_event_handler_that_takes_too_long_is_stopped_by_the_clock() {
    let mut h = misbehave();
    let id = h.id().to_string();
    h.runtime().run_command(&id, "spin-events").unwrap();
    let t = std::time::Instant::now();
    h.runtime()
        .dispatch_event(&rekindle_plugin_host::api::Event::Window((true, false)));
    // Stopped by its 50 ms limit, long before 2 billion instructions of fuel.
    let why = faulted(&h);
    assert!(why.contains("took too long handling events"), "{why}");
    assert!(
        t.elapsed() < std::time::Duration::from_millis(500),
        "{:?}",
        t.elapsed()
    );
    assert!(
        h.logged(LogLevel::Error, "took too long handling events"),
        "{}",
        h.log_text()
    );
}

#[test]
fn split_takes_two_to_ten_sendable_parts_and_only_while_sending() {
    use rekindle_plugin_host::api::ComposeKind;
    let mut h = misbehave();
    let out = h.send_message(1002, "split-two");
    assert_eq!(out.parts, ["one", "two"], "{out:?}");
    assert_eq!(out.text, "split-two", "the draft itself is unchanged");
    let refused = |h: &mut Harness, text: &str| {
        h.send_message(1002, text)
            .cancelled
            .map(|(_, why)| why)
            .unwrap_or_default()
    };
    let r = refused(&mut h, "split-one");
    assert!(r.contains("part-count"), "{r:?}");
    assert!(refused(&mut h, "split-eleven").contains("part-count"));
    assert!(refused(&mut h, "split-blank").contains("bad-part"));
    // An edit, and anywhere outside `on-compose`, can't be split.
    let alices = h.with_world(|w| w.messages[&1002][0].id);
    let mut edit = h.reply_draft(1002, alices, "split-two", false);
    edit.reply = None;
    edit.kind = ComposeKind::Edit;
    assert!(
        h.compose(&edit)
            .cancelled
            .is_some_and(|(_, why)| why.contains("not-sending"))
    );
    let id = h.id().to_string();
    h.runtime().run_command(&id, "split-now").unwrap();
    assert!(
        h.logged(
            LogLevel::Info,
            "split outside compose: Err(SplitError { code: 0, name: \"not-sending\""
        ),
        "{}",
        h.log_text()
    );
}

#[test]
fn a_timer_as_far_off_as_a_u64_reaches_never_fires() {
    let mut h = misbehave();
    h.command("far-timers");
    assert!(
        h.logged(LogLevel::Info, "far timers: Ok(()) Ok(())"),
        "{}",
        h.log_text()
    );
    h.runtime().fire_due_timers();
    h.assert_active();
    assert!(!h.logged(LogLevel::Info, "fired"), "{}", h.log_text());
}

#[test]
fn a_command_the_plugin_never_registered_is_refused() {
    let mut h = misbehave();
    let id = h.id().to_string();
    let err = h.runtime().run_command(&id, "unregistered").unwrap_err();
    assert!(err.to_string().contains("has no command"), "{err}");
    h.assert_active();
}

#[test]
fn panic_is_logged_with_message_then_faults() {
    let mut h = misbehave();
    let id = h.id().to_string();
    h.runtime().run_command(&id, "panic").unwrap();
    // The fault reason is the panic message itself, not just "it trapped".
    let why = faulted(&h);
    assert!(
        why.contains("panicked") && why.contains("deliberate panic"),
        "{why}"
    );
    assert!(
        h.logged(LogLevel::Error, "deliberate panic"),
        "{}",
        h.log_text()
    );
}

#[test]
fn memory_is_capped() {
    let mut h = misbehave();
    let id = h.id().to_string();
    h.runtime().run_command(&id, "oom").unwrap();
    // The guest allocator fails to grow memory past the cap and aborts.
    faulted(&h);
}

#[test]
fn storage_quota_is_enforced_without_faulting() {
    let mut h = misbehave();
    h.command("storage-flood");
    h.assert_active();
    assert!(
        h.logged(LogLevel::Info, "quota-exceeded"),
        "{}",
        h.log_text()
    );
    let total: usize = {
        let s = h.runtime().storage("test.rekindle.misbehave").unwrap();
        let s = s.lock().unwrap();
        s.entries().iter().map(|(k, v)| k.len() + v.len()).sum()
    };
    assert!(total <= 1024 * 1024);
}

#[test]
fn log_messages_are_truncated() {
    let mut h = misbehave();
    h.command("log-flood");
    h.assert_active();
    assert!(h.logs().iter().all(|r| r.message.len() <= 8192 + 4));
}

#[test]
fn theming_v2_round_trips_through_the_contract() {
    use rekindle_plugin_host::model::ThemeValue;
    let mut h = misbehave();
    let base_surface = h.color("background-base-lower");
    h.command("theme-v2");
    h.assert_active();
    assert!(
        h.logged(
            LogLevel::Info,
            "theme-v2 results: [true, true, true, true, true, true, true, true, true, true]"
        ),
        "{}",
        h.log_text()
    );
    assert_eq!(h.value("radius-md"), Some(ThemeValue::Length(0.0)));
    assert_eq!(
        h.value("density"),
        Some(ThemeValue::Choice("compact".into()))
    );
    assert_eq!(
        h.value("font-primary"),
        Some(ThemeValue::Font("Fixture Sans".into()))
    );
    assert_eq!(
        h.color("background-base-low"),
        Some(Color::rgb(9, 9, 9)),
        "alias resolved to canonical"
    );
    assert_ne!(
        h.color("background-base-lower"),
        base_surface,
        "tint applied to untouched surfaces"
    );
    assert_eq!(h.backend().fonts().len(), 1);
    h.stop().unwrap();
    assert!(h.backend().fonts().is_empty() && h.backend().tint().is_none());
}

#[test]
fn slot_limits_validation_and_toast_rate_limit() {
    use rekindle_plugin_host::model::SLOT_CHAT_BAR_BUTTON;
    let mut h = misbehave();
    h.command("slots");
    h.assert_active();
    assert!(
        h.logged(
            LogLevel::Info,
            "slots results: [true, true, true, true, true, true, true, true, true]"
        ),
        "{}",
        h.log_text()
    );
    assert_eq!(h.slot_items(SLOT_CHAT_BAR_BUTTON).len(), 25);
    assert_eq!(h.toasts().len(), 3, "rate limited to 3 per window");
    h.stop().unwrap();
    assert!(h.backend().slot_items(SLOT_CHAT_BAR_BUTTON).is_empty());
}

#[test]
fn activate_error_is_reported_and_leaves_nothing_behind() {
    let err = Harness::builder(crate_path(MISBEHAVE))
        .stored("fail-activate", "1")
        .start()
        .err()
        .expect("activation should fail");
    assert!(format!("{err:#}").contains("refusing to start"), "{err:#}");
}

#[test]
fn clipboard_writes_only_during_a_user_gesture() {
    let mut h = misbehave();
    assert!(
        h.logged(LogLevel::Info, "name: \"no-gesture\""),
        "{}",
        h.log_text()
    );
    assert!(h.backend().clipboard().is_none());
    h.command("clipboard");
    assert!(
        h.logged(LogLevel::Info, "command clipboard: Ok(())"),
        "{}",
        h.log_text()
    );
    assert_eq!(h.clipboard().as_deref(), Some("from a gesture"));
}

#[test]
fn an_import_the_manifest_doesnt_request_is_refused_before_any_code_runs() {
    // The code imports `theming`; a manifest edited to leave it out (so a
    // reviewer or the install screen would never see it) is refused.
    let wasm = built(MISBEHAVE).bytes.clone();
    let mut m = manifest::extract(&wasm).unwrap().unwrap();
    m.capabilities.retain(|c| *c != Capability::Theming);
    let edited = manifest::replace(&wasm, &m).unwrap();
    assert_eq!(manifest::extract(&edited).unwrap().unwrap(), m);
    let err = Harness::from_wasm(edited).start().err().expect("refused");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("uses the `theming` interface but doesn't request"),
        "{msg}"
    );
}

#[test]
fn a_faulting_plugin_does_not_disturb_others() {
    let backend = Arc::new(Mutex::new(MockBackend::new()));
    let mut rt = Runtime::new(
        backend.clone(),
        RuntimeConfig {
            storage: StorageLocation::Memory,
            ..Default::default()
        },
    )
    .unwrap();
    let midnight = rt
        .load(
            &built("../../plugins/rust/midnight-theme").bytes,
            Trust::FirstParty,
        )
        .unwrap();
    let bad = rt.load(&built(MISBEHAVE).bytes, Trust::ThirdParty).unwrap();

    // misbehave loaded last, so it owns `accent`; midnight's value is underneath.
    assert_eq!(
        backend.lock().unwrap().override_owner("accent"),
        Some(bad.as_str())
    );
    rt.run_command(&bad, "spin").unwrap();

    let b = backend.lock().unwrap();
    assert_eq!(b.override_owner("accent"), Some(midnight.as_str()));
    assert_eq!(
        b.get_color("background-primary"),
        Some(Color::hex(0x0b0d12))
    );
    drop(b);
    assert_eq!(rt.plugin(&midnight).unwrap().status, Status::Active);
    rt.run_command(&midnight, "next-variant").unwrap();
    assert_eq!(rt.plugin(&midnight).unwrap().status, Status::Active);
}

#[test]
fn actions_only_happen_inside_a_gesture() {
    let mut h = misbehave();
    // From activate: refused, and nothing reached the client.
    assert!(
        h.logged(LogLevel::Info, "activate act:"),
        "{}",
        h.log_text()
    );
    let line = h
        .logs()
        .into_iter()
        .find(|r| r.message.starts_with("activate act:"))
        .unwrap()
        .message;
    assert_eq!(line.matches("no-gesture").count(), 2, "{line}");
    assert!(h.performed().is_empty());
    // From a timer: refused too.
    std::thread::sleep(std::time::Duration::from_millis(80));
    h.runtime().fire_due_timers();
    let line = h
        .logs()
        .into_iter()
        .find(|r| r.message.starts_with("timer act:"))
        .expect("timer fired")
        .message;
    assert!(line.contains("no-gesture"), "{line}");
    assert!(h.performed().is_empty());
}

#[test]
fn your_own_activity_needs_no_gesture_but_is_rate_limited() {
    let mut h = misbehave();
    h.command("presence-later");
    std::thread::sleep(std::time::Duration::from_millis(100));
    h.runtime().fire_due_timers();
    let log = h.log_text();
    let line = h
        .logs()
        .into_iter()
        .find(|r| r.message.starts_with("timer presence:"))
        .unwrap_or_else(|| panic!("timer fired: {log}"))
        .message;
    // Status still needs a click; rich presence doesn't, 5 per 20 s.
    assert!(
        line.contains("no-gesture") && line.ends_with("[true, true, true, true, true, false]"),
        "{line}"
    );
    let performed = h.performed();
    assert_eq!(performed.len(), 5);
    assert!(
        performed.iter().all(|a| matches!(
            a,
            rekindle_plugin_host::model::Action::SetRichPresence { .. }
        )),
        "{performed:?}"
    );
}

#[test]
fn a_gesture_carries_a_budget() {
    let mut h = misbehave();
    h.command("budget");
    // 3 reactions, then refused; 10 local actions, then refused.
    assert!(
        h.logged(
            LogLevel::Info,
            "budget results: [true, true, true, false] 10"
        ),
        "{}",
        h.log_text()
    );
    assert_eq!(
        h.performed().len(),
        10,
        "the mock has no message 1, so only the mark-reads land"
    );
    // The next gesture has a fresh budget.
    h.command("budget");
    assert_eq!(h.performed().len(), 20);
}

#[test]
fn the_render_isolate_cant_write_or_send_urls() {
    use rekindle_plugin_host::api::Transform;
    use rekindle_plugin_host::backend::fixtures::{ALICE, GENERAL};
    let mut h = misbehave();
    let ok = h.receive_message(GENERAL, ALICE, "fine");
    assert!(
        matches!(&h.render_message(GENERAL, ok.id)[..], [Transform::Collapse(r)] if r == "probe")
    );

    // A link out of the isolate loses its target.
    let link = h.receive_message(GENERAL, ALICE, "a link please");
    let t = h.render_message(GENERAL, link.id);
    let [Transform::Append(spans)] = &t[..] else {
        panic!("{t:?}")
    };
    assert!(spans[0].target.is_none(), "URL target stripped");

    // Writing storage from the isolate traps the isolate, not the plugin:
    // the message renders untransformed and nothing was stored.
    let bad = h.receive_message(GENERAL, ALICE, "exfiltrate this");
    assert!(h.render_message(GENERAL, bad.id).is_empty());
    h.assert_active();
    assert!(h.stored("stolen").is_none());
    assert!(
        h.logged(LogLevel::Warn, "render isolate"),
        "{}",
        h.log_text()
    );
    // A new isolate is made on the next render.
    let again = h.receive_message(GENERAL, ALICE, "fine again");
    assert!(!h.render_message(GENERAL, again.id).is_empty());
}

#[test]
fn an_account_whose_storage_cant_open_switches_nothing() {
    let dir = std::env::temp_dir().join(format!(
        "rekindle-enforcement-{}-account",
        std::process::id()
    ));
    // Left over from an earlier run, or absent.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let backend = Arc::new(Mutex::new(MockBackend::new()));
    let config = RuntimeConfig {
        storage: StorageLocation::Dir(dir.clone()),
        ..Default::default()
    };
    let mut rt = Runtime::new(backend, config).unwrap();
    rt.set_account(Some(1)).unwrap();
    let id = rt.load(&built(MISBEHAVE).bytes, Trust::FirstParty).unwrap();
    let storage = rt.storage(&id).unwrap();
    storage.lock().unwrap().set("k", "one").unwrap();
    std::fs::write(dir.join(format!("{id}@2.json")), "not json").unwrap();

    let err = rt.set_account(Some(2)).unwrap_err();
    assert!(
        format!("{err:#}").contains("corrupt storage file"),
        "{err:#}"
    );
    assert_eq!(rt.account(), Some(1));
    assert!(Arc::ptr_eq(&rt.storage(&id).unwrap(), &storage));
    drop(rt);
    // Best effort: a leftover temp directory fails nothing.
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reload_that_fails_to_start_keeps_the_previous_versions_settings() {
    let built = built(MISBEHAVE).bytes.clone();
    let mut m = manifest::extract(&built).unwrap().unwrap();
    m.settings = vec![
        serde_json::from_value(serde_json::json!({
            "key": "keep", "type": "toggle", "label": "Keep", "default": false
        }))
        .unwrap(),
    ];
    let wasm = manifest::replace(&built, &m).unwrap();
    let backend = Arc::new(Mutex::new(MockBackend::new()));
    let mut rt = Runtime::new(backend, RuntimeConfig::default()).unwrap();
    let id = rt.load(&wasm, Trust::FirstParty).unwrap();
    rt.set_setting(&id, "keep", SettingValue::Boolean(true))
        .unwrap();
    rt.storage(&id)
        .unwrap()
        .lock()
        .unwrap()
        .set("fail-activate", "with-audio")
        .unwrap();

    // The new version drops the setting and refuses to start.
    m.version = "9.0.0".into();
    m.settings.clear();
    m.capabilities.push(Capability::Audio);
    let newer = manifest::replace(&built, &m).unwrap();
    rt.reload(&newer, Trust::FirstParty).unwrap_err();
    assert_ne!(rt.plugin(&id).unwrap().version, "9.0.0");
    let page = rt.settings_page(&id).unwrap();
    assert_eq!(page.entries[0].value, SettingValue::Boolean(true));
}

#[test]
fn a_reload_that_fails_to_start_keeps_the_previous_version_running() {
    let wasm = built(MISBEHAVE).bytes.clone();
    let mut m = manifest::extract(&wasm).unwrap().unwrap();
    let backend = Arc::new(Mutex::new(MockBackend::new()));
    let mut rt = Runtime::new(backend.clone(), RuntimeConfig::default()).unwrap();
    let id = rt.load(&wasm, Trust::FirstParty).unwrap();
    rt.storage(&id)
        .unwrap()
        .lock()
        .unwrap()
        .set("fail-activate", "with-audio")
        .unwrap();

    // The new version links fine but refuses to start: the old one is back.
    m.version = "9.0.0".into();
    m.capabilities.push(Capability::Audio);
    let newer = manifest::replace(&wasm, &m).unwrap();
    let err = rt.reload(&newer, Trust::FirstParty).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("previous version is running again") && msg.contains("refusing to start"),
        "{msg}"
    );
    let info = rt.plugin(&id).unwrap();
    assert_eq!(info.status, Status::Active);
    assert_ne!(info.version, "9.0.0");
    assert!(!info.granted.contains(&Capability::Audio));
    assert_eq!(
        backend.lock().unwrap().get_color("accent"),
        Some(Color::rgb(1, 2, 3))
    );
}

#[test]
fn hooks_a_plugin_cant_be_called_through_are_left_out_of_its_build() {
    // No `intercept`, `render-messages`, `compose` or `notifications`.
    let midnight = built("../../plugins/rust/midnight-theme");
    for hook in [
        "event-filter",
        "renderer",
        "compose-handler",
        "notification-filter",
    ] {
        assert!(
            !midnight.inspection.hooks.iter().any(|h| h == hook),
            "{hook} in {:?}",
            midnight.inspection.hooks
        );
    }
    assert!(midnight.inspection.hooks.iter().any(|h| h == "lifecycle"));
    // `intercept` requested: the filter stays, and works.
    let mute = built("../../plugins/rust/mute-words");
    assert!(
        mute.inspection.hooks.iter().any(|h| h == "event-filter"),
        "{:?}",
        mute.inspection.hooks
    );
    let mut h = Harness::from_wasm(mute.bytes.clone()).start().unwrap();
    assert!(h.runtime().plugin("dev.rekindle.mute-words").is_some());
}

#[test]
fn load_many_starts_plugins_in_order_and_reports_each() {
    let backend = Arc::new(Mutex::new(MockBackend::new()));
    let mut rt = Runtime::new(backend, RuntimeConfig::default()).unwrap();
    let m = built("../../plugins/rust/midnight-theme").bytes.clone();
    let results = rt.load_many(&[
        (&m, Trust::FirstParty),
        (b"not wasm", Trust::FirstParty),
        (&m, Trust::FirstParty),
    ]);
    assert_eq!(results[0].as_ref().unwrap(), "dev.rekindle.midnight-theme");
    assert!(results[1].is_err());
    let again = format!("{:#}", results[2].as_ref().unwrap_err());
    assert!(again.contains("already loaded"), "{again}");
}
