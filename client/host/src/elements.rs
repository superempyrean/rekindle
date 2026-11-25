//! The client's own elements by name, as the engine's work.
//!
//! `wire.json`'s `elements` gives a selector for each name. What each
//! plugin did to them (`rekindle_plugin_host::model::NamedElements`) becomes
//! rules on the guest's nodes those selectors find:
//!
//! - `hide` drops the node and everything under it from the host's tree.
//! - `style` writes `background`, `color` and `tint` (the foreground),
//!   `borderColor`, `opacity` and `radius` on the node.
//! - `on-press` keeps a press on the node for the plugin ([`PRESS`]), and
//!   passes it to the guest when the plugin observes it, or retargets it
//!   at the first node another name's selector finds.
//! - `insert` mounts the plugin's elements as a panel's rows, before or
//!   after the node or inside it, last. Presses in them report as a
//!   panel's would ([`rows::INSERT`]).
//!
//! Where two plugins change the same element, the one loaded first wins for
//! a press, and the cells of both are written, the later plugin's last.

use std::collections::{BTreeMap, HashMap};

use rekindle_plugin_host::model::{Color, ElementStyle, NamedElements, Place, PressMode};
use rekindle_ui_hook::Widget;
use rekindle_wire::{NodeId, PropId, Value};
use rekindle_wire_inject::{
    Cell, Mount, PressAction, PressRule, RESERVED, RowAction, RowRule, Selector,
};

use crate::rows::{self, Look, Press};

/// The handler surface of a press on one of the client's elements a plugin
/// takes presses on. The element is the name.
pub(crate) const PRESS: &str = "element";

/// The first node id of the plugins' inserts. Each insert has a slot of two
/// blocks of [`BLOCK`] ids, so one of another shape goes under new ids while
/// the old one is taken out.
const INSERT_IDS: NodeId = RESERVED + 0x0800_0000;
const BLOCK: NodeId = 0x1000;
/// How many slots there are before they are handed out again.
const SLOTS: u32 = 0x400;

const OPACITY: PropId = PropId(322);
const RADIUS: PropId = PropId(388);

/// What the plugins' changes put on the wire.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Drawn {
    pub(crate) rows: Vec<RowRule>,
    pub(crate) presses: Vec<PressRule>,
    pub(crate) mounts: Vec<Mount>,
    /// What a press on a row of an insert reports.
    pub(crate) clicks: BTreeMap<(String, String), Press>,
}

/// Which ids each insert has, kept across renders.
#[derive(Debug, Default)]
pub(crate) struct Slots {
    /// By (plugin, insert id): its slot, which block it is in, and the
    /// mount it was last drawn as.
    by_insert: HashMap<(String, String), (u32, u32, Option<Mount>)>,
    next: u32,
}

impl Slots {
    /// The first id of the block `key`'s insert goes in, given the mount it
    /// is drawn as there. A mount of another shape than the last one moves
    /// to the slot's other block.
    fn place(&mut self, key: &(String, String), mut draw: impl FnMut(NodeId) -> Mount) -> Mount {
        let next = &mut self.next;
        let (slot, block, last) = self.by_insert.entry(key.clone()).or_insert_with(|| {
            let slot = *next;
            *next = (*next + 1) % SLOTS;
            (slot, 0, None)
        });
        let first = |block: u32| INSERT_IDS + (*slot * 2 + block) * BLOCK;
        let mut mount = draw(first(*block));
        if let Some(old) = last.as_ref()
            && !old.same_shape(&mount)
        {
            *block ^= 1;
            mount = draw(first(*block));
        }
        *last = Some(mount.clone());
        mount
    }

    /// Forget the inserts that aren't in `keep`.
    fn retain(&mut self, keep: &[(String, String)]) {
        self.by_insert.retain(|k, _| keep.contains(k));
    }
}

fn rgba(c: Color) -> Value {
    rekindle_wire_inject::rgba(u32::from_be_bytes([c.r, c.g, c.b, c.a]))
}

/// The cells a style writes, `None` for one that writes none.
fn style_cells(style: &ElementStyle) -> Option<Vec<Cell>> {
    use rekindle_wire::ids;
    let mut cells = Vec::new();
    if let Some(c) = style.background {
        cells.push(Cell::new(ids::BACKGROUND, rgba(c)));
    }
    if let Some(c) = style.foreground {
        cells.push(Cell::new(ids::COLOR, rgba(c)));
        cells.push(Cell::new(ids::TINT, rgba(c)));
    }
    if let Some(c) = style.border {
        cells.push(Cell::new(ids::BORDER_COLOR, rgba(c)));
    }
    if let Some(o) = style.opacity {
        cells.push(Cell::new(OPACITY, Value::F64(o.into())));
    }
    if let Some(r) = style.radius {
        cells.push(Cell::new(RADIUS, Value::F64(r.into())));
    }
    (!cells.is_empty()).then_some(cells)
}

/// The work for what the plugins did to the elements `map` names. Inserts
/// need `look` to be drawn, and `widgets` makes their rows.
pub(crate) fn draw(
    state: &NamedElements,
    map: &BTreeMap<String, Selector>,
    look: Option<&Look>,
    widgets: impl Fn(&[rekindle_plugin_host::model::Element]) -> Vec<Widget>,
    slots: &mut Slots,
) -> Drawn {
    let mut out = Drawn::default();
    let mut drawn = Vec::new();
    for (plugin, mine) in &state.plugins {
        for name in &mine.hidden {
            if let Some(select) = map.get(name) {
                out.rows.push(RowRule {
                    select: select.clone(),
                    action: RowAction::Drop,
                });
            }
        }
        for (name, style) in &mine.styles {
            if let (Some(select), Some(cells)) = (map.get(name), style_cells(style)) {
                out.rows.push(RowRule {
                    select: select.clone(),
                    action: RowAction::Cells(cells),
                });
            }
        }
        for (name, mode) in &mine.presses {
            let Some(select) = map.get(name) else {
                continue;
            };
            let take = |forward: bool| PressAction::Plugin {
                plugin: plugin.clone(),
                surface: PRESS.into(),
                element: name.clone(),
                forward,
            };
            let action = match mode {
                PressMode::Observe => take(true),
                PressMode::Consume => take(false),
                PressMode::Retarget(to) => match map.get(to) {
                    Some(to) => PressAction::Retarget(to.clone()),
                    None => continue,
                },
            };
            out.presses.push(PressRule {
                select: select.clone(),
                action,
            });
        }
        let Some(look) = look else {
            continue;
        };
        for insert in &mine.inserts {
            let Some(select) = map.get(&insert.name) else {
                continue;
            };
            let at = match insert.place {
                Place::Before => (select.clone().parent_of(), Some(select.clone()), None),
                Place::After => (select.clone().parent_of(), None, Some(select.clone())),
                Place::Inside => (select.clone(), None, None),
            };
            let rows = widgets(&insert.elements);
            let key = (plugin.clone(), insert.id.clone());
            let mut clicks = BTreeMap::new();
            let mount = slots.place(&key, |first| {
                let r = rows::insert(look, plugin, &insert.id, &rows, at.clone(), first);
                clicks = r.presses;
                r.mounts.into_iter().next().expect("an insert is one mount")
            });
            out.clicks.extend(clicks);
            out.mounts.push(mount);
            drawn.push(key);
        }
    }
    slots.retain(&drawn);
    out
}

/// The first node id of the `wire-raw` plugins' own work: a block of
/// `RAW_IDS` each, by the plugin's slot.
const RAW_BLOCKS: NodeId = RESERVED + 0x0900_0000;
/// How many plugins' blocks there are before they are handed out again.
const RAW_SLOTS: u32 = 0x60;

/// Which block of ids each `wire-raw` plugin's work has, kept across
/// renders, so a plugin that loads or unloads doesn't move another's.
#[derive(Debug, Default)]
pub(crate) struct RawSlots {
    by_plugin: HashMap<String, u32>,
    next: u32,
}

/// The `wire-raw` plugins' own work, as one, each plugin's mounts moved
/// from `RESERVED` to its own block.
pub(crate) fn raw(state: &NamedElements, slots: &mut RawSlots) -> rekindle_wire_inject::Work {
    use rekindle_plugin_host::model::RAW_IDS;
    use rekindle_wire_inject::Handler;
    slots.by_plugin.retain(|p, _| state.raw.contains_key(p));
    let mut out = rekindle_wire_inject::Work::default();
    for (plugin, work) in &state.raw {
        let next = &mut slots.next;
        let slot = *slots.by_plugin.entry(plugin.clone()).or_insert_with(|| {
            let s = *next;
            *next = (*next + 1) % RAW_SLOTS;
            s
        });
        let shift = RAW_BLOCKS + slot * RAW_IDS - RESERVED;
        let mut work = work.clone();
        for m in &mut work.mounts {
            m.root += shift;
            for n in &mut m.nodes {
                n.id += shift;
                for c in &mut n.children {
                    *c += shift;
                }
                if let Some(Handler::Count { node, .. }) = &mut n.on_press {
                    *node += shift;
                }
            }
        }
        out.mounts.extend(work.mounts);
        out.cells.extend(work.cells);
        out.rows.extend(work.rows);
        out.presses.extend(work.presses);
        out.labels.extend(work.labels);
        out.recolors.extend(work.recolors);
        out.prop_recolors.extend(work.prop_recolors);
    }
    out
}

/// The click a press the `wire-raw` plugins' work kept reports: `clicked`
/// on `wire-raw`, the handler's element as the element.
pub(crate) fn raw_click(plugin: &str, element: &str) -> rekindle_ui_hook::Click {
    rekindle_ui_hook::Click::Ui {
        plugin: plugin.into(),
        panel: rekindle_plugin_host::model::RAW_SURFACE.into(),
        element: element.into(),
        value: rekindle_ui_hook::UiInput::Clicked,
    }
}

/// The click a press on one of the client's elements reports: `clicked`
/// on `element:<name>`, the name as the element.
pub(crate) fn click(plugin: &str, name: &str) -> rekindle_ui_hook::Click {
    rekindle_ui_hook::Click::Ui {
        plugin: plugin.into(),
        panel: format!("element:{name}"),
        element: name.into(),
        value: rekindle_ui_hook::UiInput::Clicked,
    }
}

#[cfg(test)]
mod tests {
    use rekindle_plugin_host::model::{Element, ElementInsert, ElementOp};
    use rekindle_wire::a11y::ids as a11y;

    use super::*;

    fn map() -> BTreeMap<String, Selector> {
        BTreeMap::from([
            ("composer.gift".into(), Selector::cell(a11y::ID, "composer.gift")),
            (
                "guild.search".into(),
                Selector::cell(a11y::ID, "guild-search").nth(0),
            ),
        ])
    }

    fn state(ops: Vec<(&str, ElementOp)>) -> NamedElements {
        let mut s = NamedElements::new(map().into_keys().collect());
        for (plugin, op) in ops {
            s.apply(plugin, op).unwrap();
        }
        s
    }

    #[test]
    fn hide_style_and_presses_become_rules() {
        let s = state(vec![
            ("p", ElementOp::Hide { name: "composer.gift".into() }),
            (
                "p",
                ElementOp::Style {
                    name: "guild.search".into(),
                    style: ElementStyle {
                        foreground: Some(Color { r: 255, g: 0, b: 0, a: 255 }),
                        opacity: Some(0.5),
                        ..ElementStyle::default()
                    },
                },
            ),
            (
                "p",
                ElementOp::OnPress {
                    name: "guild.search".into(),
                    mode: PressMode::Consume,
                },
            ),
            (
                "q",
                ElementOp::OnPress {
                    name: "composer.gift".into(),
                    mode: PressMode::Retarget("guild.search".into()),
                },
            ),
        ]);
        let d = draw(&s, &map(), None, |_| Vec::new(), &mut Slots::default());
        assert_eq!(d.rows.len(), 2);
        assert_eq!(d.rows[0].action, RowAction::Drop);
        let RowAction::Cells(cells) = &d.rows[1].action else {
            panic!("{:?}", d.rows[1]);
        };
        assert_eq!(cells.len(), 3, "color, tint and opacity");
        assert_eq!(
            d.presses[0].action,
            PressAction::Plugin {
                plugin: "p".into(),
                surface: PRESS.into(),
                element: "guild.search".into(),
                forward: false,
            }
        );
        assert_eq!(
            d.presses[1].action,
            PressAction::Retarget(map()["guild.search"].clone())
        );
        assert!(d.mounts.is_empty());
    }

    #[test]
    fn raw_work_moves_to_its_plugins_block() {
        let mut s = NamedElements::with_selectors(BTreeMap::new());
        let json = format!(
            r#"{{"mounts": [{{"under": {{"kind": "view"}}, "root": {r}, "nodes": [
                {{"id": {r}, "kind": "view", "children": [{c}]}},
                {{"id": {c}, "kind": "text"}}]}}]}}"#,
            r = RESERVED,
            c = RESERVED + 1
        );
        for p in ["a", "b"] {
            let w = rekindle_plugin_host::model::raw_work(p, &json).unwrap();
            s.set_raw(p, Some(w)).unwrap();
        }
        let mut slots = RawSlots::default();
        let w = raw(&s, &mut slots);
        assert_eq!(w.mounts.len(), 2);
        assert_eq!(w.mounts[0].root, RAW_BLOCKS);
        assert_eq!(w.mounts[0].nodes[0].children, [RAW_BLOCKS + 1]);
        assert_eq!(w.mounts[1].root, RAW_BLOCKS + rekindle_plugin_host::model::RAW_IDS);
        assert!(w.check().is_ok());
        // `a` unloading leaves `b` where it was.
        s.plugin_unloaded("a");
        let w = raw(&s, &mut slots);
        assert_eq!(w.mounts[0].root, RAW_BLOCKS + rekindle_plugin_host::model::RAW_IDS);
    }

    #[test]
    fn click_is_on_the_elements_surface() {
        let rekindle_ui_hook::Click::Ui { panel, element, .. } = click("p", "composer.gift")
        else {
            panic!()
        };
        assert_eq!((panel.as_str(), element.as_str()), ("element:composer.gift", "composer.gift"));
        let rekindle_ui_hook::Click::Ui { panel, element, .. } = raw_click("p", "go") else {
            panic!()
        };
        assert_eq!((panel.as_str(), element.as_str()), ("wire-raw", "go"));
    }

    #[test]
    fn an_insert_goes_next_to_its_element_and_keeps_its_ids() {
        let insert = |elements: Vec<Element>| {
            (
                "p",
                ElementOp::Insert(ElementInsert {
                    id: "note".into(),
                    name: "guild.search".into(),
                    place: Place::After,
                    elements,
                }),
            )
        };
        let look = crate::rows::test_look();
        let widgets = |es: &[Element]| {
            es.iter()
                .map(|e| match e {
                    Element::Button { id, label, .. } => Widget::Button {
                        id: id.clone(),
                        label: label.clone(),
                    },
                    _ => Widget::Separator,
                })
                .collect()
        };
        let button = |id: &str| Element::Button {
            id: id.into(),
            label: "Go".into(),
            icon: None,
            danger: false,
        };
        let mut slots = Slots::default();
        let s = state(vec![insert(vec![button("go")])]);
        let d = draw(&s, &map(), Some(&look), widgets, &mut slots);
        let m = &d.mounts[0];
        assert_eq!(m.under, map()["guild.search"].clone().parent_of());
        assert_eq!(m.after, Some(map()["guild.search"].clone()));
        assert_eq!(m.root, INSERT_IDS + m.nodes.len() as u32 - 1);
        let Some(Press::Ui { panel, element, .. }) =
            d.clicks.get(&(rows::INSERT.to_string(), "p/note/go".to_string()))
        else {
            panic!("{:?}", d.clicks);
        };
        assert_eq!((panel.as_str(), element.as_str()), ("insert:note", "go"));

        // The same shape keeps its ids, and another moves to the other block.
        let again = draw(&s, &map(), Some(&look), widgets, &mut slots);
        assert_eq!(again.mounts[0].root, m.root);
        let s = state(vec![insert(vec![button("go"), button("stop")])]);
        let two = draw(&s, &map(), Some(&look), widgets, &mut slots);
        assert!(two.mounts[0].nodes.iter().all(|n| n.id >= INSERT_IDS + BLOCK));
        assert!(draw(&s, &map(), None, widgets, &mut slots).mounts.is_empty());
    }
}
