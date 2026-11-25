//! The plugins' rows on the guest's context menus.
//!
//! `wire.json`'s `menus` says, for each menu slot, which events open the
//! menu and where the id of what it is for sits: under the row of a
//! `rowLongPress`, or at or under the node of a `longPress` or an
//! `accessibilityAction`. The menu the latest such event opened is the one
//! shown, and only its rows are mounted. Several of the guest's sheets are
//! drawn alike, so a menu's `under` alone can't tell them apart. The menu
//! stays shown until its rows' node has come and gone, or another menu
//! opens.
//!
//! A press on a plugin's row reaches the plugin with the menu's target. A
//! user's menu opened from the name on a message is for the message's
//! author, read from the client's store.

use std::sync::Mutex;

use rekindle_plugin_host::model::{
    SLOT_CHANNEL_MENU, SLOT_GUILD_MENU, SLOT_MESSAGE_MENU, SLOT_USER_MENU,
};
use rekindle_ui_hook::MenuTargetRef;
use rekindle_wire::{NodeId, PropId, Tree, Value};
use rekindle_wire_inject::Engine;

use crate::wire::{MenuMap, TargetIs};

/// The menu on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Shown {
    /// Its place among `wire.json`'s menus, as its mount's root counts it.
    nth: u32,
    slot: String,
    /// The id after its target prefix.
    id: u64,
    /// Whether its rows' node has been in the guest's tree.
    seen: bool,
}

static SHOWN: Mutex<Option<Shown>> = Mutex::new(None);
/// The place of the latest event already looked at, so an event opens a
/// menu once.
static LOOKED: Mutex<u64> = Mutex::new(0);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn menus() -> impl Iterator<Item = (u32, &'static String, &'static MenuMap)> {
    super::wire_map()
        .into_iter()
        .flat_map(|m| m.menus.iter())
        .zip(0u32..)
        .map(|((slot, map), nth)| (nth, slot, map))
}

/// After an event reached the engine: the menu it opened, if it opened
/// one. True when the menu shown changed.
pub(crate) fn opened(e: &Engine) -> bool {
    let latest = menus()
        .filter_map(|(nth, slot, map)| {
            let (at, id) = event_id(e, map)?;
            Some((at, nth, slot, id))
        })
        .max_by_key(|(at, ..)| *at);
    let Some((at, nth, slot, id)) = latest else {
        return false;
    };
    let mut looked = lock(&LOOKED);
    if at <= *looked {
        return false;
    }
    *looked = at;
    let now = Shown {
        nth,
        slot: slot.clone(),
        id,
        seen: false,
    };
    super::log(&format!("menu {slot}: opened"));
    lock(&SHOWN).replace(now.clone()) != Some(now)
}

/// After a turn: whether the menu shown has closed. True when it has.
pub(crate) fn after_turn(e: &Engine) -> bool {
    let mut shown = lock(&SHOWN);
    let Some(s) = shown.as_mut() else {
        return false;
    };
    let Some((_, _, map)) = menus().find(|(nth, ..)| *nth == s.nth) else {
        return false;
    };
    let present = e.find(&map.under).is_some();
    if present {
        s.seen = true;
        return false;
    }
    if s.seen {
        *shown = None;
        return true;
    }
    false
}

/// Whether the mount with `root` is shown: a menu's rows only while it is
/// the menu on screen.
pub(crate) fn shown(root: NodeId) -> bool {
    match crate::wire::menu_nth(root) {
        Some(nth) => lock(&SHOWN).as_ref().is_some_and(|s| s.nth == nth),
        None => true,
    }
}

/// What a press on a row of the menu on `slot` is about. Reading a
/// message's author reads the client's store, so no engine lock may be
/// held.
pub(crate) fn target(slot: &str) -> Option<MenuTargetRef> {
    let s = lock(&SHOWN).clone().filter(|s| s.slot == slot)?;
    let (_, _, map) = menus().find(|(nth, ..)| *nth == s.nth)?;
    let id = match map.target_is {
        TargetIs::Itself => s.id,
        TargetIs::MessageAuthor => {
            let store = rekindle_plugin_host::backend::cache::CacheStore::new(Box::new(
                super::database::GuestStore,
            ));
            store.message_in_selected(s.id)?.author.id
        }
    };
    match slot {
        SLOT_MESSAGE_MENU => Some(MenuTargetRef::Message {
            id,
            // Read from the store once the press reaches the runtime.
            channel: 0,
            author: 0,
        }),
        SLOT_USER_MENU => Some(MenuTargetRef::User { id }),
        SLOT_CHANNEL_MENU => Some(MenuTargetRef::Channel { id }),
        SLOT_GUILD_MENU => Some(MenuTargetRef::Guild { id }),
        _ => None,
    }
}

/// The latest event that opens `map`'s menu, by its place among all
/// events, and the id it carries.
fn event_id(e: &Engine, map: &MenuMap) -> Option<(u64, u64)> {
    let names: Vec<PropId> = map
        .opened_by
        .iter()
        .filter_map(|n| PropId::from_name(n))
        .collect();
    let (at, ev) = e.last_of(&names)?;
    let tree = e.guest();
    let node = match index(&ev.value) {
        // A list's row: the list's children start at its `first` row.
        Some(index) => {
            let first = number(tree.props().get(ev.node, PropId::from_name("first")?)).unwrap_or(0);
            *tree
                .node(ev.node)?
                .children
                .get(usize::try_from(index - first).ok()?)?
        }
        None => ev.node,
    };
    Some((at, id_under(tree, node, &map.target_prefix)?))
}

fn number(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::I32(n) => Some(i64::from(*n)),
        Value::I64(n) => Some(*n),
        #[expect(clippy::cast_possible_truncation, reason = "a row index")]
        Value::F64(f) => Some(*f as i64),
        _ => None,
    }
}

/// The `index` of a row event's value.
fn index(v: &Value) -> Option<i64> {
    let field = match v {
        Value::Object(props) => {
            let id = PropId::from_name("index")?;
            props.iter().find(|(p, _)| *p == id).map(|(_, v)| v)
        }
        Value::Map(entries) => entries.iter().find(|(k, _)| k == "index").map(|(_, v)| v),
        _ => None,
    };
    number(field)
}

/// The id after `prefix` in an `accessibilityId` at or under `node`.
fn id_under(tree: &Tree, node: NodeId, prefix: &str) -> Option<u64> {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if let Some(Value::Str(aid)) = tree.props().get(n, rekindle_wire::a11y::ids::ID)
            && let Some(id) = aid.strip_prefix(prefix).and_then(|i| i.parse().ok())
        {
            return Some(id);
        }
        stack.extend(
            tree.node(n)
                .map(|x| x.children.iter().copied())
                .into_iter()
                .flatten(),
        );
    }
    None
}
