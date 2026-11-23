//! `/api/state`: everything the preview page draws, as JSON.

use rekindle_plugin_host::ClientBackend;
use rekindle_plugin_host::api::{self as papi, Target};
use rekindle_plugin_host::backend::{MockBackend, World};
use rekindle_plugin_host::model::{
    ALL_SLOTS, DECORATION_KINDS, DecorationKey, SLOT_PROFILE_SECTION, SlotItem, ThemeValue,
};
use serde_json::{Map, Value, json};

use super::Session;
use super::world::{channel_ref, default_target, display_name, facts, message_ref};
use crate::cmd::dev::api::PRESENCE;

/// Log records included in `/api/state`.
const STATE_LOGS: u64 = 300;
/// Toasts included in `/api/state`.
const STATE_TOASTS: usize = 8;
/// Performed actions and notifications included in `/api/state`.
const STATE_PERFORMED: usize = 40;
const STATE_NOTIFICATIONS: usize = 20;
/// The largest integer a JavaScript number holds exactly.
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

impl Session {
    /// Everything the preview page renders. Snowflakes too big for a
    /// JavaScript number are sent as strings.
    pub(in crate::cmd::dev) fn state(&mut self, rev: u64) -> Value {
        let ids: Vec<String> = self.entries.iter().map(|e| e.id.clone()).collect();
        let (theme, surfaces, client, slots, decorations, performed, notifications) = {
            let b = self.backend.lock().unwrap();
            (
                theme_state(&b),
                ids.iter()
                    .map(|id| plugin_surfaces(&b, id))
                    .collect::<Vec<_>>(),
                client_state(&b, &ids),
                self.slots_state(&b),
                decorations_state(&b),
                tail(&b.performed, STATE_PERFORMED)
                    .iter()
                    .map(|(p, a)| json!({ "plugin": p, "action": a }))
                    .collect::<Vec<_>>(),
                tail(&b.notifications, STATE_NOTIFICATIONS).to_vec(),
            )
        };
        let mut client = client;
        self.render_rows(&mut client);
        let features = self.runtime.features();

        let mut plugins = Vec::new();
        for (i, surfaces) in surfaces.into_iter().enumerate() {
            let id = &ids[i];
            let storage = self
                .runtime
                .storage(id)
                .ok()
                .map_or(Value::Null, |s| json!(s.lock().unwrap().entries()));
            let e = &self.entries[i];
            let mut p = json!({
                "id": id,
                "info": self.runtime.plugin(id),
                "manifest": self.runtime.manifest(id),
                "load_error": e.load_error,
                "build": e.source.as_ref().map(|s| &s.status),
                "has_source": e.source.is_some(),
                "storage": storage,
                "settings": self.runtime.settings_page(id),
            });
            if let (Value::Object(p), Value::Object(s)) = (&mut p, surfaces) {
                p.extend(s);
            }
            plugins.push(p);
        }

        let (_, total) = self.runtime.logs().since(u64::MAX);
        let first = total.saturating_sub(STATE_LOGS);
        let (logs, _) = self.runtime.logs().since(first);
        let start = total - logs.len() as u64;
        let logs: Vec<Value> = logs
            .into_iter()
            .enumerate()
            .map(|(i, r)| json!({ "seq": start + i as u64, "record": r }))
            .collect();
        let activity: Vec<Value> = self
            .activity
            .iter()
            .map(|(seq, text)| json!({ "seq": seq, "text": text }))
            .collect();

        let mut v = json!({
            "rev": rev,
            "title": self.title,
            "plugins": plugins,
            "theme": theme,
            "client": client,
            "presence": PRESENCE,
            "logs": logs,
            "slots": slots,
            "decorations": decorations,
            "performed": performed,
            "notifications": notifications,
            "activity": activity,
            "features": features,
            "last_send": self.last_send,
            "last_slash": self.last_slash,
            "last_rest": self.last_rest,
        });
        js_safe(&mut v);
        v
    }

    /// Slot items per slot (with their owners), a default target for each
    /// slot, recent toasts, open modals, and the open profile's sections.
    /// Runs the plugins' renderers over the selected channel's messages, as
    /// the client does when it paints them, and adds each message row's
    /// transforms as `render`. The backend is unlocked first: the render
    /// isolate reads the theme through it.
    fn render_rows(&mut self, client: &mut Value) {
        let Some(rows) = client["rows"].as_array_mut() else {
            return;
        };
        let inputs: Vec<(usize, papi::RenderInput)> = {
            let b = self.backend.lock().unwrap();
            let channel = b.world.selection.channel;
            rows.iter()
                .enumerate()
                .filter(|(_, r)| r["kind"] == "message")
                .filter_map(|(i, r)| {
                    let id = r["id"].as_u64()?;
                    let m = b.world.find_message(channel?, id)?.clone();
                    let author_roles = m
                        .guild
                        .and_then(|g| b.member(g, m.author.id))
                        .map(|mb| mb.roles)
                        .unwrap_or_default();
                    Some((
                        i,
                        papi::RenderInput {
                            message: m,
                            author_roles,
                            grouped: false,
                            dark: true,
                        },
                    ))
                })
                .collect()
        };
        if inputs.is_empty() {
            return;
        }
        let batch: Vec<papi::RenderInput> = inputs.iter().map(|(_, m)| m.clone()).collect();
        let rendered = self.runtime.render(&batch);
        for ((i, _), per) in inputs.iter().zip(rendered) {
            rows[*i]["render"] = json!(per);
        }
    }

    fn slots_state(&self, b: &MockBackend) -> Value {
        let items: Map<String, Value> = ALL_SLOTS
            .iter()
            .map(|(slot, _)| {
                let list: Vec<Value> = b
                    .slot_items(slot)
                    .into_iter()
                    .map(|(p, i)| {
                        let active = match i {
                            SlotItem::IconButton { active, .. } => {
                                Some(b.is_active(p, slot, i.id()).unwrap_or(*active))
                            }
                            _ => None,
                        };
                        json!({ "plugin": p, "item": i, "active": active })
                    })
                    .collect();
                (slot.to_string(), Value::Array(list))
            })
            .collect();
        let kinds: Map<String, Value> = ALL_SLOTS
            .iter()
            .map(|(s, k)| (s.to_string(), json!(k)))
            .collect();
        let targets: Map<String, Value> = ALL_SLOTS
            .iter()
            .map(|(s, _)| {
                (
                    s.to_string(),
                    default_target(b, s).map_or(Value::Null, |t| desc(b, &t)),
                )
            })
            .collect();
        let toasts: Vec<Value> = {
            let all: Vec<_> = b.recent_toasts().collect();
            let skip = all.len().saturating_sub(STATE_TOASTS);
            all[skip..].iter().map(|(seq, t)| json!({ "seq": seq, "plugin": t.plugin, "text": t.text, "kind": t.kind })).collect()
        };
        let modals: Vec<Value> = b
            .modals()
            .map(|(p, panel)| json!({ "plugin": p, "panel": panel }))
            .collect();
        let profile = self.profile.as_ref().map(|user| {
            let sections: Vec<Value> = b
                .slot_items(SLOT_PROFILE_SECTION)
                .into_iter()
                .filter_map(|(p, i)| match i {
                    SlotItem::ListItem { id, title } => Some(json!({
                        "plugin": p,
                        "id": id,
                        "title": title,
                        // null until the plugin answers `profile-opened`.
                        "elements": b.profile_section(p, id, user.id),
                    })),
                    _ => None,
                })
                .collect();
            json!({ "user": user, "sections": sections })
        });
        json!({
            "available": b.available_slots(),
            "all": ALL_SLOTS.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            "kinds": kinds,
            "items": items,
            "targets": targets,
            "toasts": toasts,
            "modals": modals,
            "profile": profile,
        })
    }
}

/// The last `n` items of `v`.
fn tail<T>(v: &[T], n: usize) -> &[T] {
    &v[v.len().saturating_sub(n)..]
}

/// A target the page can hand back verbatim (as JSON text, so 64-bit ids
/// survive), with the predicates that hold for it.
fn desc(b: &MockBackend, t: &Target) -> Value {
    json!({ "target": serde_json::to_string(t).unwrap_or_default(), "facts": facts(b, t) })
}

fn theme_state(b: &MockBackend) -> Vec<Value> {
    b.theme_tokens()
        .into_iter()
        .filter_map(|info| {
            let v = b.get_value(&info.name)?;
            // CSS-ready string: colors as hex, lengths in px.
            let css = match &v {
                ThemeValue::Color(c) => c.to_hex(),
                ThemeValue::Length(x) => format!("{x}px"),
                ThemeValue::Font(f) => format!("\"{f}\""),
                ThemeValue::Choice(k) => k.clone(),
            };
            Some(json!({
                "token": info.name,
                "kind": info.kind,
                "group": info.group,
                "aliases": info.aliases,
                "value": css,
                "owner": b.override_owner(&info.name),
            }))
        })
        .collect()
}

/// A plugin's own UI contributions (merged into its `plugins[]` entry).
fn plugin_surfaces(b: &MockBackend, id: &str) -> Value {
    json!({
        "panels": b.panels().filter(|(p, _)| *p == id).map(|(_, x)| x).collect::<Vec<_>>(),
        "commands": b.commands().iter().filter(|c| c.plugin == id).map(|c| json!({ "id": c.id, "title": c.title })).collect::<Vec<_>>(),
        "settings_extra": b.settings_extra(id),
        "notice": b.notice(id),
        "keybinds": b.keybinds().iter().filter(|k| k.plugin == id).collect::<Vec<_>>(),
        "slash_commands": b.slash_commands().filter(|(p, _)| *p == id).map(|(_, c)| c).collect::<Vec<_>>(),
        "click_bindings": b.click_bindings().filter(|(p, _)| *p == id).map(|(_, c)| c).collect::<Vec<_>>(),
    })
}

/// The client as the page draws it: guilds, channels, people, the selected
/// channel's messages (with ghost rows and accessories), the composer.
fn client_state(b: &MockBackend, plugins: &[String]) -> Value {
    let w = &b.world;
    let guilds: Vec<Value> = w
        .guilds
        .iter()
        .map(|g| json!({ "id": g.id, "name": g.name, "t": desc(b, &Target::Guild(papi::GuildRef { id: g.id, name: g.name.clone() })) }))
        .collect();
    let channels: Vec<Value> = w
        .channels
        .iter()
        .filter(|c| c.visible)
        .map(|c| {
            json!({
                "id": c.id,
                "name": c.name,
                "guild": c.guild,
                "kind": c.kind,
                "parent": c.parent,
                "position": c.position,
                "topic": c.topic,
                "recipient": c.recipients.first().map(|u| u.id),
                "unread": w.read_states.get(&c.id).map(|r| json!({ "unread": r.unread, "mentions": r.mentions })),
                "t": desc(b, &Target::Channel(channel_ref(c))),
            })
        })
        .collect();
    let users: Map<String, Value> = w
        .users
        .values()
        .map(|u| {
            let status = w.presences.get(&u.id).map(|p| p.status.clone());
            (u.id.to_string(), json!({ "id": u.id, "name": display_name(w, u.id, None), "username": u.username, "bot": u.bot, "status": status }))
        })
        .collect();
    let selected = w.selection.channel;
    let typing: Vec<String> = selected
        .and_then(|c| w.typing.get(&c))
        .map(|v| {
            v.iter()
                .map(|u| display_name(w, *u, w.selection.guild))
                .collect()
        })
        .unwrap_or_default();
    let notices: Vec<Value> = plugins
        .iter()
        .filter_map(|p| b.notice(p).map(|n| json!({ "plugin": p, "notice": n })))
        .collect();
    json!({
        "me": { "id": w.me, "name": display_name(w, w.me, None) },
        "status": w.status,
        "selection": { "guild": w.selection.guild, "channel": selected },
        "guilds": guilds,
        "channels": channels,
        "users": users,
        "rows": selected.map(|c| message_rows(b, plugins, c)).unwrap_or_default(),
        "typing": typing,
        "composer": w.composer,
        "replying": w.pending_reply.map(|(r, mention)| json!({ "to": display_name(w, r.author, r.guild), "mention": mention, "channel": r.channel })),
        "editing": w.editing.map(|r| json!({ "id": r.id, "channel": r.channel })),
        "notices": notices,
    })
}

/// A channel's loaded messages and plugins' ghost rows, in id order.
fn message_rows(b: &MockBackend, plugins: &[String], channel: u64) -> Vec<Value> {
    let w = &b.world;
    let mut rows: Vec<(u64, u8, Value)> = Vec::new();
    for m in w.messages.get(&channel).into_iter().flatten() {
        let accessories: Vec<Value> = plugins
            .iter()
            .filter_map(|p| b.accessory(p, m.id).map(|els| json!({ "plugin": p, "surface": format!("message:{}", m.id), "elements": els })))
            .collect();
        let reply_to = m.reply_to.map(|r| {
            let to = w.find_message(r.channel, r.id);
            json!({ "author": display_name(w, r.author, r.guild), "content": to.map(|t| excerpt(&t.content, 80)) })
        });
        let author = papi::UserRef {
            id: m.author.id,
            name: m.author.name.clone(),
        };
        rows.push((
            m.id,
            0,
            json!({
                "kind": "message",
                "id": m.id,
                "channel": m.channel,
                "author": author,
                "bot": m.bot,
                "content": m.content,
                "edited": m.edited_ms.is_some(),
                "pinned": m.pinned,
                "reply_to": reply_to,
                "reactions": m.reactions.iter().map(|x| json!({ "emoji": x.emoji, "count": x.count, "me": x.me })).collect::<Vec<_>>(),
                "attachments": m.attachments.len(),
                "t": desc(b, &Target::Message(message_ref(m))),
                "author_t": desc(b, &Target::User((author.clone(), m.guild))),
                "accessories": accessories,
            }),
        ));
    }
    for (p, g) in b.ghosts(channel) {
        rows.push((g.id, 1, json!({ "kind": "ghost", "plugin": p, "id": g.id, "author": g.author, "content": g.content, "style": g.style })));
    }
    rows.sort_by_key(|(id, order, _)| (*id, *order));
    rows.into_iter().map(|(_, _, v)| v).collect()
}

/// Every decoration, per kind, with a label for what it's on.
fn decorations_state(b: &MockBackend) -> Vec<Value> {
    let available = b.available_decorations();
    DECORATION_KINDS
        .iter()
        .map(|(kind, keyed_by)| {
            let items: Vec<Value> = b
                .decorations(kind)
                .into_iter()
                .map(|(p, key, d)| json!({ "plugin": p, "key": key, "label": key_label(&b.world, *keyed_by, key), "decoration": d }))
                .collect();
            json!({ "kind": kind, "keyed_by": keyed_by, "available": available.iter().any(|k| k == kind), "items": items })
        })
        .collect()
}

fn key_label(w: &World, keyed_by: DecorationKey, key: u64) -> String {
    match keyed_by {
        DecorationKey::User => display_name(w, key, None),
        DecorationKey::Channel => match w.channels.iter().find(|c| c.id == key) {
            Some(c) if c.guild.is_some() => format!("#{}", c.name),
            Some(c) => format!("@{}", c.name),
            None => format!("channel {key}"),
        },
        DecorationKey::Guild => w
            .guilds
            .iter()
            .find(|g| g.id == key)
            .map_or_else(|| format!("guild {key}"), |g| g.name.clone()),
        DecorationKey::Message => match w.messages.values().flatten().find(|m| m.id == key) {
            Some(m) => format!("{}: {}", m.author.name, excerpt(&m.content, 40)),
            None => format!("message {key}"),
        },
    }
}

/// The first `max` characters of `s`, with an ellipsis if there were more.
pub(super) fn excerpt(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}…")
}

/// Integers beyond 2^53 become strings: the page's numbers are doubles.
fn js_safe(v: &mut Value) {
    match v {
        Value::Array(a) => a.iter_mut().for_each(js_safe),
        Value::Object(o) => o.values_mut().for_each(js_safe),
        Value::Number(n) => {
            let big = n.as_u64().is_some_and(|x| x > MAX_SAFE_INTEGER)
                || n.as_i64().is_some_and(|x| x < -(MAX_SAFE_INTEGER as i64));
            if big {
                *v = Value::String(n.to_string());
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use rekindle_plugin_host::backend::fixtures::{ALICE, GENERAL};
    use rekindle_plugin_host::model::{Element, Icon, UiOp};

    use super::super::fixtures::{general_messages, session};
    use super::super::{DEV_TRUST, Entry};
    use super::*;

    #[test]
    fn big_snowflakes_reach_the_page_as_strings() {
        let mut s = session();
        let st = s.state(1);
        let id = general_messages(&s)[0].id;
        assert!(id > MAX_SAFE_INTEGER);
        assert_eq!(st["client"]["rows"][0]["id"], id.to_string());
        assert_eq!(
            st["client"]["rows"][0]["channel"], GENERAL,
            "small ids stay numbers"
        );
        // Targets are JSON text the page hands back verbatim: exact.
        let target: Target =
            serde_json::from_str(st["client"]["rows"][0]["t"]["target"].as_str().unwrap()).unwrap();
        assert!(matches!(target, Target::Message(r) if r.id == id));
    }

    #[test]
    fn plugin_surfaces_reach_the_state() {
        let mut s = session();
        {
            let mut b = s.backend.lock().unwrap();
            let m = message_ref(&b.world.messages[&GENERAL][0]);
            b.ui_update(
                "p",
                UiOp::SetMessageAccessory {
                    message: m,
                    elements: Some(vec![Element::Text { text: "acc".into() }]),
                },
            )
            .unwrap();
            let deco = rekindle_plugin_host::model::Decoration {
                icon: Some(Icon::Star),
                image: None,
                text: None,
                tooltip: None,
                color: None,
                clickable: true,
            };
            b.ui_update(
                "p",
                UiOp::SetDecoration {
                    kind: "user.name-badge".into(),
                    key: ALICE,
                    decoration: Some(deco),
                },
            )
            .unwrap();
            let ghost = rekindle_plugin_host::model::GhostMessage {
                channel: GENERAL,
                id: m.id + 1,
                author: papi::UserRef {
                    id: ALICE,
                    name: "Ali".into(),
                },
                content: vec![rekindle_plugin_host::model::Span::plain("was here")],
                style: "deleted".into(),
            };
            b.ui_update("p", UiOp::SetGhostMessage { ghost }).unwrap();
        }
        // Accessories and notices are collected for loaded plugins only.
        s.entries.push(Entry {
            id: "p".into(),
            trust: DEV_TRUST,
            bytes: None,
            load_error: None,
            source: None,
        });
        let st = s.state(1);
        let rows = st["client"]["rows"].as_array().unwrap();
        assert_eq!(rows[0]["accessories"][0]["elements"][0]["text"], "acc");
        assert_eq!(rows[1]["kind"], "ghost", "a ghost sits where its id falls");
        let badges = st["decorations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["kind"] == "user.name-badge")
            .unwrap();
        assert_eq!(badges["items"][0]["label"], "Alice");
        assert_eq!(st["plugins"][0]["settings_extra"], json!([]));
    }
}
