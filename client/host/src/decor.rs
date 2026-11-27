//! Plugins' keyed decorations and message accessories on a client's wire.
//!
//! The shared model (`rekindle_ui_hook::decor`) holds what the plugins set.
//! Here each item becomes a mount of the engine's: a decoration is a `text`
//! at the end of the node its key names, and an accessory a column of
//! `text`s after the message's content, in the content's parent.
//!
//! The wire map says where each kind's nodes are, as the `accessibilityId`
//! the guest gives them before the key (`navigation.channel:<channel>`), and
//! messages by their content's prefix ([`crate::messages::MessageMap`]).
//! The host offers the plugins the kinds the map names, and accessories when
//! it has the messages, before they start ([`offer`]). Nothing here names
//! the client.

use std::collections::BTreeMap;
use std::sync::Mutex;

use rekindle_ui_hook::decor::{AccessoryItem, DecorationItem};
use rekindle_wire::a11y::ids as a11y;
use rekindle_wire::{ElementKind, NodeId, Value};
use rekindle_wire_inject::{Cell, Mount, MountNode, RESERVED, Selector, TEXT};
use serde::Deserialize;

/// Where one kind's nodes are, in the wire map's `decorations`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KindMap {
    /// The `accessibilityId` of the node a key names, before the key.
    pub(crate) id_prefix: String,
}

/// By kind (`channel-list.trailing`).
pub(crate) type DecorMap = BTreeMap<String, KindMap>;

/// The first id of the decorations' nodes, and how many ids one item may
/// take.
const DECOR_IDS: NodeId = RESERVED + 0x0A00_0000;
const ITEM: NodeId = 0x40;
const ITEMS: NodeId = 0x0100_0000 / ITEM;
/// An accessory's lines past this many are left out.
const MAX_LINES: usize = ITEM as usize - 1;

/// Each item's first id, kept while the process runs, so an item keeps its
/// nodes from one publish to the next.
static IDS: Mutex<BTreeMap<String, NodeId>> = Mutex::new(BTreeMap::new());

fn ids_for(key: String) -> Option<NodeId> {
    let mut ids = IDS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(id) = ids.get(&key) {
        return Some(*id);
    }
    let n = NodeId::try_from(ids.len()).ok().filter(|n| *n < ITEMS)?;
    let id = DECOR_IDS + n * ITEM;
    ids.insert(key, id);
    Some(id)
}

/// Tell the model which kinds this map draws, and whether accessories are
/// drawn. Call it before the plugins start.
pub(crate) fn offer(map: &DecorMap, messages: bool) {
    rekindle_ui_hook::decor::set_decoration_kinds(map.keys().cloned().collect());
    rekindle_ui_hook::decor::set_accessories_drawn(messages);
}

/// A `text` node with `text`, in `color` when it has one.
fn text(id: NodeId, text: &str, color: Option<u32>) -> MountNode {
    let mut cells = vec![Cell::new(TEXT, Value::Str(text.into()))];
    if let Some(c) = color {
        cells.push(Cell::new(
            rekindle_wire::ids::COLOR,
            rekindle_wire_inject::rgba(c),
        ));
    }
    MountNode {
        id,
        kind: ElementKind::TEXT,
        cells,
        children: Vec::new(),
        on_press: None,
    }
}

/// The mounts of `decorations` the map has a kind for, and of
/// `accessories` when `content_prefix` says where the messages are.
pub(crate) fn mounts(
    map: &DecorMap,
    content_prefix: Option<&str>,
    decorations: &[DecorationItem],
    accessories: &[AccessoryItem],
) -> Vec<Mount> {
    let mut out = Vec::new();
    for d in decorations {
        let Some(kind) = map.get(&d.kind) else {
            continue;
        };
        let Some(root) = ids_for(format!(
            "d\u{1f}{}\u{1f}{}\u{1f}{}",
            d.plugin, d.kind, d.key
        )) else {
            continue;
        };
        out.push(Mount {
            under: Selector::cell(a11y::ID, &format!("{}{}", kind.id_prefix, d.key)),
            after: None,
            before: None,
            root,
            nodes: vec![text(root, &d.text, Some(d.color).filter(|c| *c != 0))],
        });
    }
    let Some(prefix) = content_prefix else {
        return out;
    };
    for a in accessories {
        if a.lines.is_empty() {
            continue;
        }
        let Some(root) = ids_for(format!("a\u{1f}{}\u{1f}{}", a.plugin, a.message)) else {
            continue;
        };
        let content = Selector::cell(a11y::ID, &format!("{prefix}{}", a.message));
        let lines: Vec<MountNode> = (1..)
            .zip(a.lines.iter().take(MAX_LINES))
            .map(|(n, line)| text(root + n, line, None))
            .collect();
        let mut nodes = vec![MountNode {
            id: root,
            kind: ElementKind::VIEW,
            cells: vec![Cell::new(
                a11y::ID,
                Value::Str(format!("rekindle.accessory.{}:{}", a.plugin, a.message)),
            )],
            children: lines.iter().map(|l| l.id).collect(),
            on_press: None,
        }];
        nodes.extend(lines);
        out.push(Mount {
            under: content.clone().parent_of(),
            after: Some(content),
            before: None,
            root,
            nodes,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> DecorMap {
        serde_json::from_str(
            r#"{"channel-list.trailing": {"id_prefix": "navigation.channel:"},
                "member-list.trailing": {"id_prefix": "channel.details.member:"}}"#,
        )
        .unwrap()
    }

    fn decoration(kind: &str, key: u64, text: &str) -> DecorationItem {
        DecorationItem {
            plugin: "dev.test.decor".into(),
            kind: kind.into(),
            key,
            text: text.into(),
            tooltip: String::new(),
            color: 0x23a55aff,
        }
    }

    #[test]
    fn a_decoration_goes_at_the_end_of_its_keys_node() {
        let d = [
            decoration("channel-list.trailing", 42, "3"),
            decoration("user.name-badge", 7, "x"),
        ];
        let m = mounts(&map(), None, &d, &[]);
        assert_eq!(m.len(), 1, "a kind the map lacks is left out");
        assert_eq!(
            m[0].under,
            Selector::cell(a11y::ID, "navigation.channel:42")
        );
        assert_eq!(m[0].nodes.len(), 1);
        assert_eq!(m[0].nodes[0].kind, ElementKind::TEXT);
        assert_eq!(
            m[0].nodes[0].cells[0],
            Cell::new(TEXT, Value::Str("3".into()))
        );
        assert_eq!(m[0].nodes[0].cells.len(), 2);
        let again = mounts(&map(), None, &d, &[]);
        assert_eq!(again[0].root, m[0].root, "an item keeps its ids");
    }

    #[test]
    fn an_accessory_follows_its_messages_content() {
        let a = AccessoryItem {
            plugin: "dev.test.log".into(),
            message: 99,
            channel: 5,
            lines: vec!["Edited 1 time(s). Before:".into(), "hello".into()],
        };
        let m = mounts(
            &map(),
            Some("message.content:"),
            &[],
            std::slice::from_ref(&a),
        );
        let [mount] = &m[..] else { panic!("{m:?}") };
        let content = Selector::cell(a11y::ID, "message.content:99");
        assert_eq!(mount.after.as_ref(), Some(&content));
        assert_eq!(mount.under, content.parent_of());
        assert_eq!(mount.nodes.len(), 3);
        assert_eq!(mount.nodes[0].children, [mount.root + 1, mount.root + 2]);
        assert!(mounts(&map(), None, &[], &[a]).is_empty());
    }

    #[test]
    fn offered_kinds_are_the_maps() {
        offer(&map(), true);
        assert_eq!(
            rekindle_ui_hook::decor::decoration_kinds(),
            ["channel-list.trailing", "member-list.trailing"]
        );
        assert!(rekindle_ui_hook::decor::accessories_drawn());
    }
}
