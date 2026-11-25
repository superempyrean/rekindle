//! Calls the 0.9.0, 0.10.0 and 0.11.0 additions on command and logs what they returned. Used by
//! `plugin-host/tests/moderation_history_view.rs`, `http_filter.rs`, `wire_raw.rs` and `ledger.rs`.

use rekindle_sdk::prelude::*;
use rekindle_sdk::view::{MemberField, UserField, Users};

const LAB: u64 = 1000;
const GENERAL: u64 = 1002;
const ALICE: u64 = 101;

struct Probe;

impl Plugin for Probe {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        // Not a gesture: moderation is refused, the rest needs none.
        info!(
            "activate timeout: {:?}",
            moderation::timeout(LAB, ALICE, None, None)
        );
        let _ = events::intercept(&[EventKind::Message]);
        info!(
            "activate intercept-http: {:?}",
            events::intercept_http(&[EventKind::Message, EventKind::Typing, EventKind::Presence])
        );
        for id in [
            "open-dm",
            "invite",
            "timeout",
            "timeout-far",
            "bulk",
            "history",
            "self-name",
            "strip-others",
            "rename-member",
            "view-reset",
            "me-email",
            "self-setting",
            "self-setting-bad",
            "raw",
            "raw-bad",
            "raw-clear",
            "elements",
            "fetch",
            "fetch-off-list",
        ] {
            ui::register_command(id, id);
        }
        Ok(Probe)
    }

    #[expect(clippy::too_many_lines, reason = "one arm per command, each a call or two")]
    fn on_command(&mut self, id: &str) {
        match id {
            "open-dm" => info!("open-dm: {:?}", actions::open_dm(ALICE)),
            "fetch" => info!(
                "fetch: {:?}",
                http::get("https://api.example.com/ok").map(|r| r.status())
            ),
            "fetch-off-list" => info!(
                "fetch-off-list: {:?}",
                http::get("https://evil.example.net/x").map(|r| r.status())
            ),
            "raw" => {
                let gift = rekindle_sdk::wire_raw::selector("composer.gift");
                info!("raw selector: {gift:?}");
                let json = format!(
                    r#"{{"rows": [{{"select": {}, "action": "drop"}}]}}"#,
                    gift.unwrap_or_default()
                );
                info!("raw: {:?}", rekindle_sdk::wire_raw::set_work(&json));
            }
            "raw-bad" => info!(
                "raw-bad: {:?}",
                rekindle_sdk::wire_raw::set_work(
                    r#"{"back": {"plugin": {"plugin": "p", "surface": "s", "element": "e"}}}"#
                )
            ),
            "raw-clear" => rekindle_sdk::wire_raw::clear(),
            "elements" => {
                use rekindle_sdk::elements::{self, Place, PressMode};
                info!("names: {:?}", elements::names());
                info!(
                    "press: {:?}",
                    elements::on_press("composer.gift", &PressMode::Consume)
                );
                info!(
                    "retarget self: {:?}",
                    elements::on_press(
                        "composer.gift",
                        &PressMode::Retarget("composer.gift".into())
                    )
                );
                info!(
                    "insert: {:?}",
                    elements::insert(
                        "hint",
                        "composer.gift",
                        Place::After,
                        &Vec::new().button("go", "Go")
                    )
                );
                info!("missing: {:?}", elements::hide("nope"));
            }
            "invite" => info!("invite: {:?}", actions::create_invite(GENERAL, 0, 0)),
            "timeout" => info!(
                "timeout: {:?}",
                moderation::timeout(LAB, ALICE, Some(now_millis() + 600_000), Some("spam"))
            ),
            "timeout-far" => info!(
                "timeout-far: {:?}",
                moderation::timeout(LAB, ALICE, Some(u64::MAX), None)
            ),
            "bulk" => {
                let ids: Vec<u64> = messages::loaded(GENERAL, None, 2)
                    .iter()
                    .map(|m| m.id)
                    .collect();
                info!("bulk: {:?}", moderation::bulk_delete(GENERAL, &ids));
                info!(
                    "bulk one: {:?}",
                    moderation::bulk_delete(GENERAL, &ids[..1])
                );
            }
            "history" => match history::before(GENERAL, None, 2) {
                Ok(first) => {
                    let next = history::before(GENERAL, first.cursor, 1000);
                    info!(
                        "history: {} then {:?}",
                        first.messages.len(),
                        next.map(|p| (p.messages.len(), p.cursor))
                    );
                }
                Err(e) => info!("history: {e:?}"),
            },
            "self-name" => {
                let r =
                    view::set_users(Users::Me, &[UserField::GlobalName(Some("Override".into()))]);
                info!("self-name: {r:?}");
                info!("me: {:?}", state::me().global_name);
            }
            "strip-others" => {
                let r = view::set_users(
                    Users::Others,
                    &[
                        UserField::Nameplate(None),
                        UserField::AvatarDecoration(None),
                    ],
                );
                info!("strip-others: {r:?}");
                let alice = state::user(ALICE).map(|u| (u.nameplate.is_some(), u.has_decoration));
                info!("alice cosmetics: {alice:?}");
            }
            "rename-member" => {
                let r = view::set_members(
                    Some(LAB),
                    Users::One(ALICE),
                    &[MemberField::Nick(Some("Renamed".into()))],
                );
                info!("rename-member: {r:?}");
                info!(
                    "alice nick: {:?}",
                    users::member(LAB, ALICE).and_then(|m| m.nick)
                );
            }
            "me-email" => info!("me email: {:?}", state::me().email),
            "view-reset" => {
                view::reset();
                info!("me after reset: {:?}", state::me().global_name);
            }
            "self-setting" => {
                let key = "appearance.message-display-compact";
                info!(
                    "self-setting: {:?}",
                    view::set_my_setting(key, Some("true"))
                );
                info!("setting: {:?}", state::my_setting(key));
            }
            "self-setting-bad" => info!(
                "self-setting-bad: {:?}",
                view::set_my_setting("account.email", Some("x"))
            ),
            _ => {}
        }
    }

    fn on_intercept(&mut self, event: &Event) -> events::Verdict {
        match event {
            Event::MessageCreated(m) if m.content.contains("strip") => {
                events::Verdict::Patch(vec![
                    events::EventPatch::Content("cleaned".into()),
                    events::EventPatch::RemoveEmbeds,
                ])
            }
            Event::MessageCreated(m) if m.content.contains("bad") => {
                events::Verdict::Patch(vec![events::EventPatch::Content(String::new())])
            }
            _ => events::Verdict::Pass,
        }
    }

    fn on_request(&mut self, req: &events::HttpRequest) -> events::RequestVerdict {
        let body = req.body.as_deref().unwrap_or("");
        if body.contains("drop me") {
            events::RequestVerdict::Drop
        } else if body.contains("rewrite me") {
            events::RequestVerdict::Replace(r#"{"content":"rewritten"}"#.into())
        } else if body.contains("break me") {
            events::RequestVerdict::Replace("not json".into())
        } else {
            events::RequestVerdict::Pass
        }
    }

    fn on_response(&mut self, res: &events::HttpResponse) -> events::ResponseVerdict {
        info!(
            "response {} {} {} {}",
            res.method, res.path, res.status, res.id
        );
        match &res.body {
            Some(b) if res.method == "GET" && b.contains("secret") => {
                events::ResponseVerdict::Replace("[]".into())
            }
            _ => events::ResponseVerdict::Pass,
        }
    }
}

rekindle_sdk::plugin!(Probe);
