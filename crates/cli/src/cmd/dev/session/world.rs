//! The mock client's world as a real client would treat it: lookups, the
//! targets and surfaces it would offer, and the effect of an event on it.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use rekindle_plugin_host::ClientBackend;
use rekindle_plugin_host::api::{self as papi, Event, OptionKind, OptionValue, Target};
use rekindle_plugin_host::backend::{MockBackend, World};
use rekindle_plugin_host::model::{ALL_SLOTS, Element, MenuPredicate, SlotItem, UiEvent, UiValue};
use rekindle_plugin_host::validate::{check_elements, check_slot_target, slot_target_kind};
use serde_json::json;

use crate::cmd::dev::api::ApiError;

/// 2015-01-01T00:00:00Z, where snowflake timestamps start.
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;
/// Discord permission bits the menu predicates look at.
const ADD_REACTIONS: u64 = 1 << 6;
const SEND_MESSAGES: u64 = 1 << 11;
const MANAGE_MESSAGES: u64 = 1 << 13;
/// Reserved `on-ui` surfaces (after the plugin's own panels and modals).
const SURFACE_SETTINGS_PAGE: &str = "settings-page";
const SURFACE_NOTICE: &str = "notice";
/// Discord's channel and message type numbers the checks look at.
const CHANNEL_CATEGORY: u16 = 4;
const MESSAGE_DEFAULT: u16 = 0;
pub(super) const MESSAGE_REPLY: u16 = 19;

/// The wall clock, in milliseconds since the Unix epoch.
pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The loaded channel `id`, or a 404.
pub(super) fn find_channel(w: &World, id: u64) -> Result<&papi::Channel, ApiError> {
    w.channels
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| ApiError::new(404, format!("no channel {id} in the mock client")))
}

/// The known user `id`, or a 404.
pub(super) fn find_user(w: &World, id: u64) -> Result<&papi::User, ApiError> {
    w.users
        .get(&id)
        .ok_or_else(|| ApiError::new(404, format!("no user {id} in the mock client")))
}

/// Message `id`, if `channel` has it loaded, or a 404.
pub(super) fn find_message(w: &World, channel: u64, id: u64) -> Result<&papi::Message, ApiError> {
    w.find_message(channel, id).ok_or_else(|| {
        ApiError::new(
            404,
            format!("message {id} isn't loaded in channel {channel}"),
        )
    })
}

/// The name the client shows for a user here: their nickname in `guild`,
/// else their global display name, else their username.
pub(super) fn display_name(w: &World, user: u64, guild: Option<u64>) -> String {
    let nick = guild
        .and_then(|g| w.members.iter().find(|m| m.guild == g && m.user == user))
        .and_then(|m| m.nick.clone());
    nick.or_else(|| {
        w.users
            .get(&user)
            .map(|u| u.global_name.clone().unwrap_or_else(|| u.username.clone()))
    })
    .unwrap_or_else(|| user.to_string())
}

/// A fresh message id for `channel`: now, and after anything loaded there.
pub(super) fn next_message_id(w: &World, channel: u64) -> u64 {
    let stamp = now_ms().saturating_sub(DISCORD_EPOCH_MS) << 22;
    let last = w
        .messages
        .get(&channel)
        .and_then(|v| v.iter().map(|m| m.id).max())
        .unwrap_or(0);
    stamp.max(last + 1)
}

/// The reference a target or an action carries for `m`.
pub(super) fn message_ref(m: &papi::Message) -> papi::MessageRef {
    papi::MessageRef {
        id: m.id,
        channel: m.channel,
        guild: m.guild,
        author: m.author.id,
    }
}

/// The reference a target carries for `c`.
pub(super) fn channel_ref(c: &papi::Channel) -> papi::ChannelRef {
    papi::ChannelRef {
        id: c.id,
        name: c.name.clone(),
        guild: c.guild,
        kind: c.kind,
    }
}

/// Check an interaction against the surface the plugin is showing: its
/// panel or modal with that id first, then the reserved surfaces
/// `settings-page` (its settings-page extras), `notice`, and `message:<id>`
/// (an accessory).
pub(super) fn check_surface(b: &MockBackend, plugin: &str, ev: &UiEvent) -> Result<(), ApiError> {
    let missing = || {
        ApiError::new(
            404,
            format!("plugin `{plugin}` has no surface `{}` showing", ev.surface),
        )
    };
    let s = ev.surface.as_str();
    let elements: &[Element] = if let Some(panel) = b
        .panel(plugin, s)
        .or_else(|| b.modal(plugin).filter(|m| m.id == s))
    {
        &panel.elements
    } else if s == SURFACE_SETTINGS_PAGE {
        b.settings_extra(plugin)
    } else if s == SURFACE_NOTICE {
        if b.notice(plugin).is_none_or(|n| n.button.is_none()) {
            return Err(ApiError::new(
                404,
                format!("plugin `{plugin}` has no notice with a button showing"),
            ));
        }
        if ev.element != "button" || ev.value != UiValue::Clicked {
            return Err(ApiError::bad(
                "a notice only has its button, which is clicked",
            ));
        }
        return Ok(());
    } else if let Some(id) = s.strip_prefix("message:") {
        let id: u64 = id
            .parse()
            .map_err(|_| ApiError::bad(format!("`{s}` is not `message:<id>`")))?;
        b.accessory(plugin, id).ok_or_else(missing)?
    } else {
        return Err(missing());
    };
    check_elements(s, elements, &ev.element, &ev.value).map_err(ApiError::bad)
}

/// Which slot a click on `plugin`'s `item` came from: the one named, or the
/// one it's in that takes this kind of target. Checks, as a client would
/// only offer it there, that the target is loaded and the item's
/// predicates hold for it.
pub(super) fn resolve_slot(
    b: &MockBackend,
    plugin: &str,
    slot: Option<&str>,
    item: &str,
    target: &Target,
) -> Result<String, ApiError> {
    let found: Vec<(&str, &SlotItem)> = ALL_SLOTS
        .iter()
        .map(|(s, _)| *s)
        .filter(|s| slot.is_none_or(|want| want == *s))
        .filter_map(|s| {
            b.slot_items(s)
                .into_iter()
                .find(|(p, i)| *p == plugin && i.id() == item)
                .map(|(_, i)| (s, i))
        })
        .collect();
    if found.is_empty() {
        let place = slot.map(|s| format!(" in `{s}`")).unwrap_or_default();
        return Err(ApiError::new(
            404,
            format!("plugin `{plugin}` has no slot item `{item}`{place}"),
        ));
    }
    let mut first_err = None;
    let mut chosen = None;
    for (s, i) in found {
        match check_slot_target(s, i, target) {
            Ok(()) => {
                chosen = Some((s, i));
                break;
            }
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    let (s, i) = chosen.ok_or_else(|| ApiError::bad(first_err.unwrap_or_default()))?;
    check_known_target(b, target).map_err(ApiError::bad)?;
    if let SlotItem::MenuItem { when, .. } = i {
        let facts = facts(b, target);
        if let Some(p) = when.iter().find(|p| !facts.contains(p)) {
            return Err(ApiError::bad(format!(
                "`{item}` isn't offered here: its `{}` predicate doesn't hold",
                json!(p).as_str().unwrap_or("?")
            )));
        }
    }
    Ok(s.to_string())
}

/// The mock client only offers targets it holds: loaded messages, its own
/// people (by a name it shows them under), channels, guilds and roles.
fn check_known_target(b: &MockBackend, t: &Target) -> Result<(), String> {
    let w = &b.world;
    match t {
        Target::None | Target::Link(_) | Target::Selection(_) => Ok(()),
        Target::Message(r) => {
            let m = w
                .find_message(r.channel, r.id)
                .ok_or_else(|| format!("message {} isn't loaded in channel {}", r.id, r.channel))?;
            if m.author.id != r.author || m.guild != r.guild {
                return Err(format!(
                    "message {} doesn't match the client's (author or guild)",
                    r.id
                ));
            }
            Ok(())
        }
        Target::User((u, guild)) => {
            check_known_user(w, u)?;
            match guild {
                Some(g) if !w.guilds.iter().any(|x| x.id == *g) => {
                    Err(format!("unknown guild {g}"))
                }
                _ => Ok(()),
            }
        }
        Target::Channel(c) => match w.channels.iter().find(|x| x.id == c.id) {
            Some(x) if x.name == c.name && x.guild == c.guild && x.kind == c.kind => Ok(()),
            Some(_) => Err(format!("channel {} doesn't match the client's", c.id)),
            None => Err(format!("unknown channel {}", c.id)),
        },
        Target::Guild(g) => match w.guilds.iter().find(|x| x.id == g.id) {
            Some(x) if x.name == g.name => Ok(()),
            Some(_) => Err(format!("guild {} isn't called `{}`", g.id, g.name)),
            None => Err(format!("unknown guild {}", g.id)),
        },
        Target::Role(r) => match w.roles.iter().find(|x| x.id == r.id && x.guild == r.guild) {
            Some(x) if x.name == r.name => Ok(()),
            Some(_) => Err(format!("role {} isn't called `{}`", r.id, r.name)),
            None => Err(format!("unknown role {} in guild {}", r.id, r.guild)),
        },
        Target::Media(m) => match &m.message {
            Some(r) => check_known_target(b, &Target::Message(*r)),
            None => Ok(()),
        },
    }
}

/// A user the client knows, under a name it shows them by (username,
/// display name, or a nickname in one of the guilds).
pub(super) fn check_known_user(w: &World, u: &papi::UserRef) -> Result<(), String> {
    let Some(user) = w.users.get(&u.id) else {
        return Err(format!("unknown user {}", u.id));
    };
    let nicks = w
        .members
        .iter()
        .filter(|m| m.user == u.id)
        .filter_map(|m| m.nick.as_deref());
    let mut names = [Some(user.username.as_str()), user.global_name.as_deref()]
        .into_iter()
        .flatten()
        .chain(nicks);
    if names.any(|n| n == u.name) {
        Ok(())
    } else {
        Err(format!(
            "user {} is not called `{}` anywhere in the client",
            u.id, u.name
        ))
    }
}

/// The menu predicates (`ui.menu-predicate`) that hold for a target, as the
/// host evaluates them.
pub(super) fn facts(b: &MockBackend, t: &Target) -> Vec<MenuPredicate> {
    use MenuPredicate::{
        CanDelete, CanEdit, CanManageMessages, CanPin, CanReact, CanReply, HasAttachments,
        HasContent, HasEmbeds, InDm, InGuild, OthersMessage, OwnMessage, TargetIsBot,
        TargetIsFriend, TargetIsSelf,
    };
    let w = &b.world;
    let mut out = Vec::new();
    let about_user = |out: &mut Vec<MenuPredicate>, user: u64| {
        if w.users.get(&user).is_some_and(|u| u.bot) {
            out.push(TargetIsBot);
        }
        if user == w.me {
            out.push(TargetIsSelf);
        }
        if w.relationships
            .iter()
            .any(|r| r.user == user && r.kind == papi::RelationshipKind::Friend)
        {
            out.push(TargetIsFriend);
        }
    };
    let place = |out: &mut Vec<MenuPredicate>, guild: Option<u64>| {
        out.push(if guild.is_some() { InGuild } else { InDm });
    };
    match t {
        Target::Message(r) => {
            let Some(m) = w.find_message(r.channel, r.id) else {
                return out;
            };
            let own = m.author.id == w.me;
            let dm = m.guild.is_none();
            let perms = b.my_permissions(m.channel);
            let manage = perms & MANAGE_MESSAGES != 0;
            out.push(if own { OwnMessage } else { OthersMessage });
            place(&mut out, m.guild);
            about_user(&mut out, m.author.id);
            if !m.attachments.is_empty() {
                out.push(HasAttachments);
            }
            if !m.embeds.is_empty() {
                out.push(HasEmbeds);
            }
            if !m.content.is_empty() {
                out.push(HasContent);
            }
            if manage {
                out.push(CanManageMessages);
            }
            if manage || dm {
                out.push(CanPin);
            }
            if own && matches!(m.kind, MESSAGE_DEFAULT | MESSAGE_REPLY) {
                out.push(CanEdit);
            }
            if dm || perms & SEND_MESSAGES != 0 {
                out.push(CanReply);
            }
            if dm || perms & ADD_REACTIONS != 0 {
                out.push(CanReact);
            }
            if own || manage {
                out.push(CanDelete);
            }
        }
        Target::User((u, guild)) => {
            place(&mut out, *guild);
            about_user(&mut out, u.id);
        }
        Target::Channel(c) => {
            place(&mut out, c.guild);
            if b.my_permissions(c.id) & MANAGE_MESSAGES != 0 {
                out.push(CanManageMessages);
            }
        }
        Target::Guild(_) | Target::Role(_) => out.push(InGuild),
        Target::None | Target::Media(_) | Target::Link(_) | Target::Selection(_) => {}
    }
    out
}

/// Something sensible to click a slot's items on when the page has no
/// natural place for that slot.
pub(super) fn default_target(b: &MockBackend, slot: &str) -> Option<Target> {
    let w = &b.world;
    let ch = w
        .selection
        .channel
        .and_then(|c| w.channels.iter().find(|x| x.id == c));
    Some(match slot_target_kind(slot)? {
        "none" => Target::None,
        "message" => {
            let here = ch
                .and_then(|c| w.messages.get(&c.id))
                .and_then(|v| v.last());
            Target::Message(message_ref(
                here.or_else(|| w.messages.values().flatten().last())?,
            ))
        }
        "user" => {
            let u = *w.users.keys().find(|u| **u != w.me)?;
            Target::User((
                papi::UserRef {
                    id: u,
                    name: display_name(w, u, w.selection.guild),
                },
                w.selection.guild,
            ))
        }
        "channel" => Target::Channel(channel_ref(ch?)),
        "guild" => {
            let g = w
                .selection
                .guild
                .and_then(|g| w.guilds.iter().find(|x| x.id == g))
                .or(w.guilds.first())?;
            Target::Guild(papi::GuildRef {
                id: g.id,
                name: g.name.clone(),
            })
        }
        "role" => {
            let r = w
                .roles
                .iter()
                .find(|r| Some(r.guild) == w.selection.guild && r.id != r.guild)
                .or(w.roles.first())?;
            Target::Role(papi::RoleRef {
                id: r.id,
                guild: r.guild,
                name: r.name.clone(),
            })
        }
        "media" => Target::Media(papi::MediaRef {
            url: "https://cdn.discordapp.com/attachments/1/2/sample.png".into(),
            content_type: "image/png".into(),
            width: 640,
            height: 480,
            message: None,
        }),
        "link" => {
            let in_channel = ch.and_then(|c| w.messages.get(&c.id)).into_iter().flatten();
            let link = in_channel
                .flat_map(|m| m.content.split_whitespace())
                .find(|t| t.starts_with("https://") || t.starts_with("http://"));
            Target::Link(link.unwrap_or("https://example.com/").to_string())
        }
        "selection" => Target::Selection("some selected text".into()),
        _ => return None,
    })
}

/// An event's world effect: the client's stores apply it before plugins
/// hear about it, so their queries see the new state.
pub(super) fn apply_to_world(w: &mut World, ev: &Event) -> Result<(), String> {
    let channel_in =
        |w: &World, id: u64, guild: Option<u64>| match w.channels.iter().find(|c| c.id == id) {
            None => Err(format!("unknown channel {id}")),
            Some(c) if c.guild != guild => Err(format!(
                "channel {id} is in guild {:?}, not {guild:?}",
                c.guild
            )),
            Some(_) => Ok(()),
        };
    let user = |w: &World, id: u64| {
        if w.users.contains_key(&id) {
            Ok(())
        } else {
            Err(format!("unknown user {id}"))
        }
    };
    match ev {
        Event::SelectionChanged(s) => {
            match s.channel {
                Some(c) => {
                    channel_in(w, c, s.guild)?;
                    if w.channels
                        .iter()
                        .any(|x| x.id == c && (x.kind == CHANNEL_CATEGORY || !x.visible))
                    {
                        return Err(format!(
                            "channel {c} can't be opened (a category, or hidden)"
                        ));
                    }
                }
                None => {
                    if let Some(g) = s.guild.filter(|g| !w.guilds.iter().any(|x| x.id == *g)) {
                        return Err(format!("unknown guild {g}"));
                    }
                }
            }
            w.selection.channel = s.channel;
            w.selection.guild = s.guild;
        }
        Event::MyPresence(status) => w.status.clone_from(status),
        Event::MessageCreated(m) => {
            channel_in(w, m.channel, m.guild)?;
            user(w, m.author.id)?;
            if w.find_message(m.channel, m.id).is_some() {
                return Err(format!("message {} is already loaded", m.id));
            }
            w.push_message(m.clone());
        }
        Event::MessageEdited((m, _)) => {
            let loaded = w
                .find_message_mut(m.channel, m.id)
                .ok_or_else(|| format!("message {} isn't loaded in channel {}", m.id, m.channel))?;
            if loaded.author.id != m.author.id {
                return Err(format!("message {} has another author", m.id));
            }
            *loaded = m.clone();
        }
        Event::MessageDeleted(d) => {
            if let Some(list) = w.messages.get_mut(&d.channel) {
                list.retain(|m| m.id != d.id);
            }
            w.visible.retain(|r| r.id != d.id);
        }
        Event::Typing((channel, users)) => {
            if !w.channels.iter().any(|c| c.id == *channel) {
                return Err(format!("unknown channel {channel}"));
            }
            for u in users {
                user(w, *u)?;
            }
            w.typing.insert(*channel, users.clone());
        }
        Event::Presence(p) => {
            user(w, p.user)?;
            w.presences.insert(p.user, p.clone());
        }
        _ => {}
    }
    Ok(())
}

/// A slash command invocation from the page's text options, typed per the
/// command's declared options (as the client's command bar would).
pub(super) fn slash_invocation(
    b: &MockBackend,
    plugin: &str,
    command: &str,
    given: &BTreeMap<String, String>,
    channel: Option<u64>,
) -> Result<papi::Invocation, ApiError> {
    let w = &b.world;
    let cmd = b
        .slash_commands()
        .find(|(p, c)| *p == plugin && c.name == command)
        .map(|(_, c)| c)
        .ok_or_else(|| {
            ApiError::new(
                404,
                format!("plugin `{plugin}` has no slash command `/{command}`"),
            )
        })?;
    let channel = channel
        .or(w.selection.channel)
        .ok_or_else(|| ApiError::bad("no channel selected"))?;
    let ch = find_channel(w, channel)?;
    if ch.guild.is_some() && !cmd.in_guilds {
        return Err(ApiError::bad(format!(
            "/{command} isn't offered in servers"
        )));
    }
    if ch.guild.is_none() && !cmd.in_dms {
        return Err(ApiError::bad(format!("/{command} isn't offered in DMs")));
    }
    if let Some(k) = given
        .keys()
        .find(|k| !cmd.options.iter().any(|o| &o.name == *k))
    {
        return Err(ApiError::bad(format!("/{command} has no option `{k}`")));
    }
    let mut options = Vec::new();
    for o in &cmd.options {
        let Some(raw) = given
            .get(&o.name)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        else {
            if o.required {
                return Err(ApiError::bad(format!("/{command} needs `{}`", o.name)));
            }
            continue;
        };
        let value =
            option_value(w, o, raw).map_err(|e| ApiError::bad(format!("`{}`: {e}", o.name)))?;
        options.push((o.name.clone(), value));
    }
    Ok(papi::Invocation {
        command: command.to_string(),
        options,
        channel: channel_ref(ch),
    })
}

/// A slash command option typed as text, as `o` declares it.
fn option_value(w: &World, o: &papi::CommandOption, raw: &str) -> Result<OptionValue, String> {
    let in_range = |n: f64| {
        if o.min.is_some_and(|m| n < m) || o.max.is_some_and(|m| n > m) {
            Err(format!("{n} is out of range"))
        } else {
            Ok(())
        }
    };
    let id = || {
        raw.parse::<u64>()
            .map_err(|_| format!("`{raw}` is not an id"))
    };
    let user = |id: u64| w.users.contains_key(&id);
    let role = |id: u64| w.roles.iter().any(|r| r.id == id);
    Ok(match o.kind {
        OptionKind::Text => OptionValue::Text(raw.to_string()),
        OptionKind::Integer => {
            let n: i64 = raw
                .parse()
                .map_err(|_| format!("`{raw}` is not an integer"))?;
            in_range(n as f64)?;
            OptionValue::Integer(n)
        }
        OptionKind::Number => {
            let n: f64 = raw
                .parse()
                .ok()
                .filter(|n: &f64| n.is_finite())
                .ok_or_else(|| format!("`{raw}` is not a number"))?;
            in_range(n)?;
            OptionValue::Number(n)
        }
        OptionKind::Boolean => OptionValue::Boolean(match raw {
            "true" | "yes" | "1" => true,
            "false" | "no" | "0" => false,
            _ => return Err(format!("`{raw}` is not true/false")),
        }),
        OptionKind::User => {
            OptionValue::User(Some(id()?).filter(|u| user(*u)).ok_or("no such user")?)
        }
        OptionKind::Channel => OptionValue::Channel(
            Some(id()?)
                .filter(|c| w.channels.iter().any(|x| x.id == *c))
                .ok_or("no such channel")?,
        ),
        OptionKind::Role => {
            OptionValue::Role(Some(id()?).filter(|r| role(*r)).ok_or("no such role")?)
        }
        OptionKind::Mentionable => match id()? {
            u if user(u) => OptionValue::User(u),
            r if role(r) => OptionValue::Role(r),
            _ => return Err("no such user or role".into()),
        },
        OptionKind::Choice => {
            let (_, value) = o
                .choices
                .iter()
                .find(|(label, value)| value == raw || label == raw)
                .ok_or_else(|| format!("`{raw}` is not a choice"))?;
            OptionValue::Choice(value.clone())
        }
    })
}

#[cfg(test)]
mod tests {
    use rekindle_plugin_host::api::{ChannelRef, UserRef};
    use rekindle_plugin_host::backend::fixtures::{ALICE, BOT, DM_ALICE, GAMERS, GENERAL, LAB, ME};
    use rekindle_plugin_host::model::{
        Notice, Panel, SLOT_CHANNEL_MENU, SLOT_CHAT_BAR_BUTTON, SLOT_GUILD_LIST_BUTTON,
        SLOT_MESSAGE_HOVER_BAR, SLOT_MESSAGE_MENU, SLOT_PROFILE_SECTION, SLOT_USER_MENU, ToastKind,
        UiOp,
    };
    use rekindle_plugin_host::validate::check_ui_event;

    use super::super::fixtures::{button, menu};
    use super::*;

    fn ui(surface: &str, element: &str, value: UiValue) -> UiEvent {
        UiEvent {
            surface: surface.into(),
            element: element.into(),
            value,
        }
    }

    fn panel(id: &str, elements: Vec<Element>) -> Panel {
        Panel {
            id: id.into(),
            title: id.to_uppercase(),
            elements,
        }
    }

    fn toggle(id: &str) -> Element {
        Element::Toggle {
            id: id.into(),
            label: id.into(),
            value: false,
        }
    }

    #[test]
    fn slot_clicks_resolve_the_slot_and_check_the_target() {
        let mut b = MockBackend::default();
        b.add_slot_item("p", SLOT_MESSAGE_MENU, menu("x", vec![]))
            .unwrap();
        b.add_slot_item("p", SLOT_USER_MENU, menu("x", vec![]))
            .unwrap();
        b.add_slot_item(
            "p",
            SLOT_MESSAGE_MENU,
            menu("mine", vec![MenuPredicate::OwnMessage]),
        )
        .unwrap();
        b.add_slot_item("p", SLOT_MESSAGE_HOVER_BAR, button("hb"))
            .unwrap();
        let msgs = b.world.messages[&GENERAL].clone();
        let alices = Target::Message(message_ref(&msgs[0]));
        let yours = Target::Message(message_ref(&msgs[1]));
        let alice = Target::User((
            papi::UserRef {
                id: ALICE,
                name: "Ali".into(),
            },
            Some(LAB),
        ));

        // The same id in two slots: the target picks the slot, or the page names it.
        assert_eq!(
            resolve_slot(&b, "p", None, "x", &alices).unwrap(),
            SLOT_MESSAGE_MENU
        );
        assert_eq!(
            resolve_slot(&b, "p", None, "x", &alice).unwrap(),
            SLOT_USER_MENU
        );
        assert_eq!(
            resolve_slot(&b, "p", Some(SLOT_USER_MENU), "x", &alices)
                .unwrap_err()
                .status,
            400
        );
        assert_eq!(
            resolve_slot(&b, "p", Some(SLOT_CHANNEL_MENU), "x", &alices)
                .unwrap_err()
                .status,
            404
        );
        assert_eq!(
            resolve_slot(&b, "p", None, "x", &Target::None)
                .unwrap_err()
                .status,
            400
        );
        assert_eq!(
            resolve_slot(&b, "q", None, "x", &alices)
                .unwrap_err()
                .status,
            404
        );
        assert_eq!(
            resolve_slot(&b, "p", None, "hb", &yours).unwrap(),
            SLOT_MESSAGE_HOVER_BAR
        );

        // Predicates: an own-message item isn't offered on someone else's message.
        assert!(resolve_slot(&b, "p", None, "mine", &yours).is_ok());
        assert_eq!(
            resolve_slot(&b, "p", None, "mine", &alices)
                .unwrap_err()
                .status,
            400
        );

        // Targets must be loaded.
        let mut gone = message_ref(&msgs[0]);
        gone.id += 1000;
        assert_eq!(
            resolve_slot(&b, "p", None, "x", &Target::Message(gone))
                .unwrap_err()
                .status,
            400
        );
    }

    #[test]
    fn targets_must_exist_in_the_world() {
        let b = MockBackend::default();
        let w = &b.world;
        let m = &w.messages[&GENERAL][0];
        assert!(check_known_target(&b, &Target::Message(message_ref(m))).is_ok());
        let mut wrong = message_ref(m);
        wrong.author = BOT;
        assert!(check_known_target(&b, &Target::Message(wrong)).is_err());
        let general = w.channels.iter().find(|c| c.id == GENERAL).unwrap();
        assert!(check_known_target(&b, &Target::Channel(channel_ref(general))).is_ok());
        let renamed = papi::ChannelRef {
            name: "other".into(),
            ..channel_ref(general)
        };
        assert!(check_known_target(&b, &Target::Channel(renamed)).is_err());
        let you = papi::UserRef {
            id: ME,
            name: "You".into(),
        };
        assert!(check_known_target(&b, &Target::User((you.clone(), Some(LAB)))).is_ok());
        assert!(check_known_target(&b, &Target::User((you.clone(), Some(5)))).is_err());
        assert!(
            check_known_target(
                &b,
                &Target::User((
                    papi::UserRef {
                        id: 5,
                        name: "you".into()
                    },
                    None
                ))
            )
            .is_err()
        );
        assert!(
            check_known_target(
                &b,
                &Target::Guild(papi::GuildRef {
                    id: LAB,
                    name: "Rekindle Lab".into()
                })
            )
            .is_ok()
        );
        assert!(
            check_known_target(
                &b,
                &Target::Guild(papi::GuildRef {
                    id: LAB,
                    name: "Lab".into()
                })
            )
            .is_err()
        );
        assert!(
            check_known_target(
                &b,
                &Target::Role(papi::RoleRef {
                    id: 1100,
                    guild: LAB,
                    name: "Moderator".into()
                })
            )
            .is_ok()
        );
        assert!(
            check_known_target(
                &b,
                &Target::Role(papi::RoleRef {
                    id: 1100,
                    guild: GAMERS,
                    name: "Moderator".into()
                })
            )
            .is_err()
        );
        // Every slot's default target is one the client would offer.
        for (slot, _) in ALL_SLOTS {
            if let Some(t) = default_target(&b, slot) {
                assert!(check_known_target(&b, &t).is_ok(), "{slot}");
                assert_eq!(
                    rekindle_plugin_host::validate::target_kind(&t),
                    slot_target_kind(slot).unwrap(),
                    "{slot}"
                );
            }
        }
    }

    #[test]
    fn menu_predicates_follow_the_target() {
        use MenuPredicate::*;
        let b = MockBackend::default();
        let msgs = &b.world.messages[&GENERAL];
        let alices = facts(&b, &Target::Message(message_ref(&msgs[0])));
        assert!(
            alices.contains(&OthersMessage)
                && alices.contains(&TargetIsFriend)
                && alices.contains(&InGuild)
        );
        assert!(
            !alices.contains(&CanEdit) && alices.contains(&CanDelete),
            "you moderate the lab"
        );
        let yours = facts(&b, &Target::Message(message_ref(&msgs[1])));
        assert!(
            yours.contains(&OwnMessage)
                && yours.contains(&CanEdit)
                && yours.contains(&TargetIsSelf)
        );
        let bot = facts(&b, &Target::Message(message_ref(&msgs[2])));
        assert!(bot.contains(&TargetIsBot));
        let dm = facts(
            &b,
            &Target::Message(message_ref(&b.world.messages[&DM_ALICE][0])),
        );
        assert!(dm.contains(&InDm) && dm.contains(&CanPin));
    }

    fn status(r: Result<(), ApiError>) -> u16 {
        r.unwrap_err().status
    }

    #[test]
    fn ui_events_are_checked_against_the_panel_showing() {
        let mut b = MockBackend::default();
        b.show_panel("p", panel("main", vec![toggle("on")]));
        assert!(check_surface(&b, "p", &ui("main", "on", UiValue::Toggled(true))).is_ok());
        let wrong_kind = check_surface(&b, "p", &ui("main", "on", UiValue::Clicked));
        assert_eq!(status(wrong_kind), 400);
        let other_panel = check_surface(&b, "p", &ui("other", "on", UiValue::Toggled(true)));
        assert_eq!(status(other_panel), 404);
        let other_plugin = check_surface(&b, "q", &ui("main", "on", UiValue::Toggled(true)));
        assert_eq!(status(other_plugin), 404);
    }

    #[test]
    fn a_notice_has_only_its_button() {
        let mut b = MockBackend::default();
        let notice = |button: Option<&str>| UiOp::ShowNotice {
            notice: Notice {
                text: "n".into(),
                kind: ToastKind::Info,
                button: button.map(Into::into),
            },
        };
        let click = |element: &str| ui("notice", element, UiValue::Clicked);
        b.ui_update("p", notice(None)).unwrap();
        assert_eq!(status(check_surface(&b, "p", &click("button"))), 404);
        b.ui_update("p", notice(Some("Undo"))).unwrap();
        assert!(check_surface(&b, "p", &click("button")).is_ok());
        assert_eq!(status(check_surface(&b, "p", &click("other"))), 400);
        // A panel with a reserved name wins over the reserved surface.
        b.show_panel("p", panel("notice", vec![toggle("t")]));
        assert!(check_surface(&b, "p", &ui("notice", "t", UiValue::Toggled(true))).is_ok());
    }

    #[test]
    fn settings_extras_and_accessories_are_surfaces() {
        let mut b = MockBackend::default();
        let extra = vec![Element::Button {
            id: "export".into(),
            label: "Export".into(),
            icon: None,
            danger: false,
        }];
        let click = |surface: &str| ui(surface, "export", UiValue::Clicked);
        b.ui_update(
            "p",
            UiOp::SetSettingsExtra {
                elements: extra.clone(),
            },
        )
        .unwrap();
        assert!(check_surface(&b, "p", &click("settings-page")).is_ok());
        assert_eq!(
            status(check_surface(&b, "p", &click("settings"))),
            404,
            "`settings` is an ordinary panel id"
        );
        let m = message_ref(&b.world.messages[&GENERAL][0]);
        b.ui_update(
            "p",
            UiOp::SetMessageAccessory {
                message: m,
                elements: Some(extra),
            },
        )
        .unwrap();
        assert!(check_surface(&b, "p", &click(&format!("message:{}", m.id))).is_ok());
        assert_eq!(status(check_surface(&b, "p", &click("message:1"))), 404);
        assert_eq!(status(check_surface(&b, "p", &click("message:x"))), 400);
    }

    #[test]
    fn slash_options_are_typed_per_the_command() {
        let mut b = MockBackend::default();
        let opt = |name: &str, kind, required| papi::CommandOption {
            name: name.into(),
            description: String::new(),
            kind,
            required,
            choices: vec![("Twenty".into(), "20".into())],
            min: Some(1.0),
            max: Some(100.0),
            autocomplete: false,
        };
        let command = papi::SlashCommand {
            name: "roll".into(),
            description: "Roll".into(),
            options: vec![
                opt("sides", OptionKind::Integer, true),
                opt("who", OptionKind::User, false),
                opt("preset", OptionKind::Choice, false),
            ],
            in_guilds: true,
            in_dms: false,
        };
        b.ui_update("p", UiOp::RegisterSlashCommand { command })
            .unwrap();
        let given = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<_, _>>()
        };

        let inv = slash_invocation(
            &b,
            "p",
            "roll",
            &given(&[("sides", "20"), ("who", "101"), ("preset", "Twenty")]),
            None,
        )
        .unwrap();
        assert_eq!(inv.channel.id, GENERAL);
        assert_eq!(
            inv.options,
            vec![
                ("sides".to_string(), OptionValue::Integer(20)),
                ("who".to_string(), OptionValue::User(ALICE)),
                ("preset".to_string(), OptionValue::Choice("20".into()))
            ]
        );
        for (bad, why) in [
            (given(&[]), "required"),
            (given(&[("sides", "x")]), "not an integer"),
            (given(&[("sides", "500")]), "range"),
            (given(&[("sides", "2"), ("who", "9")]), "unknown user"),
            (given(&[("sides", "2"), ("preset", "7")]), "not a choice"),
            (given(&[("sides", "2"), ("extra", "1")]), "unknown option"),
        ] {
            assert_eq!(
                slash_invocation(&b, "p", "roll", &bad, None)
                    .unwrap_err()
                    .status,
                400,
                "{why}"
            );
        }
        assert_eq!(
            slash_invocation(&b, "p", "roll", &given(&[("sides", "2")]), Some(DM_ALICE))
                .unwrap_err()
                .status,
            400,
            "not in DMs"
        );
        assert_eq!(
            slash_invocation(&b, "p", "nope", &given(&[]), None)
                .unwrap_err()
                .status,
            404
        );
    }

    #[test]
    fn ui_values_are_checked_against_the_element() {
        let panel = Panel {
            id: "s".into(),
            title: "S".into(),
            elements: vec![
                Element::Heading { text: "h".into() },
                toggle("on"),
                Element::Slider {
                    id: "size".into(),
                    label: "Size".into(),
                    min: 0.0,
                    max: 10.0,
                    step: 1.0,
                    value: 5.0,
                },
                Element::Select {
                    id: "pick".into(),
                    label: "Pick".into(),
                    options: vec!["a".into(), "b".into()],
                    selected: 0,
                },
                Element::Button {
                    id: "go".into(),
                    label: "Go".into(),
                    icon: None,
                    danger: false,
                },
                Element::Table {
                    id: "t".into(),
                    label: "T".into(),
                    columns: vec!["a".into(), "b".into()],
                    rows: vec![],
                },
            ],
        };
        assert!(check_ui_event(&panel, "on", &UiValue::Toggled(true)).is_ok());
        assert!(check_ui_event(&panel, "size", &UiValue::Number(10.0)).is_ok());
        assert!(check_ui_event(&panel, "size", &UiValue::Number(11.0)).is_err());
        assert!(check_ui_event(&panel, "pick", &UiValue::Selected(1)).is_ok());
        assert!(check_ui_event(&panel, "pick", &UiValue::Selected(2)).is_err());
        assert!(check_ui_event(&panel, "go", &UiValue::Clicked).is_ok());
        assert!(check_ui_event(&panel, "go", &UiValue::Toggled(true)).is_err());
        let rows = |r: &[&[&str]]| {
            UiValue::Rows(
                r.iter()
                    .map(|row| row.iter().map(ToString::to_string).collect())
                    .collect(),
            )
        };
        assert!(check_ui_event(&panel, "t", &rows(&[&["1", "2"]])).is_ok());
        assert!(check_ui_event(&panel, "t", &rows(&[&["1"]])).is_err());
        assert!(check_ui_event(&panel, "missing", &UiValue::Clicked).is_err());
    }

    #[test]
    fn slot_targets_must_match_the_slot() {
        let section = SlotItem::ListItem {
            id: "s".into(),
            title: "S".into(),
        };
        let msg = Target::Message(papi::MessageRef {
            id: 1,
            channel: 2,
            guild: None,
            author: 3,
        });
        let user = Target::User((
            UserRef {
                id: 3,
                name: "Sam".into(),
            },
            None,
        ));
        let chan = Target::Channel(ChannelRef {
            id: 2,
            name: "general".into(),
            guild: Some(1),
            kind: 0,
        });
        let (menu, button) = (menu("m", vec![]), button("b"));
        assert!(check_slot_target(SLOT_MESSAGE_MENU, &menu, &msg).is_ok());
        assert!(check_slot_target(SLOT_USER_MENU, &menu, &user).is_ok());
        assert!(check_slot_target(SLOT_CHAT_BAR_BUTTON, &button, &chan).is_ok());
        assert!(check_slot_target(SLOT_GUILD_LIST_BUTTON, &button, &Target::None).is_ok());

        assert!(check_slot_target(SLOT_MESSAGE_MENU, &menu, &user).is_err());
        assert!(check_slot_target(SLOT_USER_MENU, &menu, &Target::None).is_err());
        assert!(check_slot_target(SLOT_CHAT_BAR_BUTTON, &button, &Target::None).is_err());
        assert!(check_slot_target(SLOT_PROFILE_SECTION, &section, &user).is_err());
    }
}
