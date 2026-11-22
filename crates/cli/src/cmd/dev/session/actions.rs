//! What the page and the tests can do: each [`Action`], checked against the
//! mock client's world and carried out.

use std::collections::BTreeMap;

use rekindle_plugin_host::ClientBackend;
use rekindle_plugin_host::api::{
    CommandReply, ComposeKind, DeletedMessage, Draft, Event, ReplyInfo, SelectionChange, SlotEvent,
    Target, UserRef,
};
use rekindle_plugin_host::backend::sample_message;
use rekindle_plugin_host::model::{Mods, UiEvent};
use rekindle_plugin_host::runtime::Status;
use rekindle_plugin_host::settings::SettingValue;
use serde_json::{Value, json};

use super::state::excerpt;
use super::world::{
    apply_to_world, check_known_user, check_surface, display_name, find_channel, find_message,
    find_user, next_message_id, now_ms, resolve_slot, slash_invocation,
};
use super::{Session, conflict};
use crate::cmd::dev::api::{Action, ApiError};
use crate::cmd::dev::term;

impl Session {
    pub(super) fn perform(&mut self, action: Action) -> Result<(), ApiError> {
        match action {
            Action::State => Ok(()),
            Action::Reset => self.reset(),
            Action::Ui { plugin, event } => self.ui_event(&plugin, &event),
            Action::Command { plugin, id } => self.run_command(&plugin, &id),
            Action::Event(ev) => self.apply_event(*ev),
            Action::SelectChannel { channel } => self.select_channel(channel),
            Action::SetPresence { status } => self.apply_event(Event::MyPresence(status)),
            Action::Receive {
                channel,
                author,
                content,
            } => self.receive(channel, author, &content),
            Action::EditMessage {
                channel,
                id,
                content,
            } => self.edit_message(channel, id, content),
            Action::DeleteMessage { channel, id } => self.delete_message(channel, id),
            Action::Send { channel, text } => self.send(channel, text, ComposeKind::Send),
            Action::Reload { plugin } => self.reload(plugin.as_deref()),
            Action::Slot {
                plugin,
                slot,
                item,
                target,
                mods,
            } => self.click_slot(&plugin, slot.as_deref(), &item, target, mods),
            Action::Decoration { plugin, kind, key } => self.click_decoration(&plugin, kind, key),
            Action::Profile { user } => self.open_profile(user),
            Action::ModalClose { plugin } => self.close_modal(&plugin),
            Action::SlotAvailability { slot, available } => {
                self.set_slot_available(&slot, available);
                Ok(())
            }
            Action::Setting { plugin, key, value } => self.change_setting(&plugin, &key, value),
            Action::Keybind { plugin, id } => self.press_keybind(&plugin, &id),
            Action::Slash {
                plugin,
                command,
                options,
                channel,
            } => self.run_slash(&plugin, &command, &options, channel),
        }
    }

    fn ui_event(&mut self, plugin: &str, event: &UiEvent) -> Result<(), ApiError> {
        self.require_active(plugin)?;
        check_surface(&self.backend.lock().unwrap(), plugin, event)?;
        // The backend lock is released before the plugin runs (it calls back into it).
        self.runtime.ui_event(plugin, event).map_err(conflict)
    }

    fn run_command(&mut self, plugin: &str, id: &str) -> Result<(), ApiError> {
        self.require_active(plugin)?;
        let known = self
            .backend
            .lock()
            .unwrap()
            .commands()
            .iter()
            .any(|c| c.plugin == plugin && c.id == id);
        if !known {
            return Err(ApiError::new(
                404,
                format!("plugin `{plugin}` has no command `{id}`"),
            ));
        }
        self.note(format!("command: {plugin} / {id}"));
        self.runtime.run_command(plugin, id).map_err(conflict)
    }

    fn press_keybind(&mut self, plugin: &str, id: &str) -> Result<(), ApiError> {
        self.require_active(plugin)?;
        let known = self
            .backend
            .lock()
            .unwrap()
            .keybinds()
            .iter()
            .any(|k| k.plugin == plugin && k.id == id);
        if !known {
            return Err(ApiError::new(
                404,
                format!("plugin `{plugin}` has no keybind `{id}`"),
            ));
        }
        self.note(format!("keybind: {plugin} / {id}"));
        self.runtime.keybind(plugin, id).map_err(conflict)
    }

    /// The user switches channel: `selection-changed`.
    fn select_channel(&mut self, channel: u64) -> Result<(), ApiError> {
        let ev = {
            let b = self.backend.lock().unwrap();
            let ch = find_channel(&b.world, channel)?;
            Event::SelectionChanged(SelectionChange {
                guild: ch.guild,
                channel: Some(channel),
                previous_channel: b.world.selection.channel,
                message: None,
            })
        };
        self.apply_event(ev)
    }

    /// Someone else's message arrives: `message-created`.
    fn receive(&mut self, channel: u64, author: u64, content: &str) -> Result<(), ApiError> {
        let m = {
            let b = self.backend.lock().unwrap();
            let w = &b.world;
            let guild = find_channel(w, channel)?.guild;
            let bot = find_user(w, author)?.bot;
            let name = display_name(w, author, guild);
            let mut m = sample_message(
                channel,
                guild,
                next_message_id(w, channel),
                (author, &name),
                content,
            );
            m.bot = bot;
            m
        };
        self.apply_event(Event::MessageCreated(m))
    }

    /// A loaded message's author edits it: `message-edited`.
    fn edit_message(&mut self, channel: u64, id: u64, content: String) -> Result<(), ApiError> {
        let (m, before) = {
            let b = self.backend.lock().unwrap();
            let mut m = find_message(&b.world, channel, id)?.clone();
            let before = std::mem::replace(&mut m.content, content);
            m.edited_ms = Some(now_ms());
            (m, before)
        };
        self.apply_event(Event::MessageEdited((m, Some(before))))
    }

    /// A loaded message is deleted: `message-deleted`.
    fn delete_message(&mut self, channel: u64, id: u64) -> Result<(), ApiError> {
        let cached = find_message(&self.backend.lock().unwrap().world, channel, id)?.clone();
        self.apply_event(Event::MessageDeleted(DeletedMessage {
            channel,
            id,
            guild: cached.guild,
            cached: Some(cached),
        }))
    }

    /// Rebuild `plugin` (or every plugin) from its source; one without a
    /// source restarts from its last good bytes.
    fn reload(&mut self, plugin: Option<&str>) -> Result<(), ApiError> {
        let targets: Vec<usize> = match plugin {
            None => (0..self.entries.len()).collect(),
            Some(p) => vec![
                self.find(p)
                    .ok_or_else(|| ApiError::new(404, format!("unknown plugin `{p}`")))?,
            ],
        };
        for idx in targets {
            if self.entries[idx].source.is_some() {
                self.start_build(idx, vec!["(forced reload)".into()]);
            } else if let Some(bytes) = self.entries[idx].bytes.clone() {
                // `install` records and prints a load failure itself.
                let _ = self.install(idx, bytes);
            }
        }
        Ok(())
    }

    fn click_slot(
        &mut self,
        plugin: &str,
        slot: Option<&str>,
        item: &str,
        target: Target,
        mods: Mods,
    ) -> Result<(), ApiError> {
        self.require_active(plugin)?;
        let slot = resolve_slot(&self.backend.lock().unwrap(), plugin, slot, item, &target)?;
        self.note(format!(
            "slot: {plugin} / {item} in {slot} on {}",
            describe_target(&target)
        ));
        self.runtime
            .slot_click(plugin, &slot, item, target, mods)
            .map_err(conflict)
    }

    fn click_decoration(&mut self, plugin: &str, kind: String, key: u64) -> Result<(), ApiError> {
        self.require_active(plugin)?;
        let clickable = self
            .backend
            .lock()
            .unwrap()
            .decoration(plugin, &kind, key)
            .is_some_and(|d| d.clickable);
        if !clickable {
            return Err(ApiError::new(
                404,
                format!("plugin `{plugin}` has no clickable `{kind}` decoration on {key}"),
            ));
        }
        self.note(format!("decoration: {plugin} / {kind} on {key}"));
        self.runtime
            .slot_event(plugin, &SlotEvent::DecorationClicked((kind, key)))
            .map_err(conflict)
    }

    fn open_profile(&mut self, user: UserRef) -> Result<(), ApiError> {
        check_known_user(&self.backend.lock().unwrap().world, &user).map_err(ApiError::bad)?;
        self.note(format!("profile opened: {} ({})", user.name, user.id));
        self.runtime.open_profile(&user);
        self.profile = Some(user);
        Ok(())
    }

    /// The user dismisses a plugin's modal; the plugin isn't called.
    fn close_modal(&mut self, plugin: &str) -> Result<(), ApiError> {
        let mut b = self.backend.lock().unwrap();
        if b.modal(plugin).is_none() {
            return Err(ApiError::new(
                404,
                format!("plugin `{plugin}` has no open modal"),
            ));
        }
        b.close_modal(plugin);
        Ok(())
    }

    /// Pretend this client build does (or doesn't) have a slot's hook point.
    fn set_slot_available(&mut self, slot: &str, available: bool) {
        self.backend
            .lock()
            .unwrap()
            .set_slot_available(slot, available);
        if available {
            self.note(format!(
                "slot {slot} is available again; reload plugins to re-register their items"
            ));
        } else {
            term::warn(&format!(
                "slot {slot} is now unavailable (its items were removed)"
            ));
            self.note(format!("slot {slot} is now unavailable"));
        }
        // What resolved on this build changed: plugins hear about it.
        self.runtime.features_changed();
    }

    /// The user changes (`Some`) or resets (`None`) a setting on a plugin's
    /// generated settings page.
    fn change_setting(
        &mut self,
        plugin: &str,
        key: &str,
        value: Option<SettingValue>,
    ) -> Result<(), ApiError> {
        let page = self
            .runtime
            .settings_page(plugin)
            .ok_or_else(|| ApiError::new(404, format!("plugin `{plugin}` is not loaded")))?;
        let entry = page
            .entries
            .iter()
            .find(|e| e.spec.key == key)
            .ok_or_else(|| {
                ApiError::new(404, format!("plugin `{plugin}` has no setting `{key}`"))
            })?;
        if !entry.visible || !entry.enabled {
            return Err(ApiError::new(
                409,
                format!(
                    "setting `{key}` is {} right now",
                    if entry.visible { "disabled" } else { "hidden" }
                ),
            ));
        }
        let changed = if let Some(v) = value {
            self.note(format!(
                "setting: {plugin} / {key} = {}",
                serde_json::to_string(&v).unwrap_or_default()
            ));
            self.runtime.set_setting(plugin, key, v)
        } else {
            self.note(format!("setting: {plugin} / {key} reset"));
            self.runtime.reset_setting(plugin, key)
        };
        changed.map_err(|e| ApiError::bad(format!("{e:#}")))
    }

    /// The user runs a plugin's slash command, its options typed as text.
    fn run_slash(
        &mut self,
        plugin: &str,
        command: &str,
        options: &BTreeMap<String, String>,
        channel: Option<u64>,
    ) -> Result<(), ApiError> {
        self.require_active(plugin)?;
        let inv = slash_invocation(
            &self.backend.lock().unwrap(),
            plugin,
            command,
            options,
            channel,
        )?;
        self.note(format!(
            "slash: /{command} ({plugin}) in {}",
            inv.channel.id
        ));
        let reply = self.runtime.slash_command(plugin, &inv).map_err(conflict)?;
        self.last_slash = Some(json!({ "plugin": plugin, "command": command, "reply": reply }));
        match reply {
            CommandReply::Insert(text) => {
                self.backend.lock().unwrap().world.composer = text;
                Ok(())
            }
            CommandReply::Send(text) => self.send(inv.channel.id, text, ComposeKind::Command),
            CommandReply::None | CommandReply::Local(_) => Ok(()),
        }
    }

    /// Run an event past the plugins' `event-filter`s, apply its effect on
    /// the world (a message arrives, the selection moves), then deliver it.
    /// A dropped event changes nothing.
    fn apply_event(&mut self, ev: Event) -> Result<(), ApiError> {
        let described = describe_event(&ev);
        let Some(ev) = self.runtime.intercept(ev) else {
            self.note(format!("event dropped by a plugin: {described}"));
            return Ok(());
        };
        apply_to_world(&mut self.backend.lock().unwrap().world, &ev).map_err(ApiError::bad)?;
        self.note(format!("event: {}", describe_event(&ev)));
        self.runtime.dispatch_event(&ev);
        Ok(())
    }

    /// The user sends `text` in `channel` (or saves the edit the composer is
    /// in, or replies): through `compose` plugins, then the server's copy
    /// arrives as `message-created` / `message-edited`.
    fn send(&mut self, channel: u64, text: String, kind: ComposeKind) -> Result<(), ApiError> {
        let draft = {
            let b = self.backend.lock().unwrap();
            let w = &b.world;
            let guild = find_channel(w, channel)?.guild;
            let typed = kind != ComposeKind::Command;
            let editing = w.editing.filter(|r| typed && r.channel == channel);
            let reply = w
                .pending_reply
                .filter(|(r, _)| typed && editing.is_none() && r.channel == channel)
                .map(|(r, mention)| ReplyInfo {
                    message: r,
                    author: UserRef {
                        id: r.author,
                        name: display_name(w, r.author, r.guild),
                    },
                    mention,
                });
            Draft {
                channel,
                guild,
                kind: if editing.is_some() {
                    ComposeKind::Edit
                } else {
                    kind
                },
                editing,
                text,
                reply,
                attachments: vec![],
                sticker_count: 0,
                silent: false,
            }
        };
        let c = self.runtime.compose(&draft);
        let verb = if draft.editing.is_some() {
            "saved edit"
        } else {
            "sent"
        };
        if !c.parts.is_empty() {
            self.note(format!(
                "{verb} as {} messages (split by {})",
                c.parts.len(),
                c.rewritten_by.join(", ")
            ));
        } else if c.rewritten_by.is_empty() {
            self.note(format!("{verb}: {:?}", c.text));
        } else {
            self.note(format!(
                "{verb}: {:?} (rewritten by {})",
                c.text,
                c.rewritten_by.join(", ")
            ));
        }
        for (plugin, why) in &c.rejected {
            self.note(format!("refused {plugin}'s rewrite: {why}"));
        }
        if let Some((plugin, why)) = &c.cancelled {
            self.note(format!("{plugin} cancelled the send: {why}"));
        }
        self.last_send = Some(json!({
            "channel": channel,
            "draft": draft.text,
            "text": c.text,
            "parts": c.parts,
            "kind": draft.kind,
            "rewritten_by": c.rewritten_by,
            "rejected": c.rejected.iter().map(|(p, why)| json!({ "plugin": p, "reason": why })).collect::<Vec<_>>(),
            "cancelled": c.cancelled.as_ref().map(|(p, why)| json!({ "plugin": p, "reason": why })),
            "mention": c.mention,
            "silent": c.silent,
        }));
        if c.cancelled.is_some() {
            // The client keeps the draft in the composer.
            self.backend.lock().unwrap().world.composer = draft.text;
            return Ok(());
        }
        // A split send goes as one message per part, the reply on the first.
        let texts = if c.parts.is_empty() || draft.editing.is_some() {
            vec![c.text.clone()]
        } else {
            c.parts.clone()
        };
        for (i, text) in texts.into_iter().enumerate() {
            let reply = if i == 0 { draft.reply.as_ref() } else { None };
            self.post_composed(&draft, text, c.mention, reply)?;
        }
        Ok(())
    }

    /// Post one composed message (or save an edit) to the simulated client.
    fn post_composed(
        &mut self,
        draft: &Draft,
        text: String,
        mention: Option<bool>,
        reply: Option<&ReplyInfo>,
    ) -> Result<(), ApiError> {
        let channel = draft.channel;
        let ev = {
            let mut b = self.backend.lock().unwrap();
            let w = &mut b.world;
            w.composer.clear();
            if let Some(r) = draft.editing {
                w.editing = None;
                let mut m = find_message(w, r.channel, r.id)?.clone();
                let before = std::mem::replace(&mut m.content, text);
                m.edited_ms = Some(now_ms());
                Event::MessageEdited((m, Some(before)))
            } else {
                let me = w.me;
                let name = display_name(w, me, draft.guild);
                let mut m = sample_message(
                    channel,
                    draft.guild,
                    next_message_id(w, channel),
                    (me, &name),
                    &text,
                );
                if let Some(reply) = reply {
                    w.pending_reply = None;
                    m.reply_to = Some(reply.message);
                    m.kind = 19;
                    if mention.unwrap_or(reply.mention) {
                        m.mentions = vec![reply.author.id];
                    }
                }
                Event::MessageCreated(m)
            }
        };
        self.apply_event(ev)
    }

    fn require_active(&self, plugin: &str) -> Result<(), ApiError> {
        match self.runtime.plugin(plugin) {
            None => Err(ApiError::new(
                404,
                format!("plugin `{plugin}` is not loaded"),
            )),
            Some(p) => match p.status {
                Status::Active => Ok(()),
                Status::Faulted(why) => Err(ApiError::new(
                    409,
                    format!("plugin `{plugin}` has faulted: {why}"),
                )),
            },
        }
    }
}

/// A target, for the activity log.
fn describe_target(t: &Target) -> String {
    match t {
        Target::None => "(no target)".into(),
        Target::Message(m) => format!("message {} in {} by {}", m.id, m.channel, m.author),
        Target::User((u, _)) => format!("user {} ({})", u.name, u.id),
        Target::Channel(c) => format!("channel #{} ({})", c.name, c.id),
        Target::Guild(g) => format!("guild {} ({})", g.name, g.id),
        Target::Role(r) => format!("role {} ({})", r.name, r.id),
        Target::Media(m) => format!("media {}", m.url),
        Target::Link(url) => format!("link {url}"),
        Target::Selection(s) => format!("selection {:?}", excerpt(s, 40)),
    }
}

/// An event's case name, as the WIT spells it (`selection-changed`).
fn event_name(ev: &Event) -> String {
    let name = match serde_json::to_value(ev) {
        Ok(Value::String(s)) => s,
        Ok(Value::Object(m)) => m.keys().next().cloned().unwrap_or_default(),
        _ => String::new(),
    };
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// An event, for the activity log: its name and what it is about.
fn describe_event(ev: &Event) -> String {
    let opt = |v: Option<u64>| v.map_or_else(|| "none".to_string(), |v| v.to_string());
    let detail = match ev {
        Event::SelectionChanged(s) => {
            format!("channel {} (guild {})", opt(s.channel), opt(s.guild))
        }
        Event::MyPresence(s) => s.clone(),
        Event::Hook((name, payload)) => format!("{name} ({} bytes)", payload.len()),
        Event::Raw((name, _)) => name.clone(),
        Event::PointerMoved((x, y)) => format!("{x:.0},{y:.0}"),
        Event::MessageCreated(m) | Event::MessageEdited((m, _)) => {
            format!("{} in {} by {}", m.id, m.channel, m.author.name)
        }
        Event::MessageDeleted(d) => format!(
            "{} in {}{}",
            d.id,
            d.channel,
            if d.cached.is_some() { " (cached)" } else { "" }
        ),
        Event::Typing((channel, users)) => format!("in {channel}: {users:?}"),
        Event::Presence(p) => format!("{} is {}", p.user, p.status),
        Event::Reaction(r) => format!(
            "{}{} on {} by {}",
            if r.added { "+" } else { "-" },
            r.emoji,
            r.message.id,
            r.user
        ),
        _ => String::new(),
    };
    let name = event_name(ev);
    if detail.is_empty() {
        name
    } else {
        format!("{name} {detail}")
    }
}

#[cfg(test)]
mod tests {
    use rekindle_plugin_host::api as papi;
    use rekindle_plugin_host::backend::fixtures::{
        ALICE, BOT, DM_ALICE, GAMERS, GENERAL, LAB, ME, OFF_TOPIC,
    };
    use rekindle_plugin_host::model::{ALL_SLOTS, SLOT_CHAT_BAR_BUTTON};

    use super::super::fixtures::{general_messages, session, world};
    use super::super::world::message_ref;
    use super::*;

    #[test]
    fn selecting_a_channel_moves_the_selection() {
        let mut s = session();
        s.act(Action::SelectChannel { channel: DM_ALICE }).unwrap();
        assert_eq!(
            world(&s, |w| (w.selection.channel, w.selection.guild)),
            (Some(DM_ALICE), None)
        );
        assert_eq!(s.state(1)["client"]["rows"].as_array().unwrap().len(), 1);
        assert_eq!(
            s.act(Action::SelectChannel { channel: 424242 })
                .unwrap_err()
                .status,
            404
        );
        let category = world(&s, |w| w.channels.iter().find(|c| c.kind == 4).unwrap().id);
        assert_eq!(
            s.act(Action::SelectChannel { channel: category })
                .unwrap_err()
                .status,
            400
        );
        assert!(
            s.activity
                .back()
                .unwrap()
                .1
                .starts_with("event: selection-changed"),
            "{:?}",
            s.activity
        );
    }

    #[test]
    fn messages_arrive_change_and_go() {
        let mut s = session();
        let n = general_messages(&s).len();
        s.act(Action::Receive {
            channel: GENERAL,
            author: ALICE,
            content: "new!".into(),
        })
        .unwrap();
        let msgs = general_messages(&s);
        assert_eq!(msgs.len(), n + 1);
        let new = msgs.last().unwrap();
        assert_eq!(
            (
                new.author.id,
                new.author.name.as_str(),
                new.content.as_str()
            ),
            (ALICE, "Ali", "new!"),
            "her nickname in the lab"
        );
        assert!(new.id > msgs[n - 1].id);
        assert!(world(&s, |w| w.visible.iter().any(|r| r.id == new.id)));
        assert_eq!(
            s.act(Action::Receive {
                channel: GENERAL,
                author: 5,
                content: "x".into()
            })
            .unwrap_err()
            .status,
            404
        );

        s.act(Action::EditMessage {
            channel: GENERAL,
            id: new.id,
            content: "edited".into(),
        })
        .unwrap();
        let edited = general_messages(&s).last().unwrap().clone();
        assert_eq!(edited.content, "edited");
        assert!(edited.edited_ms.is_some());
        s.act(Action::DeleteMessage {
            channel: GENERAL,
            id: new.id,
        })
        .unwrap();
        assert_eq!(general_messages(&s).len(), n);
        assert_eq!(
            s.act(Action::DeleteMessage {
                channel: GENERAL,
                id: new.id
            })
            .unwrap_err()
            .status,
            404
        );
    }

    #[test]
    fn a_send_goes_through_compose_and_arrives() {
        let mut s = session();
        s.backend.lock().unwrap().world.composer = "typed".into();
        s.act(Action::Send {
            channel: GENERAL,
            text: "hello".into(),
        })
        .unwrap();
        let sent = general_messages(&s).last().unwrap().clone();
        assert_eq!((sent.author.id, sent.content.as_str()), (ME, "hello"));
        assert_eq!(s.last_send.as_ref().unwrap()["text"], "hello");
        assert!(world(&s, |w| w.composer.is_empty()));
        assert_eq!(
            s.act(Action::Send {
                channel: 1,
                text: "x".into()
            })
            .unwrap_err()
            .status,
            404
        );
    }

    #[test]
    fn sends_follow_the_composer_reply_and_edit() {
        let mut s = session();
        let msgs = general_messages(&s);
        let hello = message_ref(&msgs[0]);
        s.backend.lock().unwrap().world.pending_reply = Some((hello, true));
        s.act(Action::Send {
            channel: GENERAL,
            text: "re".into(),
        })
        .unwrap();
        let reply = general_messages(&s).last().unwrap().clone();
        assert_eq!(reply.reply_to, Some(hello));
        assert_eq!(reply.mentions, vec![ALICE]);
        assert!(world(&s, |w| w.pending_reply.is_none()));

        let mine = message_ref(&reply);
        s.backend.lock().unwrap().world.editing = Some(mine);
        let n = general_messages(&s).len();
        s.act(Action::Send {
            channel: GENERAL,
            text: "re (fixed)".into(),
        })
        .unwrap();
        let after = general_messages(&s);
        assert_eq!(after.len(), n, "an edit, not a new message");
        assert_eq!(after.last().unwrap().content, "re (fixed)");
        assert_eq!(
            s.last_send.as_ref().unwrap()["kind"],
            json!(ComposeKind::Edit)
        );
    }

    #[test]
    fn posted_events_are_checked_against_the_world() {
        let mut s = session();
        let m = sample_message(GENERAL, Some(LAB), 77, (ALICE, "Ali"), "hi");
        assert!(
            s.act(Action::Event(Box::new(Event::MessageCreated(m.clone()))))
                .is_ok()
        );
        assert_eq!(
            s.act(Action::Event(Box::new(Event::MessageCreated(m.clone()))))
                .unwrap_err()
                .status,
            400,
            "already loaded"
        );
        let elsewhere = sample_message(GENERAL, Some(GAMERS), 78, (ALICE, "Ali"), "hi");
        assert_eq!(
            s.act(Action::Event(Box::new(Event::MessageCreated(elsewhere))))
                .unwrap_err()
                .status,
            400,
            "wrong guild"
        );
        let stranger = sample_message(GENERAL, Some(LAB), 79, (4242, "?"), "hi");
        assert_eq!(
            s.act(Action::Event(Box::new(Event::MessageCreated(stranger))))
                .unwrap_err()
                .status,
            400
        );
        let unloaded = sample_message(OFF_TOPIC, Some(LAB), 80, (ALICE, "Ali"), "x");
        assert_eq!(
            s.act(Action::Event(Box::new(Event::MessageEdited((
                unloaded, None
            )))))
            .unwrap_err()
            .status,
            400
        );
        s.act(Action::Event(Box::new(Event::Typing((
            GENERAL,
            vec![ALICE, BOT],
        )))))
        .unwrap();
        assert_eq!(s.state(1)["client"]["typing"], json!(["Ali", "helperbot"]));
        assert_eq!(
            s.act(Action::Event(Box::new(Event::Typing((GENERAL, vec![9])))))
                .unwrap_err()
                .status,
            400
        );
        // Events with no world effect are delivered as posted.
        s.act(Action::Event(Box::new(Event::Hook((
            "x".into(),
            "{}".into(),
        )))))
        .unwrap();
        assert_eq!(s.activity.back().unwrap().1, "event: hook x (2 bytes)");
        s.act(Action::Event(Box::new(Event::MyVoiceLeft))).unwrap();
        assert_eq!(s.activity.back().unwrap().1, "event: my-voice-left");
    }

    #[test]
    fn slot_actions_are_validated() {
        let mut s = session();
        let st = s.state(1);
        assert_eq!(
            st["slots"]["available"].as_array().unwrap().len(),
            ALL_SLOTS.len()
        );
        assert!(st["slots"]["profile"].is_null());
        assert_eq!(
            st["slots"]["targets"][SLOT_CHAT_BAR_BUTTON]["facts"],
            json!(["in-guild", "can-manage-messages"])
        );

        let msg = Target::Message(message_ref(&general_messages(&s)[0]));
        let click = Action::Slot {
            plugin: "nope".into(),
            slot: None,
            item: "x".into(),
            target: msg,
            mods: Mods::default(),
        };
        assert_eq!(s.act(click).unwrap_err().status, 404);
        assert_eq!(
            s.act(Action::ModalClose {
                plugin: "nope".into()
            })
            .unwrap_err()
            .status,
            404
        );

        // Profiles: only the client's own people, under a name it shows them by.
        let bad = papi::UserRef {
            id: ALICE,
            name: "Mallory".into(),
        };
        assert_eq!(
            s.act(Action::Profile { user: bad }).unwrap_err().status,
            400
        );
        for name in ["Alice", "alice", "Ali"] {
            s.act(Action::Profile {
                user: papi::UserRef {
                    id: ALICE,
                    name: name.into(),
                },
            })
            .unwrap();
        }
        assert_eq!(s.state(2)["slots"]["profile"]["user"]["name"], "Ali");

        s.act(Action::SlotAvailability {
            slot: SLOT_CHAT_BAR_BUTTON.into(),
            available: false,
        })
        .unwrap();
        let avail = s.state(3)["slots"]["available"].clone();
        assert!(
            !avail
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == SLOT_CHAT_BAR_BUTTON)
        );
    }

    #[test]
    fn event_names_are_the_wit_case_names() {
        assert_eq!(
            event_name(&Event::SelectionChanged(papi::SelectionChange {
                guild: None,
                channel: None,
                previous_channel: None,
                message: None
            })),
            "selection-changed"
        );
        assert_eq!(event_name(&Event::MyVoiceLeft), "my-voice-left");
        assert_eq!(
            describe_event(&Event::MyPresence("idle".into())),
            "my-presence idle"
        );
    }
}
