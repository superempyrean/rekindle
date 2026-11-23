//! Plugins written in TypeScript, built with `rekindle build`'s
//! script-language path: the example plugins (`plugins/ts/`), held to the
//! same behaviour as their Rust originals, and fixtures (`plugins/fixtures/`)
//! for what the build adds to script plugins: the SDK, the runtime
//! adaptations and the build-time checks. `fixtures/js-runtime` is in plain
//! JavaScript, which builds the same way.
//!
//! They need the toolchain: `npm install` (jco) in `plugins/`. Without it a
//! test prints why and passes; set `REKINDLE_REQUIRE_GUEST_TOOLCHAINS=1` to
//! make that a failure instead (CI).

use std::path::{Path, PathBuf};
use std::process::Command;

use rekindle_plugin_host::api::{Target, Transform, UserRef};
use rekindle_plugin_host::backend::fixtures::{ALICE, CAROL, GENERAL};
use rekindle_plugin_host::builder::{self, BuildOptions};
use rekindle_plugin_host::capability::Capability;
use rekindle_plugin_host::model::{LogLevel, SlotItem, UiValue};
use rekindle_plugin_host::testing::Harness;

mod toolchain;
use toolchain::needs;

fn plugin_dir(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins")
        .join(rel)
}

/// Type-check a TypeScript project (after a build wrote its SDK and
/// declarations); anything else passes.
fn typecheck(dir: &Path) {
    if !dir.join("tsconfig.json").exists() {
        return;
    }
    let tsc = plugin_dir("node_modules/typescript/bin/tsc");
    let out = Command::new("node")
        .arg(tsc)
        .args(["--noEmit", "-p"])
        .arg(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "tsc {}: {}{}",
        dir.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Build `dir` and check what it imports: only `rekindle:plugin@0.10.0`
/// interfaces (no WASI), and of those carrying a capability, exactly `caps`.
/// Returns its hook exports. TypeScript projects are type-checked too.
fn check_imports(dir: &Path, caps: &[Capability]) -> Vec<String> {
    let out = rekindle_plugin_host::testing::build(dir).unwrap();
    for i in &out.inspection.imports {
        assert!(
            i.starts_with("rekindle:plugin/") && i.ends_with("@0.10.0"),
            "foreign import {i}"
        );
    }
    let imported: Vec<Capability> = out.inspection.imported.iter().copied().collect();
    let mut want = caps.to_vec();
    want.sort();
    assert_eq!(imported, want, "imports: {:?}", out.inspection.imports);
    typecheck(dir);
    out.inspection.hooks.clone()
}

/// A caught panic's message.
fn panic_text(e: &Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            e.downcast_ref::<&str>()
                .map(std::string::ToString::to_string)
        })
        .unwrap_or_default()
}

/// Build the plugin at `dir` and return why it failed.
fn build_error(dir: &Path) -> String {
    match builder::build(&BuildOptions {
        path: dir.to_path_buf(),
        release: true,
        quiet: true,
        typecheck: true,
    }) {
        Ok(out) => panic!("{} built, but shouldn't have", out.manifest.id),
        Err(e) => format!("{e:#}"),
    }
}

/// Let real time pass, then fire whatever timers are due.
fn wait_and_fire(h: &mut Harness, ms: u64) {
    std::thread::sleep(std::time::Duration::from_millis(ms));
    h.runtime().fire_due_timers();
}

/// Text Replace, held to the Rust plugin's scenarios, in TypeScript.
mod text_replace_ts {
    use super::*;

    const REL: &str = "ts/text-replace";
    const PLUGIN: &str = "dev.rekindle.text-replace-ts";

    fn text(h: &mut Harness, element: &str, value: &str) {
        h.ui("settings", element, UiValue::Text(value.into()));
    }

    fn add_rule(h: &mut Harness, i: usize, find: &str, replace: &str, regex: bool) {
        h.click("settings", "add");
        text(h, &format!("find-{i}"), find);
        text(h, &format!("replace-{i}"), replace);
        if regex {
            h.toggle("settings", &format!("regex-{i}"), true);
        }
    }

    #[test]
    fn imports_only_what_it_requests() {
        // `compose` puts `compose-control` in a script plugin's world, and the
        // JS engine imports every interface its world offers.
        let hooks = check_imports(
            &needs!(plugin_dir(REL)),
            &[Capability::Compose, Capability::Storage, Capability::Ui],
        );
        assert!(hooks.contains(&"compose-handler".to_string()), "{hooks:?}");
        assert!(!hooks.contains(&"renderer".to_string()), "{hooks:?}");
    }

    #[test]
    fn leaves_messages_alone_with_no_rules() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        let out = h.send_message(1, "hello there");
        assert_eq!(out.text, "hello there");
        assert!(out.rewritten_by.is_empty());
        assert!(h.panel("settings").is_some());
    }

    #[test]
    fn plain_and_regex_rules_apply_in_order() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        add_rule(&mut h, 0, "teh", "the", false);
        add_rule(&mut h, 1, r"(\w+)@example\.com", "<$1 at example>", true);
        let out = h.send_message(1, "teh mail is maple@example.com");
        assert_eq!(out.text, "the mail is <maple at example>");
        assert_eq!(out.rewritten_by, vec![PLUGIN.to_string()]);
        assert!(h.send_message(1, "nothing here").rewritten_by.is_empty());
    }

    #[test]
    fn rules_persist_across_restart_and_can_be_deleted() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        add_rule(&mut h, 0, "brb", "be right back", false);
        h.restart().unwrap();
        assert_eq!(h.send_message(1, "brb").text, "be right back");
        h.click("settings", "delete-0");
        assert_eq!(h.send_message(1, "brb").text, "brb");
    }

    #[test]
    fn invalid_regex_is_flagged_and_skipped() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        add_rule(&mut h, 0, "(unclosed", "x", true);
        let panel = h.panel("settings").unwrap();
        assert!(format!("{:?}", panel.elements).contains("Invalid regex"));
        assert_eq!(h.send_message(1, "(unclosed").text, "(unclosed");
    }

    #[test]
    fn host_refuses_a_rewrite_to_nothing() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        add_rule(&mut h, 0, ".*", "", true);
        let out = h.send_message(1, "keep me");
        assert_eq!(out.text, "keep me");
        assert_eq!(out.rejected.len(), 1);
        assert!(out.rewritten_by.is_empty());
    }

    #[test]
    fn logs_through_the_host() {
        let h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        h.assert_active();
        assert!(!h.logged(LogLevel::Error, ""), "{}", h.log_text());
    }

    #[test]
    fn reads_capture_references_like_the_rust_plugin() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        add_rule(&mut h, 0, r"(\w+)@(\w+)", "${1} at $2 ($$5, [$9])", true);
        assert_eq!(h.send_message(1, "ann@home").text, "ann at home ($5, [])");
    }
}

/// Copy User URLs (a `MenuItem` in the user menu, and the clipboard on its
/// click), held to the Rust plugin's scenarios, in TypeScript.
mod copy_user_urls_ts {
    use super::*;
    use rekindle_plugin_host::MockBackend;
    use rekindle_plugin_host::model::{Icon, SLOT_USER_MENU, ToastKind};

    const REL: &str = "ts/copy-user-urls";

    fn user(id: u64, name: &str) -> Target {
        Target::User((
            UserRef {
                id,
                name: name.into(),
            },
            None,
        ))
    }

    #[test]
    fn adds_a_user_menu_item() {
        let h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        let items = h.slot_items(SLOT_USER_MENU);
        assert!(
            matches!(&items[..], [SlotItem::MenuItem { id, label, icon: Some(Icon::Link), group: Some(g), danger: false, .. }]
                if id == "copy-user-url" && label == "Copy User URL" && g == "copy"),
            "{items:?}"
        );
    }

    #[test]
    fn imports_only_what_it_requests() {
        check_imports(
            &needs!(plugin_dir(REL)),
            &[Capability::Clipboard, Capability::Ui],
        );
    }

    #[test]
    fn copies_the_users_url_on_click() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        // Bigger than 2^53: a u64 id must survive as a bigint, not a float.
        h.click_slot("copy-user-url", user(80351110224678912, "maple"));
        assert_eq!(
            h.clipboard().as_deref(),
            Some("https://discord.com/users/80351110224678912")
        );
        let t = h.toasts().pop().unwrap();
        assert_eq!(
            (t.kind, t.text.as_str()),
            (ToastKind::Success, "Copied maple's URL")
        );
    }

    #[test]
    fn toast_without_a_display_name() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        h.click_slot("copy-user-url", user(5, ""));
        assert_eq!(h.toasts().pop().unwrap().text, "Copied user URL");
    }

    #[test]
    fn a_user_menu_item_is_only_clicked_on_a_user() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        let e = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            h.click_slot("copy-user-url", Target::None);
        }))
        .unwrap_err();
        assert!(
            panic_text(&e).contains("which is clicked on a user, not a none"),
            "{}",
            panic_text(&e)
        );
    }

    #[test]
    fn degrades_without_user_menus() {
        let mut b = MockBackend::new();
        b.set_slot_available(SLOT_USER_MENU, false);
        let h = Harness::builder(needs!(plugin_dir(REL)))
            .backend(b)
            .start()
            .unwrap();
        h.assert_active();
        assert!(h.slot_items(SLOT_USER_MENU).is_empty());
        // The warning reached the host log.
        assert!(
            h.logged(LogLevel::Warn, "aren't available on this client build"),
            "{}",
            h.log_text()
        );
    }
}

/// Read All (state queries, a server-list button and a gesture-bound action),
/// held to the Rust plugin's scenarios, in TypeScript.
mod read_all_ts {
    use super::*;
    use rekindle_plugin_host::backend::ClientBackend;
    use rekindle_plugin_host::model::{Action, SLOT_GUILD_LIST_BUTTON, ToastKind};

    const REL: &str = "ts/read-all";

    fn tooltip(h: &Harness) -> String {
        match &h.slot_items(SLOT_GUILD_LIST_BUTTON)[..] {
            [SlotItem::IconButton { tooltip, .. }] => tooltip.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn imports_only_what_it_requests() {
        check_imports(
            &needs!(plugin_dir(REL)),
            &[Capability::ActLocal, Capability::ReadClient, Capability::Ui],
        );
    }

    #[test]
    fn the_button_marks_everything_read_on_click() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        assert_eq!(tooltip(&h), "Mark 2 channels read");
        h.click_slot("read-all", Target::None);
        assert_eq!(h.performed(), vec![Action::MarkAllRead]);
        assert!(h.backend().unread_channels(10).is_empty());
        assert_eq!(
            h.toasts().last().unwrap().kind,
            ToastKind::Success,
            "{}",
            h.log_text()
        );
        assert_eq!(tooltip(&h), "Nothing unread");
    }

    #[test]
    fn the_palette_command_is_a_gesture_too() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        h.command("read-all");
        assert_eq!(h.performed(), vec![Action::MarkAllRead]);
    }
}

/// Keyword Highlight (the render isolate and the settings schema), held to
/// the Rust plugin's scenarios, in TypeScript.
mod keyword_highlight_ts {
    use super::*;
    use rekindle_plugin_host::settings::SettingValue;

    const REL: &str = "ts/keyword-highlight";

    fn ranges(t: &[Transform]) -> Vec<(u32, u32)> {
        t.iter()
            .flat_map(|t| match t {
                Transform::Highlight(hs) => hs.iter().map(|h| (h.start, h.end)).collect(),
                _ => vec![],
            })
            .collect()
    }

    #[test]
    fn imports_only_what_it_requests() {
        let hooks = check_imports(&needs!(plugin_dir(REL)), &[Capability::RenderMessages]);
        assert!(hooks.contains(&"renderer".to_string()), "{hooks:?}");
        assert!(!hooks.contains(&"compose-handler".to_string()), "{hooks:?}");
    }

    #[test]
    fn declares_its_settings() {
        let h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        assert_eq!(
            h.setting("keywords"),
            Some(SettingValue::Items(vec!["rekindle".into()]))
        );
        assert_eq!(h.setting("tint-rows"), Some(SettingValue::Boolean(false)));
    }

    #[test]
    fn highlights_every_keyword_occurrence() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        let m = h.receive_message(GENERAL, CAROL, "Rekindle and rekindle again");
        assert_eq!(
            ranges(&h.render_message(GENERAL, m.id)),
            vec![(0, 8), (13, 21)],
            "{}",
            h.log_text()
        );
        let plain = h.receive_message(GENERAL, CAROL, "nothing to see");
        assert!(h.render_message(GENERAL, plain.id).is_empty());
        h.assert_active();
    }

    #[test]
    fn ranges_are_utf8_byte_offsets() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        // "é" and "🔥" are 2 and 4 bytes in UTF-8 (1 and 2 UTF-16 units).
        let m = h.receive_message(GENERAL, CAROL, "é🔥 rekindle");
        assert_eq!(ranges(&h.render_message(GENERAL, m.id)), vec![(7, 15)]);
    }

    #[test]
    fn settings_changes_rerender() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        let m = h.receive_message(GENERAL, CAROL, "deploy friday");
        assert!(h.render_message(GENERAL, m.id).is_empty());
        h.set_setting("keywords", SettingValue::Items(vec!["friday".into()]));
        h.set_setting("tint-rows", SettingValue::Boolean(true));
        let t = h.render_message(GENERAL, m.id);
        assert_eq!(ranges(&t), vec![(7, 13)]);
        assert!(matches!(t.last(), Some(Transform::Background(_))), "{t:?}");
    }

    #[test]
    fn renders_without_read_messages() {
        // The render isolate is why: it sees content but can't keep or send it.
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        assert!(!h.info().granted.contains(&Capability::ReadMessages));
        assert!(h.info().hooks.contains(&"renderer".to_string()));
        let m = h.receive_message(GENERAL, CAROL, "rekindle!");
        assert_eq!(ranges(&h.render_message(GENERAL, m.id)), vec![(0, 8)]);
    }
}

mod js_runtime {
    use super::*;

    fn check_prelude(rel: &str) {
        let mut h = Harness::for_crate(needs!(plugin_dir(rel))).unwrap();
        h.assert_active();
        // Top-level console output ran at build time; it's logged on activate.
        assert!(
            h.logged(LogLevel::Info, "hello from the top level"),
            "{}",
            h.log_text()
        );

        h.command("clock");
        assert!(
            h.logged(
                LogLevel::Warn,
                "Date.now() threw: Date.now(): rekindle plugins have no wall clock"
            ),
            "{}",
            h.log_text()
        );
        assert!(h.logged(
            LogLevel::Warn,
            "new Date() threw: new Date(): rekindle plugins have no wall clock"
        ));
        assert!(h.logged(LogLevel::Info, "new Date(0) = 1970-01-01T00:00:00.000Z"));
        assert!(h.logged(LogLevel::Info, "performance.now() = number"));
        h.assert_active();

        h.command("entropy");
        assert!(h.logged(LogLevel::Warn, "crypto.getRandomValues() threw: crypto.getRandomValues(): rekindle plugins have no entropy source"), "{}", h.log_text());
        assert!(
            h.logged(
                LogLevel::Warn,
                "randomUUID() threw: crypto.randomUUID(): rekindle plugins have no entropy source"
            ),
            "{}",
            h.log_text()
        );
        h.assert_active();

        h.command("log-shapes");
        assert!(
            h.logged(
                LogLevel::Info,
                r#"object {"a":[1,2],"big":"7n"} error Error: shown"#
            ),
            "{}",
            h.log_text()
        );

        // Math.random() is reseeded per session (here: per load).
        h.restart().unwrap();
        assert!(
            h.logged(LogLevel::Info, "random differs from last session: true"),
            "{}",
            h.log_text()
        );

        h.command("timers");
        wait_and_fire(&mut h, 80);
        assert!(
            h.logged(LogLevel::Info, "timeout fired for maple"),
            "{}",
            h.log_text()
        );
        assert!(h.logged(LogLevel::Info, "interval tick 1"));
        assert!(!h.logged(LogLevel::Info, "cancelled timeout fired"));
        wait_and_fire(&mut h, 80);
        assert!(h.logged(LogLevel::Info, "interval tick 2"));
        wait_and_fire(&mut h, 80);
        assert!(
            !h.logged(LogLevel::Info, "interval tick 3"),
            "cleared after two"
        );
        // Prelude timers never reach the plugin's own onTimer.
        assert!(
            !h.logged(LogLevel::Info, "plugin timer"),
            "{}",
            h.log_text()
        );

        // A throw is logged with its stack, and the plugin carries on.
        h.command("throw");
        assert!(
            h.logged(
                LogLevel::Error,
                "onCommand threw: Error: boom from onCommand"
            ),
            "{}",
            h.log_text()
        );
        h.assert_active();
        h.command("clock");
    }

    #[test]
    fn quickjs() {
        check_prelude("fixtures/js-runtime");
    }

    #[test]
    fn starlingmonkey() {
        check_prelude("fixtures/js-runtime-sm");
    }

    #[test]
    fn a_misspelt_handler_fails_the_build() {
        let e = build_error(&needs!(plugin_dir("fixtures/ts-misspelt")));
        assert!(
            e.contains("`onComand` is not a rekindle handler; did you mean `onCommand`?"),
            "{e}"
        );
    }

    #[test]
    fn async_handlers_fail_the_build() {
        let e = build_error(&needs!(plugin_dir("fixtures/ts-async")));
        assert!(
            e.contains("`activate` is async, but handlers must be synchronous"),
            "{e}"
        );
    }

    #[test]
    fn an_unrequested_interface_fails_the_build() {
        let e = build_error(&needs!(plugin_dir("fixtures/ts-unrequested")));
        assert!(
            e.contains("uses the `theming` interface but its manifest doesn't request `theming`"),
            "{e}"
        );
    }

    #[test]
    fn a_hook_without_its_capability_fails_the_build() {
        let e = build_error(&needs!(plugin_dir("fixtures/ts-ungated")));
        assert!(
            e.contains(
                "`onCompose` needs the `compose` capability, which the manifest doesn't request"
            ),
            "{e}"
        );
    }

    #[test]
    fn a_node_global_with_host_imports_fails_the_build() {
        let e = build_error(&needs!(plugin_dir("fixtures/ts-node-global")));
        assert!(
            e.contains("imports `jco:node/http@0.1.0`, which rekindle plugins don't get"),
            "{e}"
        );
    }

    #[test]
    fn a_type_error_fails_the_build_unless_unchecked() {
        let dir = needs!(plugin_dir("fixtures/ts-type-error"));
        let e = build_error(&dir);
        assert!(
            e.contains("type errors") && e.contains("src/index.ts(4,7)") && e.contains("TS2322"),
            "{e}"
        );
        builder::build(&BuildOptions {
            path: dir,
            release: true,
            quiet: true,
            typecheck: false,
        })
        .expect("builds unchecked");
    }
}

/// The `rekindle` SDK for TypeScript, through a fixture that uses
/// every area of the API: panels and their callbacks, palette
/// commands, a `MenuItem`, a message click binding, a keybind, a slash command
/// with autocomplete, a settings callback, an event callback, timers, actions
/// on and off a gesture, and the render isolate.
mod sdk_ts {
    use super::*;
    use rekindle_plugin_host::api::{CommandReply, Message, OptionValue};
    use rekindle_plugin_host::model::{Action, ClickArea, Mods, ToastKind};
    use rekindle_plugin_host::settings::SettingValue;

    const REL: &str = "fixtures/ts-sdk";

    #[test]
    fn every_area_of_the_sdk_reaches_the_host() {
        let dir = needs!(plugin_dir(REL));
        let mut h = Harness::for_crate(&dir).unwrap();
        h.assert_active();
        assert!(
            h.logged(LogLevel::Info, "granted 7; t is a number; me is you"),
            "{}",
            h.log_text()
        );
        assert_eq!(
            h.commands(),
            ["copy", "timer", "json", "error"].map(String::from)
        );
        typecheck(&dir);

        panels_commands_and_menus(&mut h);
        let m = events_clicks_and_settings(&mut h);
        keybinds_slash_commands_and_timers(&mut h);
        render_isolate(&mut h, &m);
    }

    fn panels_commands_and_menus(h: &mut Harness) {
        // A text input and a toggle through `onUi`, a button through its callback.
        h.ui("settings", "name", UiValue::Text("maple".into()));
        h.toggle("settings", "on", true);
        h.click("settings", "bump");
        h.click("settings", "bump");
        assert_eq!(h.stored("name").as_deref(), Some("maple"));
        assert_eq!(h.stored("on").as_deref(), Some("true"));
        assert_eq!(h.stored("count").as_deref(), Some("2"));
        assert!(format!("{:?}", h.panel("settings").unwrap().elements).contains("count is 2"));

        h.command("copy");
        assert_eq!(h.clipboard().as_deref(), Some("maple"));
        h.command("json");
        assert!(
            h.logged(LogLevel::Info, r#"json {"a":[1,2]} #00aaff"#),
            "{}",
            h.log_text()
        );
        h.command("error");
        assert!(
            h.logged(LogLevel::Warn, "caught clipboard.writeText too-large"),
            "{}",
            h.log_text()
        );
        h.assert_active();

        h.click_slot(
            "greet",
            Target::User((
                UserRef {
                    id: 7,
                    name: "quill".into(),
                },
                None,
            )),
        );
        let t = h.toasts().pop().unwrap();
        assert_eq!((t.kind, t.text.as_str()), (ToastKind::Success, "hi quill"));
    }

    /// Returns the message it received, for the render isolate.
    fn events_clicks_and_settings(h: &mut Harness) -> Message {
        // The fixture subscribes to message events only.
        let m = h.receive_message(GENERAL, ALICE, "a fixture here");
        assert!(
            h.logged(LogLevel::Info, &format!("message from {}", m.author.name)),
            "{}",
            h.log_text()
        );

        // A message click binding is a gesture: its callback may act as the user.
        let r = h.message_ref(GENERAL, m.id);
        assert_eq!(
            h.message_click(ClickArea::Content, 2, Mods::default(), r)
                .as_deref(),
            Some(h.id())
        );
        assert_eq!(
            h.performed(),
            vec![Action::StartReply {
                message: r,
                mention: false
            }],
            "{}",
            h.log_text()
        );
        h.set_setting("loud", SettingValue::Boolean(true));
        assert!(h.logged(LogLevel::Info, "loud is "), "{}", h.log_text());
        h.message_click(ClickArea::Content, 2, Mods::default(), r);
        assert_eq!(
            h.performed().last(),
            Some(&Action::StartReply {
                message: r,
                mention: true
            })
        );
        m
    }

    fn keybinds_slash_commands_and_timers(h: &mut Harness) {
        h.keybind("shout");
        assert_eq!(h.toasts().pop().unwrap().text, "keybind");
        assert!(
            matches!(h.slash("echo", vec![("text", OptionValue::Text("hi".into()))]), CommandReply::Insert(t) if t == "hi!")
        );
        assert_eq!(
            h.autocomplete("echo", "text", "ab"),
            vec![("ab1".to_string(), "ab1".to_string())]
        );

        // A timer is never a gesture: the action is refused, and caught.
        h.command("timer");
        wait_and_fire(h, 80);
        assert!(
            h.logged(LogLevel::Info, "timer callback t") && h.logged(LogLevel::Info, "timer t"),
            "{}",
            h.log_text()
        );
        assert!(
            h.logged(LogLevel::Warn, "caught localActions.markAllRead no-gesture"),
            "{}",
            h.log_text()
        );
        assert_eq!(h.performed().len(), 2, "nothing done off a gesture");
        h.assert_active();
    }

    /// The isolate reads storage and logs, never activating.
    fn render_isolate(h: &mut Harness, m: &Message) {
        let t = h.render_message(GENERAL, m.id);
        assert!(
            matches!(&t[..], [Transform::Highlight(hs), Transform::Append(spans)]
            if hs[0].start == 2 && hs[0].end == 9 && spans[0].text == "read 2" && spans[0].italic),
            "{t:?}
{}",
            h.log_text()
        );
        assert!(
            h.logged(LogLevel::Info, "rendering 14 chars"),
            "{}",
            h.log_text()
        );
        // A write from the isolate traps it; the message shows untransformed,
        // the main instance carries on, and nothing was stored.
        let w = h.receive_message(GENERAL, CAROL, "please write this down");
        assert!(h.render_message(GENERAL, w.id).is_empty());
        assert!(
            h.logged(LogLevel::Warn, "render isolate"),
            "{}",
            h.log_text()
        );
        assert_eq!(h.stored("leak"), None);
        h.assert_active();
        // A fresh isolate renders the next message.
        let n = h.receive_message(GENERAL, CAROL, "another fixture");
        assert!(
            !h.render_message(GENERAL, n.id).is_empty(),
            "{}",
            h.log_text()
        );
        assert!(h.info().renderer_disabled.is_none());
    }
}

/// Compact Mode, held to the Rust plugin's scenarios, in TypeScript.
mod compact_mode_ts {
    use super::*;
    use rekindle_plugin_host::model::{ThemeValue, ToastKind};

    const REL: &str = "ts/compact-mode";

    fn density(h: &Harness) -> Option<ThemeValue> {
        h.value("density")
    }

    #[test]
    fn imports_only_what_it_requests() {
        check_imports(
            &needs!(plugin_dir(REL)),
            &[Capability::Storage, Capability::Theming, Capability::Ui],
        );
    }

    #[test]
    fn registers_its_command_and_starts_at_the_default() {
        let h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        assert_eq!(h.commands(), vec!["toggle".to_string()]);
        assert_eq!(density(&h), Some(ThemeValue::Choice("default".into())));
    }

    #[test]
    fn toggles_and_says_so() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        h.command("toggle");
        assert_eq!(density(&h), Some(ThemeValue::Choice("compact".into())));
        let t = h.toasts().pop().unwrap();
        assert_eq!(
            (t.kind, t.text.as_str()),
            (ToastKind::Info, "Compact mode on")
        );
        h.command("toggle");
        assert_eq!(density(&h), Some(ThemeValue::Choice("default".into())));
        assert_eq!(h.toasts().pop().unwrap().text, "Compact mode off");
    }

    #[test]
    fn remembers_across_restart() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        h.command("toggle");
        assert_eq!(h.stored("compact").as_deref(), Some("true"));
        h.restart().unwrap();
        assert_eq!(density(&h), Some(ThemeValue::Choice("compact".into())));
    }

    #[test]
    fn unloading_restores_the_clients_density() {
        let mut h = Harness::for_crate(needs!(plugin_dir(REL))).unwrap();
        h.command("toggle");
        h.stop().unwrap();
        assert_eq!(density(&h), Some(ThemeValue::Choice("default".into())));
    }
}
