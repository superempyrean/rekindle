use rekindle_plugin_host::api::{ChannelRef, MessageRef, SelectionChange, UserRef};
use rekindle_plugin_host::model::{Color, UiValue};

use super::bodies::{MAX_HOOK_PAYLOAD, MAX_ITEMS, MAX_TEXT};
use super::*;

fn post(path: &str, body: &str) -> Result<Route, ApiError> {
    route(Method::Post, path, body.as_bytes())
}

fn status(path: &str, body: &str) -> u16 {
    post(path, body).unwrap_err().status
}

#[test]
fn routes_and_methods() {
    assert_eq!(route(Method::Get, "/", b""), Ok(Route::Page));
    assert_eq!(
        route(Method::Get, "/api/state?x=1", b""),
        Ok(Route::Api(Action::State))
    );
    assert_eq!(
        route(Method::Get, "/api/wait?rev=42", b""),
        Ok(Route::Wait { rev: 42 })
    );
    assert_eq!(
        route(Method::Get, "/api/wait", b""),
        Ok(Route::Wait { rev: 0 })
    );
    assert_eq!(
        route(Method::Get, "/api/wait?rev=-1", b"")
            .unwrap_err()
            .status,
        400
    );
    assert_eq!(route(Method::Get, "/api/ui", b"").unwrap_err().status, 405);
    assert_eq!(
        route(Method::Post, "/api/state", b"").unwrap_err().status,
        405
    );
    assert_eq!(
        route(Method::Other, "/api/state", b"").unwrap_err().status,
        405
    );
    assert_eq!(
        route(Method::Get, "/etc/passwd", b"").unwrap_err().status,
        404
    );
    for (path, _) in POSTS {
        assert_eq!(
            route(Method::Get, path, b"").unwrap_err().status,
            405,
            "{path}"
        );
    }
}

#[test]
fn snowflakes_are_numbers_or_digit_strings() {
    let big = u64::MAX - 7;
    assert_eq!(
        post("/api/select", &format!(r#"{{"channel":"{big}"}}"#)),
        Ok(Route::Api(Action::SelectChannel { channel: big }))
    );
    assert_eq!(
        post("/api/select", r#"{"channel":1002}"#),
        Ok(Route::Api(Action::SelectChannel { channel: 1002 }))
    );
    for bad in [
        r#"{"channel":"12a"}"#,
        r#"{"channel":-1}"#,
        r#"{"channel":null}"#,
        r"{}",
    ] {
        assert_eq!(status("/api/select", bad), 400, "{bad}");
    }
}

#[test]
fn send_runs_a_draft_through_compose() {
    assert_eq!(
        post("/api/send", r#"{"channel":1002,"text":"teh draft"}"#),
        Ok(Route::Api(Action::Send {
            channel: 1002,
            text: "teh draft".into()
        }))
    );
    for bad in [
        r#"{"channel":1,"text":"   "}"#,
        r#"{"channel":1}"#,
        r#"{"channel":1,"text":"x","extra":1}"#,
    ] {
        assert_eq!(status("/api/send", bad), 400, "{bad}");
    }
    assert_eq!(
        status(
            "/api/send",
            &format!(r#"{{"channel":1,"text":"{}"}}"#, "a".repeat(MAX_TEXT + 1))
        ),
        400
    );
}

#[test]
fn ui_bodies() {
    let r = post(
        "/api/ui",
        r#"{"plugin":"p","surface":"s","element":"on","value":{"kind":"toggled","value":true}}"#,
    );
    assert_eq!(
        r,
        Ok(Route::Api(Action::Ui {
            plugin: "p".into(),
            event: UiEvent {
                surface: "s".into(),
                element: "on".into(),
                value: UiValue::Toggled(true)
            }
        }))
    );
    // `panel` is still accepted for the surface.
    let r = post(
        "/api/ui",
        r#"{"plugin":"p","panel":"s","element":"go","value":{"kind":"clicked"}}"#,
    )
    .unwrap();
    assert!(matches!(
        r,
        Route::Api(Action::Ui {
            event: UiEvent {
                value: UiValue::Clicked,
                ..
            },
            ..
        })
    ));
    let r = post("/api/ui", r##"{"plugin":"p","surface":"s","element":"c","value":{"kind":"color","value":"#11223380"}}"##).unwrap();
    assert!(matches!(
        r,
        Route::Api(Action::Ui {
            event: UiEvent {
                value: UiValue::Color(Color { a: 0x80, .. }),
                ..
            },
            ..
        })
    ));
    let r = post(
        "/api/ui",
        r#"{"plugin":"p","surface":"message:123","element":"t","value":{"kind":"rows","value":[["a","b"]]}}"#,
    );
    assert!(r.is_ok());
    assert!(post("/api/ui", r#"{"plugin":"p","surface":"settings-page","element":"l","value":{"kind":"items","value":["x"]}}"#).is_ok());

    for bad in [
        "not json",
        r#"{"plugin":"p","surface":"s","element":"x","value":{"kind":"bogus"}}"#,
        r#"{"plugin":"","surface":"s","element":"x","value":{"kind":"clicked"}}"#,
        r#"{"plugin":"p","surface":"s","element":"x","value":{"kind":"color","value":"red"}}"#,
        r#"{"plugin":"p","surface":"s","element":"x","value":{"kind":"clicked"},"extra":1}"#,
        r#"{"plugin":"p","surface":"s","element":"x","value":{"kind":"selected","value":-1}}"#,
    ] {
        assert_eq!(status("/api/ui", bad), 400, "{bad}");
    }
    let long = format!(
        r#"{{"plugin":"p","surface":"s","element":"x","value":{{"kind":"text","value":"{}"}}}}"#,
        "a".repeat(5000)
    );
    assert_eq!(status("/api/ui", &long), 400);
    let many: Vec<String> = (0..=MAX_ITEMS).map(|i| format!("\"{i}\"")).collect();
    let many = format!(
        r#"{{"plugin":"p","surface":"s","element":"x","value":{{"kind":"items","value":[{}]}}}}"#,
        many.join(",")
    );
    assert_eq!(status("/api/ui", &many), 400);
    let huge = "x".repeat(MAX_BODY + 1);
    assert_eq!(status("/api/ui", &huge), 413);
}

#[test]
fn events_take_the_contract_shape() {
    let ev = Event::SelectionChanged(SelectionChange {
        guild: Some(1000),
        channel: Some(1002),
        previous_channel: None,
        message: None,
    });
    let body = serde_json::to_string(&ev).unwrap();
    assert_eq!(
        post("/api/event", &body),
        Ok(Route::Api(Action::Event(Box::new(ev))))
    );
    for ok in [
        Event::MyPresence("dnd".into()),
        Event::Hook(("message.sent".into(), r#"{"n":1}"#.into())),
        Event::Typing((1002, vec![101])),
        Event::Raw(("GUILD_AUDIT_LOG_ENTRY_CREATE".into(), "{}".into())),
        Event::PointerMoved((10.0, 20.0)),
    ] {
        let body = serde_json::to_string(&ok).unwrap();
        assert_eq!(
            post("/api/event", &body),
            Ok(Route::Api(Action::Event(Box::new(ok)))),
            "{body}"
        );
    }
    for bad in [
        Event::MyPresence("asleep".into()),
        Event::Hook(("bad name".into(), "{}".into())),
        Event::Hook(("ok".into(), "{not json".into())),
        Event::Raw(("X".into(), "x".repeat(MAX_HOOK_PAYLOAD + 1))),
    ] {
        let body = serde_json::to_string(&bad).unwrap();
        assert_eq!(status("/api/event", &body), 400, "{body}");
    }
    assert_eq!(status("/api/event", r#"{"Teleport":1}"#), 400);
    assert_eq!(
        status(
            "/api/event",
            r#"{"kind":"presence-changed","status":"dnd"}"#
        ),
        400,
        "the 0.7 shape is gone"
    );
}

#[test]
fn convenience_event_bodies() {
    assert_eq!(
        post("/api/presence", r#"{"status":"idle"}"#),
        Ok(Route::Api(Action::SetPresence {
            status: "idle".into()
        }))
    );
    assert_eq!(status("/api/presence", r#"{"status":"away"}"#), 400);
    assert_eq!(
        post(
            "/api/receive",
            r#"{"channel":1002,"author":101,"content":"hi"}"#
        ),
        Ok(Route::Api(Action::Receive {
            channel: 1002,
            author: 101,
            content: "hi".into()
        }))
    );
    assert_eq!(
        status(
            "/api/receive",
            r#"{"channel":1002,"author":101,"content":" "}"#
        ),
        400
    );
    assert_eq!(
        post(
            "/api/message/edit",
            r#"{"channel":1002,"id":"18446744073709551000","content":"fixed"}"#
        ),
        Ok(Route::Api(Action::EditMessage {
            channel: 1002,
            id: 18446744073709551000,
            content: "fixed".into()
        }))
    );
    assert_eq!(
        status("/api/message/edit", r#"{"channel":1002,"id":5}"#),
        400
    );
    assert_eq!(
        post("/api/message/delete", r#"{"channel":1002,"id":5}"#),
        Ok(Route::Api(Action::DeleteMessage {
            channel: 1002,
            id: 5
        }))
    );
    assert_eq!(
        status(
            "/api/message/delete",
            r#"{"channel":1002,"id":5,"content":"x"}"#
        ),
        400
    );
}

#[test]
fn reload_command_and_keybind_bodies() {
    assert_eq!(
        post("/api/reload", ""),
        Ok(Route::Api(Action::Reload { plugin: None }))
    );
    assert_eq!(
        post("/api/reload", "{}"),
        Ok(Route::Api(Action::Reload { plugin: None }))
    );
    assert_eq!(
        post("/api/reload", r#"{"plugin":"a"}"#),
        Ok(Route::Api(Action::Reload {
            plugin: Some("a".into())
        }))
    );
    assert_eq!(
        post("/api/command", r#"{"plugin":"a","id":"go"}"#),
        Ok(Route::Api(Action::Command {
            plugin: "a".into(),
            id: "go".into()
        }))
    );
    assert_eq!(status("/api/command", r#"{"plugin":"a"}"#), 400);
    assert_eq!(
        post("/api/keybind", r#"{"plugin":"a","id":"k"}"#),
        Ok(Route::Api(Action::Keybind {
            plugin: "a".into(),
            id: "k".into()
        }))
    );
    assert_eq!(status("/api/keybind", r#"{"plugin":"a","id":""}"#), 400);
}

#[test]
fn slot_bodies() {
    let msg = Target::Message(MessageRef {
        id: u64::MAX - 1,
        channel: 1002,
        guild: Some(1000),
        author: 101,
    });
    let body = format!(
        r#"{{"plugin":"p","item":"bm","target":{}}}"#,
        serde_json::to_string(&msg).unwrap()
    );
    assert_eq!(
        post("/api/slot", &body),
        Ok(Route::Api(Action::Slot {
            plugin: "p".into(),
            slot: None,
            item: "bm".into(),
            target: msg,
            mods: Mods::default()
        }))
    );
    let r = post(
        "/api/slot",
        r#"{"plugin":"p","slot":"user-area.button","item":"open","target":"None","mods":{"shift":true,"ctrl":false,"alt":false,"meta":false}}"#,
    );
    assert!(
        matches!(
            r,
            Ok(Route::Api(Action::Slot {
                mods: Mods { shift: true, .. },
                slot: Some(_),
                ..
            }))
        ),
        "{r:?}"
    );
    let user = Target::User((
        UserRef {
            id: 101,
            name: "Ali".into(),
        },
        Some(1000),
    ));
    assert!(
        post(
            "/api/slot",
            &format!(
                r#"{{"plugin":"p","item":"x","target":{}}}"#,
                serde_json::to_string(&user).unwrap()
            )
        )
        .is_ok()
    );
    assert!(
        post(
            "/api/slot",
            r#"{"plugin":"p","item":"x","target":{"Link":"https://example.com/a"}}"#
        )
        .is_ok()
    );

    let long_user = format!(
        r#"{{"plugin":"p","item":"x","target":{{"User":[{{"id":9,"name":"{}"}},null]}}}}"#,
        "a".repeat(101)
    );
    let empty_chan = serde_json::to_string(&Target::Channel(ChannelRef {
        id: 9,
        name: String::new(),
        guild: None,
        kind: 0,
    }))
    .unwrap();
    let empty_chan = format!(r#"{{"plugin":"p","item":"x","target":{empty_chan}}}"#);
    for bad in [
        r#"{"plugin":"p","item":"","target":"None"}"#,
        r#"{"plugin":"p","item":"x"}"#,
        r#"{"plugin":"p","item":"x","target":{"Guild":null}}"#,
        r#"{"plugin":"p","item":"x","target":{"Message":{"id":-1,"channel":2,"guild":null,"author":3}}}"#,
        r#"{"plugin":"p","item":"x","target":{"User":[{"id":9,"name":""},null]}}"#,
        r#"{"plugin":"p","item":"x","target":{"Link":"javascript:alert(1)"}}"#,
        r#"{"plugin":"p","item":"x","target":{"Selection":"  "}}"#,
        r#"{"plugin":"p","item":"x","target":"None","extra":true}"#,
        r#"{"plugin":"p","slot":"title-bar","item":"x","target":"None"}"#,
        &long_user,
        &empty_chan,
    ] {
        assert_eq!(status("/api/slot", bad), 400, "{bad}");
    }
}

#[test]
fn decoration_profile_modal_and_availability_bodies() {
    assert_eq!(
        post(
            "/api/decoration",
            r#"{"plugin":"p","kind":"user.name-badge","key":101}"#
        ),
        Ok(Route::Api(Action::Decoration {
            plugin: "p".into(),
            kind: "user.name-badge".into(),
            key: 101
        }))
    );
    assert_eq!(
        status(
            "/api/decoration",
            r#"{"plugin":"p","kind":"user.hat","key":101}"#
        ),
        400
    );
    assert_eq!(
        post("/api/profile", r#"{"user":{"id":101,"name":"Alice"}}"#),
        Ok(Route::Api(Action::Profile {
            user: UserRef {
                id: 101,
                name: "Alice".into()
            }
        }))
    );
    assert_eq!(
        status("/api/profile", r#"{"user":{"id":101,"name":""}}"#),
        400
    );
    assert_eq!(status("/api/profile", r#"{"user":{"id":101}}"#), 400);
    assert_eq!(
        post("/api/modal/close", r#"{"plugin":"p"}"#),
        Ok(Route::Api(Action::ModalClose { plugin: "p".into() }))
    );
    assert_eq!(status("/api/modal/close", "{}"), 400);
    assert_eq!(
        post(
            "/api/slot-availability",
            r#"{"slot":"chat-bar.button","available":false}"#
        ),
        Ok(Route::Api(Action::SlotAvailability {
            slot: "chat-bar.button".into(),
            available: false
        }))
    );
    assert_eq!(
        status(
            "/api/slot-availability",
            r#"{"slot":"title-bar","available":true}"#
        ),
        400
    );
    assert_eq!(
        status(
            "/api/slot-availability",
            r#"{"slot":"chat-bar.button","available":"no"}"#
        ),
        400
    );
}

#[test]
fn setting_bodies() {
    assert_eq!(
        post(
            "/api/setting",
            r#"{"plugin":"p","key":"on","value":{"kind":"boolean","value":true}}"#
        ),
        Ok(Route::Api(Action::Setting {
            plugin: "p".into(),
            key: "on".into(),
            value: Some(SettingValue::Boolean(true))
        }))
    );
    let ids = post(
        "/api/setting",
        r#"{"plugin":"p","key":"who","value":{"kind":"ids","value":[18446744073709551000]}}"#,
    );
    assert!(
        matches!(ids, Ok(Route::Api(Action::Setting { value: Some(SettingValue::Ids(v)), .. })) if v == [18446744073709551000])
    );
    assert_eq!(
        post("/api/setting/reset", r#"{"plugin":"p","key":"on"}"#),
        Ok(Route::Api(Action::Setting {
            plugin: "p".into(),
            key: "on".into(),
            value: None
        }))
    );
    for bad in [
        r#"{"plugin":"p","key":"on"}"#,
        r#"{"plugin":"p","key":"","value":{"kind":"boolean","value":true}}"#,
        r#"{"plugin":"p","key":"on","value":{"kind":"vibes","value":1}}"#,
    ] {
        assert_eq!(status("/api/setting", bad), 400, "{bad}");
    }
    let long = format!(
        r#"{{"plugin":"p","key":"k","value":{{"kind":"text","value":"{}"}}}}"#,
        "a".repeat(MAX_TEXT + 1)
    );
    assert_eq!(status("/api/setting", &long), 400);
    assert_eq!(status("/api/setting/reset", r#"{"plugin":"p"}"#), 400);
}

#[test]
fn slash_bodies() {
    let r = post(
        "/api/slash",
        r#"{"plugin":"p","command":"roll","options":{"sides":"20"}}"#,
    );
    let options = BTreeMap::from([("sides".to_string(), "20".to_string())]);
    assert_eq!(
        r,
        Ok(Route::Api(Action::Slash {
            plugin: "p".into(),
            command: "roll".into(),
            options,
            channel: None
        }))
    );
    let r = post(
        "/api/slash",
        r#"{"plugin":"p","command":"roll","channel":"9001"}"#,
    );
    assert!(matches!(
        r,
        Ok(Route::Api(Action::Slash {
            channel: Some(9001),
            ..
        }))
    ));
    for bad in [
        r#"{"plugin":"p"}"#,
        r#"{"plugin":"p","command":"roll","options":{"sides":20}}"#,
        r#"{"plugin":"p","command":"roll","options":{"":"x"}}"#,
    ] {
        assert_eq!(status("/api/slash", bad), 400, "{bad}");
    }
}

#[test]
fn origin_guard() {
    assert!(allowed_origin(Some("127.0.0.1:7777"), None, 7777));
    assert!(allowed_origin(
        Some("localhost:7777"),
        Some("http://localhost:7777"),
        7777
    ));
    assert!(!allowed_origin(None, None, 7777));
    assert!(
        !allowed_origin(Some("evil.example:7777"), None, 7777),
        "DNS rebinding"
    );
    assert!(
        !allowed_origin(Some("127.0.0.1:7777"), Some("https://evil.example"), 7777),
        "cross-site"
    );
    assert!(!allowed_origin(Some("127.0.0.1:7778"), None, 7777));
}
