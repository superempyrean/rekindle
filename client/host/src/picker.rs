//! The plugins' tabs in the client's emoji picker, on its wire.
//!
//! The picker the guest draws has a row of tabs (Emoji, GIFs, Stickers)
//! above a search field and a body. Each plugin's tab goes at the end of
//! that row, built like the guest's own tabs. A press on it shows the tab:
//! the search field and the body are dropped from the host's tree, and the
//! plugin's panel with the tab's id goes in their place. A press on one of
//! the guest's tabs reaches the guest as before, and takes the plugin's tab
//! down again. So does the picker closing.
//!
//! The panel's buttons become a grid, its select (or tabs) a row of chips,
//! and its text and headings labels between them. A press on a button or a
//! chip reports what the same control reports in any other panel. Text
//! fields and the other controls aren't shown, since nothing here takes
//! typed text.
//!
//! The text colours and the selected tab's fill are read off the guest's
//! own tabs, so the plugin's tab follows the client's theme. The rest of
//! the look is in [`PickerMap`].

use std::collections::BTreeMap;

use rekindle_plugin_host::model::SLOT_EXPRESSION_PICKER_TAB;
use rekindle_ui_hook::{PanelItem, UiInput, Widget};
use rekindle_wire::a11y::{Role, ids as a11y};
use rekindle_wire::{ElementKind, NodeId, PropId, Tree, Value};
use rekindle_wire_inject::{
    Cell, Handler, Mount, MountNode, PressAction, PressRule, RESERVED, RowAction, RowRule,
    Selector, TEXT,
};
use serde::Deserialize;

use crate::rows::Press;

/// The handler surface of a plugin's tab.
pub(crate) const TAB: &str = SLOT_EXPRESSION_PICKER_TAB;
/// The handler surface of a button or chip in a tab's body.
pub(crate) const BODY: &str = "expression-picker.body";
/// The handler surface of a press on one of the guest's own tabs while a
/// plugin's tab is shown.
pub(crate) const GUEST_TAB: &str = "expression-picker.guest-tab";

/// The first node id of the plugins' tabs.
const TAB_IDS: NodeId = RESERVED + 0x0700_0000;
/// The spacer that keeps the guest's selection under its own tab.
const SPACER: NodeId = TAB_IDS + 0xffff;
/// The first node ids of a tab's body: two blocks, so a body of another
/// shape is created under new ids while the old one is taken out.
const BODY_IDS: [NodeId; 2] = [RESERVED + 0x0710_0000, RESERVED + 0x0718_0000];

/// The `flex` cell.
fn flex() -> PropId {
    PropId::from_name("flex").expect("the PropId table has `flex`")
}

/// The `accessibilitySelected` cell.
fn selected() -> PropId {
    PropId::from_name("accessibilitySelected")
        .expect("the PropId table has `accessibilitySelected`")
}

/// Where a build's guest draws its emoji picker, and the cells of what
/// rekindle draws in it, as `wire.json` holds it under `expression_picker`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PickerMap {
    /// Each of the guest's tabs. Their parent is the row the plugins' tabs
    /// go in, last.
    pub(crate) tab: Selector,
    /// The column holding the tab strip, the search field and the body.
    pub(crate) content: Selector,
    /// The child of `content` that holds the tabs. The rest of `content`
    /// is dropped while a plugin's tab is shown.
    pub(crate) strip: Selector,
    /// The row that draws the selection behind the selected tab, with a
    /// spacer, the selection's fill and a spacer, by `flex`. It is dropped
    /// while a plugin's tab is shown, and has a spacer of rekindle's
    /// otherwise, so the guest's selection stays under the guest's tab.
    #[serde(default)]
    pub(crate) indicator: Option<Selector>,
    /// A tab, and what it adds when it is shown. The fill comes from the
    /// guest's selection.
    #[serde(default)]
    pub(crate) tab_cells: Vec<Cell>,
    #[serde(default)]
    pub(crate) tab_on: Vec<Cell>,
    /// A tab's title. Its colour comes from the guest's tabs.
    #[serde(default)]
    pub(crate) tab_text: Vec<Cell>,
    /// The body, which fills what's left of `content`.
    #[serde(default)]
    pub(crate) body: Vec<Cell>,
    /// The row of chips a select becomes, a chip, what a chip adds when
    /// selected, and its text.
    #[serde(default)]
    pub(crate) chips: Vec<Cell>,
    #[serde(default)]
    pub(crate) chip: Vec<Cell>,
    #[serde(default)]
    pub(crate) chip_on: Vec<Cell>,
    #[serde(default)]
    pub(crate) chip_text: Vec<Cell>,
    /// The scroll holding the grid, and the column in it.
    #[serde(default)]
    pub(crate) scroll: Vec<Cell>,
    #[serde(default)]
    pub(crate) column: Vec<Cell>,
    /// A label for a panel's text or heading.
    #[serde(default)]
    pub(crate) label: Vec<Cell>,
    /// How many buttons a row of the grid holds. 3 when not given.
    #[serde(default)]
    pub(crate) columns: Option<usize>,
    /// A row of the grid, a button and its text.
    #[serde(default)]
    pub(crate) row: Vec<Cell>,
    #[serde(default)]
    pub(crate) cell: Vec<Cell>,
    #[serde(default)]
    pub(crate) cell_text: Vec<Cell>,
}

/// The colours of the guest's tabs as it draws them now: the selected
/// tab's text, another tab's text, and the selection's fill.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Colors {
    pub(crate) on: Option<Value>,
    pub(crate) off: Option<Value>,
    pub(crate) fill: Option<Value>,
}

/// The picker as the guest's tree has it: `None` while it isn't open. The
/// colours are right only in the turn that opened it: the guest animates
/// its tabs' text from a worklet, and writes their `color` cells only when
/// it creates them.
pub(crate) fn read(
    guest: &Tree,
    map: &PickerMap,
    find: impl Fn(&Selector) -> Option<NodeId>,
) -> Option<Colors> {
    find(&map.tab)?;
    let color = |sel: Selector| {
        let text = find(&Selector::All(vec![
            Selector::Kind(ElementKind::TEXT),
            sel.within(),
        ]))?;
        guest.props().get(text, rekindle_wire::ids::COLOR).cloned()
    };
    let tab_now = |on: bool| {
        Selector::All(vec![
            map.tab.clone(),
            Selector::Cell {
                prop: selected(),
                value: Value::Bool(on),
            },
        ])
    };
    let fill = map.indicator.as_ref().and_then(|ind| {
        let mut stack = vec![find(ind)?];
        while let Some(n) = stack.pop() {
            if let Some(v) = guest.props().get(n, rekindle_wire::ids::BACKGROUND) {
                return Some(v.clone());
            }
            stack.extend(guest.node(n)?.children.iter().rev());
        }
        None
    });
    Some(Colors {
        on: color(tab_now(true)),
        off: color(tab_now(false)),
        fill,
    })
}

/// A plugin's tab, as the host draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tab {
    pub(crate) plugin: String,
    pub(crate) id: String,
    pub(crate) title: String,
}

/// What the plugins' tabs put on the wire, and what a press in a tab's
/// body reports.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Drawn {
    pub(crate) mounts: Vec<Mount>,
    pub(crate) rows: Vec<RowRule>,
    pub(crate) presses: Vec<PressRule>,
    pub(crate) clicks: BTreeMap<(String, String), Press>,
}

/// The plugins' `tabs` in the picker, with `shown` (an index into `tabs`)
/// shown with `panel` as its body. `block` picks the body's ids.
pub(crate) fn draw(
    map: &PickerMap,
    colors: &Colors,
    tabs: &[Tab],
    shown: Option<(usize, Option<&PanelItem>)>,
    block: usize,
) -> Drawn {
    let mut out = Drawn::default();
    if tabs.is_empty() {
        return out;
    }
    for (i, t) in (0u32..).zip(tabs) {
        let on = shown.is_some_and(|(s, _)| s == i as usize);
        out.mounts.push(tab_mount(map, colors, t, i, on));
    }
    let text_of =
        |tab: Selector| Selector::All(vec![Selector::Kind(ElementKind::TEXT), tab.within()]);
    let guest_tab = |on: bool| {
        Selector::All(vec![
            map.tab.clone(),
            Selector::Cell {
                prop: selected(),
                value: Value::Bool(on),
            },
        ])
    };
    let color = |c: &Option<Value>| {
        c.iter()
            .map(|c| Cell::new(rekindle_wire::ids::COLOR, c.clone()))
            .collect::<Vec<_>>()
    };
    let Some((index, panel)) = shown.filter(|(s, _)| *s < tabs.len()) else {
        // The guest's tabs' text, as the guest has them selected. A rule
        // that wrote them while a plugin's tab was shown would otherwise
        // leave the cells the guest wrote when it made them, which its
        // worklet has since painted over.
        for (on, c) in [(true, &colors.on), (false, &colors.off)] {
            if c.is_some() {
                out.rows.push(RowRule {
                    select: text_of(guest_tab(on)),
                    action: RowAction::Cells(color(c)),
                });
            }
        }
        if let Some(ind) = &map.indicator {
            out.mounts.push(Mount {
                under: ind.clone(),
                after: None,
                before: None,
                root: SPACER,
                nodes: vec![MountNode {
                    id: SPACER,
                    kind: ElementKind::VIEW,
                    cells: vec![Cell::new(flex(), Value::F64(tabs.len() as f64))],
                    children: Vec::new(),
                    on_press: None,
                }],
            });
        }
        return out;
    };
    let tab = &tabs[index];
    out.rows.push(RowRule {
        select: Selector::All(vec![
            map.content.clone().child_of(),
            Selector::Not(Box::new(map.strip.clone())),
        ]),
        action: RowAction::Drop,
    });
    if let Some(ind) = &map.indicator {
        out.rows.push(RowRule {
            select: ind.clone(),
            action: RowAction::Drop,
        });
    }
    out.rows.push(RowRule {
        select: map.tab.clone(),
        action: RowAction::Cells(vec![Cell::new(selected(), Value::Bool(false))]),
    });
    if colors.off.is_some() {
        out.rows.push(RowRule {
            select: text_of(map.tab.clone()),
            action: RowAction::Cells(color(&colors.off)),
        });
    }
    out.presses.push(PressRule {
        select: map.tab.clone(),
        action: PressAction::Notify(handler(&tab.plugin, GUEST_TAB, "")),
    });
    let mut b = Body {
        map,
        colors,
        plugin: &tab.plugin,
        panel: &tab.id,
        next: BODY_IDS[block % 2],
        nodes: Vec::new(),
        clicks: BTreeMap::new(),
    };
    let root = b.body(panel.map_or(&[][..], |p| &p.widgets));
    out.mounts.push(Mount {
        under: map.content.clone(),
        after: Some(map.strip.clone()),
        before: None,
        root,
        nodes: b.nodes,
    });
    out.clicks = b.clicks;
    out
}

/// Add the cells of `extra` to `cells`, each replacing one of the same prop.
fn push_cells(cells: &mut Vec<Cell>, extra: &[Cell]) {
    for c in extra {
        cells.retain(|x| x.prop != c.prop);
        cells.push(c.clone());
    }
}

/// One plugin tab, number `i`, mounted at the end of the picker's tab row.
/// `on` draws it as the selected tab, with the guest's selection fill and
/// its selected text colour.
fn tab_mount(map: &PickerMap, colors: &Colors, tab: &Tab, i: u32, on: bool) -> Mount {
    let root = TAB_IDS + 2 * i;
    let mut cells = vec![
        Cell::new(a11y::ROLE, Value::Str(Role::Tab.name().into())),
        Cell::new(a11y::LABEL, Value::Str(tab.title.clone())),
        Cell::new(
            a11y::ID,
            Value::Str(format!("rekindle.{}.picker-tab.{}", tab.plugin, tab.id)),
        ),
        Cell::new(selected(), Value::Bool(on)),
    ];
    push_cells(&mut cells, &map.tab_cells);
    if on {
        push_cells(&mut cells, &map.tab_on);
        if let Some(fill) = &colors.fill {
            push_cells(
                &mut cells,
                &[Cell::new(rekindle_wire::ids::BACKGROUND, fill.clone())],
            );
        }
    }
    let mut text = vec![Cell::new(TEXT, Value::Str(tab.title.clone()))];
    push_cells(&mut text, &map.tab_text);
    if let Some(c) = if on { &colors.on } else { &colors.off } {
        push_cells(&mut text, &[Cell::new(rekindle_wire::ids::COLOR, c.clone())]);
    }
    Mount {
        under: map.tab.clone().parent_of(),
        after: None,
        before: None,
        root,
        nodes: vec![
            MountNode {
                id: root,
                kind: ElementKind::VIEW,
                cells,
                children: vec![root + 1],
                on_press: Some(handler(&tab.plugin, TAB, &tab.id)),
            },
            MountNode {
                id: root + 1,
                kind: ElementKind::TEXT,
                cells: text,
                children: Vec::new(),
                on_press: None,
            },
        ],
    }
}

fn handler(plugin: &str, surface: &str, element: &str) -> Handler {
    Handler::Plugin {
        plugin: plugin.into(),
        surface: surface.into(),
        element: element.into(),
    }
}

/// Builds a tab's body, handing out its node ids in order.
struct Body<'a> {
    map: &'a PickerMap,
    colors: &'a Colors,
    plugin: &'a str,
    panel: &'a str,
    next: NodeId,
    nodes: Vec<MountNode>,
    clicks: BTreeMap<(String, String), Press>,
}

impl Body<'_> {
    fn node(&mut self, kind: ElementKind, cells: Vec<Cell>, children: Vec<NodeId>) -> NodeId {
        let id = self.next;
        self.next += 1;
        self.nodes.push(MountNode {
            id,
            kind,
            cells,
            children,
            on_press: None,
        });
        id
    }

    fn text(&mut self, base: &[Cell], text: &str, color: Option<&Value>) -> NodeId {
        let mut cells = vec![Cell::new(TEXT, Value::Str(text.into()))];
        cells.extend(base.iter().cloned());
        if let Some(c) = color {
            cells.push(Cell::new(rekindle_wire::ids::COLOR, c.clone()));
        }
        self.node(ElementKind::TEXT, cells, Vec::new())
    }

    /// A pressable node that reports `element` and `value` on the panel.
    fn pressable(
        &mut self,
        cells: Vec<Cell>,
        child: NodeId,
        key: String,
        element: &str,
        value: UiInput,
    ) -> NodeId {
        let id = self.node(ElementKind::VIEW, cells, vec![child]);
        if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
            n.on_press = Some(handler(self.plugin, BODY, &key));
        }
        self.clicks.insert(
            (BODY.into(), key),
            Press::Ui {
                plugin: self.plugin.into(),
                panel: self.panel.into(),
                element: element.into(),
                value,
            },
        );
        id
    }

    /// The body: chips for the first select, then a scroll of labels and
    /// rows of buttons. Its root's id.
    fn body(&mut self, widgets: &[Widget]) -> NodeId {
        let root = self.next;
        self.next += 1;
        let mut children = Vec::new();
        let chips = widgets.iter().find_map(|w| match w {
            Widget::Select {
                id,
                options,
                selected,
                ..
            } => Some((id, options, *selected)),
            Widget::Tabs { id, tabs, selected } => Some((id, tabs, *selected)),
            _ => None,
        });
        if let Some((id, options, chosen)) = chips {
            children.push(self.chips(id, options, chosen));
        }
        let mut column = Vec::new();
        let mut buttons: Vec<(&str, &str)> = Vec::new();
        for w in widgets {
            match w {
                Widget::Button { id, label } => buttons.push((id, label)),
                Widget::Heading(t) | Widget::Text(t) => {
                    self.grid(&mut buttons, &mut column);
                    let label = self.map.label.clone();
                    let color = self.colors.off.clone();
                    column.push(self.text(&label, t, color.as_ref()));
                }
                _ => {}
            }
        }
        self.grid(&mut buttons, &mut column);
        let inner = self.node(ElementKind::VIEW, self.map.column.clone(), column);
        children.push(self.node(ElementKind::SCROLL, self.map.scroll.clone(), vec![inner]));
        let mut cells = vec![Cell::new(
            a11y::ID,
            Value::Str(format!(
                "rekindle.{}.picker-body.{}",
                self.plugin, self.panel
            )),
        )];
        cells.extend(self.map.body.iter().cloned());
        self.nodes.push(MountNode {
            id: root,
            kind: ElementKind::VIEW,
            cells,
            children,
            on_press: None,
        });
        root
    }

    fn chips(&mut self, id: &str, options: &[String], chosen: u32) -> NodeId {
        let mut list = Vec::new();
        for (i, o) in (0u32..).zip(options) {
            let on = i == chosen;
            let color = if on {
                &self.colors.on
            } else {
                &self.colors.off
            }
            .clone();
            let text = self.text(&self.map.chip_text.clone(), o, color.as_ref());
            let mut cells = vec![
                Cell::new(a11y::ROLE, Value::Str(Role::Button.name().into())),
                Cell::new(a11y::LABEL, Value::Str(o.clone())),
                Cell::new(selected(), Value::Bool(on)),
            ];
            cells.extend(self.map.chip.iter().cloned());
            if on {
                cells.extend(self.map.chip_on.iter().cloned());
                if let Some(fill) = &self.colors.fill {
                    cells.push(Cell::new(rekindle_wire::ids::BACKGROUND, fill.clone()));
                }
            }
            list.push(self.pressable(cells, text, format!("{id}#{i}"), id, UiInput::Selected(i)));
        }
        let row = self.node(
            ElementKind::VIEW,
            vec![Cell::new(
                PropId::from_name("direction").expect("the PropId table has `direction`"),
                Value::Str("row".into()),
            )],
            list,
        );
        self.node(ElementKind::HSCROLL, self.map.chips.clone(), vec![row])
    }

    /// `buttons` as rows of the grid at the end of `column`, emptied.
    fn grid(&mut self, buttons: &mut Vec<(&str, &str)>, column: &mut Vec<NodeId>) {
        let per = self.map.columns.unwrap_or(3).max(1);
        for chunk in std::mem::take(buttons).chunks(per) {
            let mut cells = Vec::new();
            for (id, label) in chunk {
                let color = self.colors.on.clone();
                let text = self.text(&self.map.cell_text.clone(), label, color.as_ref());
                let mut c = vec![
                    Cell::new(a11y::ROLE, Value::Str(Role::Button.name().into())),
                    Cell::new(a11y::LABEL, Value::Str((*label).into())),
                ];
                c.extend(self.map.cell.iter().cloned());
                cells.push(self.pressable(c, text, (*id).into(), id, UiInput::Clicked));
            }
            // A short last row keeps the grid's columns.
            for _ in chunk.len()..per {
                let mut c = self.map.cell.clone();
                c.push(Cell::new(a11y::HIDDEN, Value::Bool(true)));
                cells.push(self.node(ElementKind::VIEW, c, Vec::new()));
            }
            column.push(self.node(ElementKind::VIEW, self.map.row.clone(), cells));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> PickerMap {
        let seg = Selector::Prefix {
            prop: a11y::ID,
            prefix: "tabs:".into(),
        };
        let v = |s: Selector| serde_json::to_value(s).unwrap();
        serde_json::from_value(serde_json::json!({
            "tab": v(seg.clone()),
            "content": v(seg.clone().parent_of().parent_of()),
            "strip": v(seg.parent_of()),
            "indicator": v(Selector::cell(a11y::ID, "indicator")),
        }))
        .unwrap()
    }

    fn tabs() -> Vec<Tab> {
        vec![Tab {
            plugin: "dev.k".into(),
            id: "faces".into(),
            title: "Faces".into(),
        }]
    }

    fn panel() -> PanelItem {
        PanelItem {
            plugin: "dev.k".into(),
            id: "faces".into(),
            title: "Faces".into(),
            widgets: vec![
                Widget::TextInput {
                    id: "search".into(),
                    label: "Search".into(),
                    value: String::new(),
                    placeholder: String::new(),
                },
                Widget::Select {
                    id: "category".into(),
                    label: "Category".into(),
                    options: vec!["Happy".into(), "Sad".into()],
                    selected: 1,
                },
                Widget::Button {
                    id: "face-0".into(),
                    label: "(^_^)".into(),
                },
                Widget::Button {
                    id: "face-1".into(),
                    label: "(;_;)".into(),
                },
                Widget::Text("More".into()),
                Widget::Button {
                    id: "face-2".into(),
                    label: "(-_-)".into(),
                },
            ],
        }
    }

    #[test]
    fn the_colours_are_read_off_the_guests_tabs() {
        use rekindle_wire::{NodeOp, Turn};
        let mut t = Turn::new();
        let node = |t: &mut Turn, id, kind, props| {
            t.push(NodeOp::Create {
                node: id,
                kind,
                props: rekindle_wire::turn::create_props(props),
            });
        };
        let s = |v: &str| Value::Str(v.into());
        node(&mut t, 1, ElementKind::VIEW, vec![]);
        node(&mut t, 2, ElementKind::VIEW, vec![]);
        node(
            &mut t,
            3,
            ElementKind::VIEW,
            vec![(a11y::ID, s("indicator"))],
        );
        node(
            &mut t,
            4,
            ElementKind::VIEW,
            vec![(rekindle_wire::ids::BACKGROUND, Value::color(0x3d97_979f))],
        );
        node(
            &mut t,
            5,
            ElementKind::VIEW,
            vec![(a11y::ID, s("tabs:0")), (selected(), Value::Bool(true))],
        );
        node(
            &mut t,
            6,
            ElementKind::TEXT,
            vec![(rekindle_wire::ids::COLOR, Value::color(0xff11_1111))],
        );
        node(
            &mut t,
            7,
            ElementKind::VIEW,
            vec![(a11y::ID, s("tabs:1")), (selected(), Value::Bool(false))],
        );
        node(
            &mut t,
            8,
            ElementKind::TEXT,
            vec![(rekindle_wire::ids::COLOR, Value::color(0xff99_9999))],
        );
        for (n, c) in [
            (1, vec![2]),
            (2, vec![3, 5, 7]),
            (3, vec![4]),
            (5, vec![6]),
            (7, vec![8]),
        ] {
            t.push(NodeOp::Children {
                node: n,
                children: c,
            });
        }
        let mut tree = Tree::new();
        tree.apply(&t);
        let mut e =
            rekindle_wire_inject::Engine::new(rekindle_wire_inject::Work::default()).unwrap();
        e.filter(&t.to_bytes()).unwrap();
        let read = read(&tree, &map(), |sel| e.find(sel));
        assert_eq!(
            read,
            Some(Colors {
                on: Some(Value::color(0xff11_1111)),
                off: Some(Value::color(0xff99_9999)),
                fill: Some(Value::color(0x3d97_979f)),
            })
        );
        let empty = Tree::new();
        assert_eq!(
            super::read(&empty, &map(), |_| None),
            None,
            "the picker is closed"
        );
    }

    #[test]
    fn a_hidden_tab_is_mounted_with_a_spacer_and_changes_nothing_else() {
        let d = draw(&map(), &Colors::default(), &tabs(), None, 0);
        assert_eq!(d.mounts.len(), 2);
        assert_eq!(
            d.mounts[0].nodes[0].on_press,
            Some(handler("dev.k", TAB, "faces"))
        );
        assert_eq!(d.mounts[1].root, SPACER);
        assert_eq!(
            d.mounts[1].nodes[0].cells,
            vec![Cell::new(flex(), Value::F64(1.0))]
        );
        assert!(d.rows.is_empty() && d.presses.is_empty() && d.clicks.is_empty());
        // With the guest's colours, its tabs' text keeps them.
        let colors = Colors {
            on: Some(Value::color(0xff11_1111)),
            off: Some(Value::color(0xff99_9999)),
            fill: None,
        };
        assert_eq!(draw(&map(), &colors, &tabs(), None, 0).rows.len(), 2);
        assert!(
            draw(&map(), &Colors::default(), &[], None, 0)
                .mounts
                .is_empty()
        );
    }

    #[test]
    fn a_shown_tab_drops_the_guests_body_and_shows_the_panel() {
        let colors = Colors {
            on: Some(Value::color(0xff11_1111)),
            off: Some(Value::color(0xff99_9999)),
            fill: Some(Value::color(0x3d97_979f)),
        };
        let p = panel();
        let d = draw(&map(), &colors, &tabs(), Some((0, Some(&p))), 1);
        // The tab, then the body after the strip.
        assert_eq!(d.mounts.len(), 2);
        let tab = &d.mounts[0].nodes[0];
        assert!(tab.cells.contains(&Cell::new(
            rekindle_wire::ids::BACKGROUND,
            Value::color(0x3d97_979f)
        )));
        let body = &d.mounts[1];
        assert_eq!(body.root, BODY_IDS[1]);
        assert_eq!(body.after, Some(map().strip));
        // The guest's body and selection go, its tabs read unselected, and
        // a press on one takes the plugin's tab down.
        assert_eq!(d.rows.len(), 4);
        assert!(matches!(d.rows[0].action, RowAction::Drop));
        assert_eq!(
            d.presses[0].action,
            PressAction::Notify(handler("dev.k", GUEST_TAB, ""))
        );
        // Two chips and three buttons report as the panel's own controls.
        assert_eq!(d.clicks.len(), 5);
        assert_eq!(
            d.clicks.get(&(BODY.into(), "category#0".into())),
            Some(&Press::Ui {
                plugin: "dev.k".into(),
                panel: "faces".into(),
                element: "category".into(),
                value: UiInput::Selected(0),
            })
        );
        assert_eq!(
            d.clicks.get(&(BODY.into(), "face-2".into())),
            Some(&Press::Ui {
                plugin: "dev.k".into(),
                panel: "faces".into(),
                element: "face-2".into(),
                value: UiInput::Clicked,
            })
        );
        // Two rows of three, two faces and a filler, then the last face and
        // two fillers, and the column holding them with the label between.
        let rows = body
            .nodes
            .iter()
            .filter(|n| n.children.len() == 3 && n.kind == ElementKind::VIEW)
            .count();
        assert_eq!(rows, 3);
        assert!(body.nodes.iter().any(|n| {
            n.cells
                .contains(&Cell::new(TEXT, Value::Str("More".into())))
        }));
        // The ids are the mount's own and unique.
        let mut ids: Vec<NodeId> = body.nodes.iter().map(|n| n.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), body.nodes.len());
        let work = rekindle_wire_inject::Work {
            mounts: d.mounts,
            rows: d.rows,
            presses: d.presses,
            ..Default::default()
        };
        assert!(work.check().is_ok());
    }
}
