//! rekindle's panels as rows on a client's wire: its section on the client's
//! settings screen and a plugin's modal.
//!
//! A client split across a wire has no framework layer to draw rekindle's
//! panels in, so they become mounts of the client's own element kinds. The
//! row shapes come from a [`Look`], the cells each part of a row gets, read
//! from the rows the client's guest builds. Nothing here names the client:
//! every cell a node gets beyond its text, its accessibility cells and its
//! state is in the look.
//!
//! The section is rendered from rekindle's settings panel (the `Manager`'s
//! panel, as widgets): one switch row per installed plugin and one for
//! developer mode, or, while the panel shows a page of its own (a plugin's
//! page, the developer-mode warning, a review), that page's elements as rows.
//! A press on a row comes back as the [`Click`](rekindle_ui_hook::Click) the
//! panel's own controls report, from [`Rendered::presses`].

use std::collections::BTreeMap;

use rekindle_plugin_host::manager;
use rekindle_ui_hook::{CardItem, PanelItem, UiInput, Widget};
use rekindle_wire::a11y::{Role, ids as a11y};
use rekindle_wire::{ElementKind, NodeId, PropId, Value};
use rekindle_wire_inject::{Cell, Handler, Mount, MountNode, RESERVED, Selector, TEXT};
use serde::Deserialize;

/// The handler surface of a row in rekindle's section.
pub(crate) const SECTION: &str = "settings";
/// The handler surface of a row in a plugin's modal.
pub(crate) const MODAL: &str = "modal";
/// The handler surface of the layer behind a modal: a press there closes it.
pub(crate) const MODAL_SCRIM: &str = "modal.scrim";
/// The handler surface of a modal's card, which a press on does nothing.
pub(crate) const MODAL_CARD: &str = "modal.card";

/// The first node id of rekindle's section.
const SECTION_IDS: NodeId = RESERVED + 0x0200_0000;
/// The first node id of a plugin's modal.
const MODAL_IDS: NodeId = RESERVED + 0x0300_0000;

/// The cells each part of a row gets, and where the section and a modal go.
/// Read from the client's own rows, so a mount looks like them.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Look {
    /// The node the section is mounted under.
    pub(crate) under: Selector,
    /// The child of that node the section goes after, if not last.
    #[serde(default)]
    pub(crate) after: Option<Selector>,
    /// The node a modal is mounted under: one whose children are drawn over
    /// each other, so the last is on top. Without one, modals aren't shown.
    #[serde(default)]
    pub(crate) modal_under: Option<Selector>,
    /// The section: a column of groups.
    pub(crate) section: Vec<Cell>,
    /// A group's header text.
    pub(crate) header: Vec<Cell>,
    /// A group of rows.
    pub(crate) group: Vec<Cell>,
    /// A row, and what its group's first and last rows add.
    pub(crate) row: Vec<Cell>,
    #[serde(default)]
    pub(crate) first: Vec<Cell>,
    #[serde(default)]
    pub(crate) last: Vec<Cell>,
    /// The row's content: icon, the title column and what ends the row.
    pub(crate) inner: Vec<Cell>,
    pub(crate) icon: Vec<Cell>,
    pub(crate) column: Vec<Cell>,
    pub(crate) title: Vec<Cell>,
    pub(crate) subtitle: Vec<Cell>,
    pub(crate) chevron_box: Vec<Cell>,
    pub(crate) chevron: Vec<Cell>,
    /// A switch: its box, its track and thumb, and what each adds when on
    /// and when off.
    pub(crate) switch_box: Vec<Cell>,
    pub(crate) track: Vec<Cell>,
    pub(crate) track_on: Vec<Cell>,
    pub(crate) track_off: Vec<Cell>,
    pub(crate) thumb: Vec<Cell>,
    pub(crate) thumb_on: Vec<Cell>,
    pub(crate) thumb_off: Vec<Cell>,
    /// Between two rows: a box holding a rule.
    pub(crate) divider: Vec<Cell>,
    pub(crate) rule: Vec<Cell>,
    /// A modal: the layer over the screen, which lays the card out, and the
    /// card on it.
    #[serde(default)]
    pub(crate) scrim: Vec<Cell>,
    #[serde(default)]
    pub(crate) card: Vec<Cell>,
    /// The `uri` of each row's icon, by the row's key. `*` is the icon of a
    /// row with none of its own. A row with neither has no icon.
    #[serde(default)]
    pub(crate) icons: BTreeMap<String, String>,
}

impl Look {
    pub(crate) fn from_json(json: &str) -> anyhow::Result<Look> {
        Ok(serde_json::from_str(json)?)
    }
}

/// One line of a panel, as rows show it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Item {
    /// The start of a group under a header.
    Header(String),
    /// The start of a group without one.
    Break,
    /// A line of text in a row of its own.
    Text(String),
    Row(Row),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    /// The row's id among its panel's rows, and its handler's element.
    pub(crate) key: String,
    pub(crate) title: String,
    pub(crate) subtitle: Option<String>,
    pub(crate) end: End,
    /// What a press on the row reports. `None` for a row a press does
    /// nothing on.
    pub(crate) press: Option<(String, UiInput)>,
}

/// What a row ends in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum End {
    Nothing,
    Chevron,
    Switch(bool),
}

/// A mount, and what a press on each of its rows reports: by the handler's
/// surface and element, the panel's element and its value.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Rendered {
    pub(crate) mounts: Vec<Mount>,
    pub(crate) presses: BTreeMap<(String, String), Press>,
}

/// What a press on a row of rekindle's reports.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Press {
    /// An interaction with a panel: `plugin`'s panel `panel`.
    Ui {
        plugin: String,
        panel: String,
        element: String,
        value: UiInput,
    },
    /// Close `plugin`'s modal.
    Close { plugin: String },
}

impl Rendered {
    fn add(&mut self, other: Rendered) {
        self.mounts.extend(other.mounts);
        self.presses.extend(other.presses);
    }
}

/// The rows of rekindle's section, from the widgets of its settings panel.
/// `developer_mode` is the home's.
pub(crate) fn section_items(widgets: &[Widget], developer_mode: bool) -> Vec<Item> {
    let mut items = vec![Item::Header("rekindle".into())];
    // The grid (it has tabs) becomes the installed plugins and a developer
    // mode switch. Any other page is shown as it is.
    if !widgets.iter().any(|w| matches!(w, Widget::Tabs { .. })) {
        items.extend(widget_items(widgets));
        return items;
    }
    for w in widgets {
        match w {
            Widget::Text(t) => items.push(Item::Text(t.clone())),
            Widget::Cards(cards) => items.extend(cards.iter().map(card_row)),
            _ => {}
        }
    }
    let (subtitle, press) = if developer_mode {
        (
            "On: plugins from outside rekindle can be installed and run.",
            "developer-mode-off",
        )
    } else {
        (
            "Off: only plugins from rekindle and its community index can be installed.",
            "developer-mode-on",
        )
    };
    items.push(Item::Row(Row {
        key: "developer-mode".into(),
        title: "Developer Mode".into(),
        subtitle: Some(subtitle.into()),
        end: End::Switch(developer_mode),
        press: Some((press.into(), UiInput::Clicked)),
    }));
    items
}

/// A plugin's card as a row: a switch when the card turns the plugin on or
/// off, else a chevron to the plugin's page. The Manager's own card id is
/// `open:<plugin>` and its action id `enable:<plugin>`/`disable:<plugin>`;
/// pressing either drives [`Manager::handle`], so the press carries that
/// action id, while the row's key (its node identity) stays `on:<plugin>`
/// so the switch keeps its node as it flips.
fn card_row(c: &CardItem) -> Item {
    let verb = c
        .action
        .as_ref()
        .and_then(|(id, _)| id.split_once(':').map(|(v, _)| v));
    let on = match verb {
        Some("disable") => Some(true),
        Some("enable") => Some(false),
        _ => None,
    };
    // The plugin id, from the action or the `open:<plugin>` card id.
    let plugin = c
        .action
        .as_ref()
        .and_then(|(id, _)| id.split_once(':').map(|(_, p)| p))
        .or_else(|| c.id.strip_prefix("open:"))
        .unwrap_or(c.id.as_str());
    let subtitle = [c.meta.as_str(), c.state.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let (key, end, element) = match on {
        Some(on) => {
            // The action id toggles the plugin through the Manager.
            let element = c.action.as_ref().map(|(id, _)| id.clone());
            (
                format!("on:{plugin}"),
                End::Switch(on),
                element.unwrap_or_else(|| format!("on:{plugin}")),
            )
        }
        None => (format!("open:{plugin}"), End::Chevron, c.id.clone()),
    };
    Item::Row(Row {
        key,
        title: c.title.clone(),
        subtitle: (!subtitle.is_empty()).then_some(subtitle),
        end,
        press: Some((element, UiInput::Clicked)),
    })
}

/// A panel's widgets as rows, for a page of rekindle's or a plugin's modal.
/// Text fields, sliders, colours and tables have no row yet and are left
/// out, except a text field's label.
pub(crate) fn widget_items(widgets: &[Widget]) -> Vec<Item> {
    let mut items = Vec::new();
    for w in widgets {
        match w {
            Widget::Heading(t) => items.push(Item::Header(t.clone())),
            Widget::Text(t) => items.push(Item::Text(t.clone())),
            Widget::Separator => items.push(Item::Break),
            Widget::Toggle { id, label, value } => items.push(Item::Row(Row {
                key: id.clone(),
                title: label.clone(),
                subtitle: None,
                end: End::Switch(*value),
                press: Some((id.clone(), UiInput::Toggled(!value))),
            })),
            Widget::Button { id, label } => items.push(Item::Row(Row {
                key: id.clone(),
                title: label.clone(),
                subtitle: None,
                end: End::Chevron,
                press: Some((id.clone(), UiInput::Clicked)),
            })),
            Widget::Header {
                back,
                title,
                meta,
                toggle,
                action,
            } => {
                items.push(Item::Row(Row {
                    key: toggle
                        .as_ref()
                        .map_or_else(|| "header".into(), |(id, _, _)| id.clone()),
                    title: title.clone(),
                    subtitle: (!meta.is_empty()).then(|| meta.clone()),
                    end: toggle
                        .as_ref()
                        .map_or(End::Nothing, |(_, _, on)| End::Switch(*on)),
                    press: toggle
                        .as_ref()
                        .map(|(id, _, on)| (id.clone(), UiInput::Toggled(!on))),
                }));
                if let Some((id, label)) = action {
                    items.push(Item::Row(Row {
                        key: id.clone(),
                        title: label.clone(),
                        subtitle: None,
                        end: End::Chevron,
                        press: Some((id.clone(), UiInput::Clicked)),
                    }));
                }
                items.push(Item::Row(Row {
                    key: back.clone(),
                    title: "Back".into(),
                    subtitle: None,
                    end: End::Chevron,
                    press: Some((back.clone(), UiInput::Clicked)),
                }));
            }
            Widget::Cards(cards) => items.extend(cards.iter().map(card_row)),
            Widget::TextInput { label, .. } => items.push(Item::Text(label.clone())),
            Widget::Select { .. }
            | Widget::Slider { .. }
            | Widget::Color { .. }
            | Widget::Tabs { .. } => {}
        }
    }
    items
}

/// rekindle's section as a mount, from its rows.
pub(crate) fn section(look: &Look, items: &[Item]) -> Rendered {
    let mut b = Builder::new(look, SECTION_IDS);
    let content = b.content(items, manager::OWNER, SECTION, |key| Press::Ui {
        plugin: manager::OWNER.into(),
        panel: manager::PANEL.into(),
        element: key.0,
        value: key.1,
    });
    let mut cells = look.section.clone();
    cells.push(Cell::new(a11y::ID, Value::Str("rekindle.settings".into())));
    let root = b.node(ElementKind::VIEW, cells, content, None);
    b.finish(look.under.clone(), look.after.clone(), root)
}

/// `plugin`'s modal `panel` as a mount: a layer over the screen that closes
/// it when pressed, holding a card with the panel's title and rows. Nothing
/// without a place for modals in the look.
pub(crate) fn modal(look: &Look, plugin: &str, panel: &PanelItem) -> Rendered {
    let Some(under) = &look.modal_under else {
        return Rendered::default();
    };
    let mut b = Builder::new(look, MODAL_IDS);
    b.icons = false;
    let mut items = vec![Item::Header(panel.title.clone())];
    items.extend(widget_items(&panel.widgets));
    let content = b.content(&items, plugin, MODAL, |(element, value)| Press::Ui {
        plugin: plugin.into(),
        panel: panel.id.clone(),
        element,
        value,
    });
    let card = b.node(
        ElementKind::VIEW,
        look.card.clone(),
        content,
        Some(handler(plugin, MODAL_CARD, "")),
    );
    let mut scrim = look.scrim.clone();
    scrim.push(Cell::new(a11y::ID, Value::Str(format!("rekindle.{plugin}.modal"))));
    let root = b.node(
        ElementKind::VIEW,
        scrim,
        vec![card],
        Some(handler(plugin, MODAL_SCRIM, "")),
    );
    let mut r = b.finish(under.clone(), None, root);
    r.presses.insert(
        (MODAL_SCRIM.into(), String::new()),
        Press::Close {
            plugin: plugin.into(),
        },
    );
    r
}

/// Several renders as one.
pub(crate) fn join(parts: impl IntoIterator<Item = Rendered>) -> Rendered {
    let mut all = Rendered::default();
    for p in parts {
        all.add(p);
    }
    all
}

fn handler(plugin: &str, surface: &str, element: &str) -> Handler {
    Handler::Plugin {
        plugin: plugin.into(),
        surface: surface.into(),
        element: element.into(),
    }
}

/// The `uri` cell (531) of an image.
fn uri() -> PropId {
    PropId::from_name("uri").expect("the PropId table has `uri`")
}

/// Hands out a mount's node ids in order, so the same rows get the same ids.
struct Builder<'l> {
    look: &'l Look,
    /// Rows get the look's icons.
    icons: bool,
    next: NodeId,
    nodes: Vec<MountNode>,
    presses: BTreeMap<(String, String), Press>,
}

impl<'l> Builder<'l> {
    fn new(look: &'l Look, first: NodeId) -> Self {
        Builder {
            look,
            icons: true,
            next: first,
            nodes: Vec::new(),
            presses: BTreeMap::new(),
        }
    }

    fn node(
        &mut self,
        kind: ElementKind,
        cells: Vec<Cell>,
        children: Vec<NodeId>,
        on_press: Option<Handler>,
    ) -> NodeId {
        let id = self.next;
        self.next += 1;
        self.nodes.push(MountNode {
            id,
            kind,
            cells,
            children,
            on_press,
        });
        id
    }

    fn text(&mut self, base: &[Cell], text: &str) -> NodeId {
        let mut cells = vec![Cell::new(TEXT, Value::Str(text.into()))];
        cells.extend(base.iter().cloned());
        self.node(ElementKind::TEXT, cells, Vec::new(), None)
    }

    /// The groups of `items`, each under its header. A row with a press
    /// gets a handler for `plugin` on `surface`, and `press` makes what it
    /// reports.
    fn content(
        &mut self,
        items: &[Item],
        plugin: &str,
        surface: &str,
        press: impl Fn((String, UiInput)) -> Press,
    ) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut group: Vec<&Item> = Vec::new();
        let flush = |b: &mut Self, group: &mut Vec<&Item>, out: &mut Vec<NodeId>| {
            if group.is_empty() {
                return;
            }
            let rows = std::mem::take(group);
            let n = rows.len();
            let mut children = Vec::new();
            for (i, item) in rows.into_iter().enumerate() {
                if i > 0 {
                    let rule = b.node(ElementKind::VIEW, b.look.rule.clone(), Vec::new(), None);
                    let divider =
                        b.node(ElementKind::VIEW, b.look.divider.clone(), vec![rule], None);
                    children.push(divider);
                }
                children.push(b.row(item, i == 0, i + 1 == n, plugin, surface, &press));
            }
            out.push(b.node(ElementKind::VIEW, b.look.group.clone(), children, None));
        };
        for item in items {
            match item {
                Item::Header(t) => {
                    flush(self, &mut group, &mut out);
                    out.push(self.text(&self.look.header.clone(), t));
                }
                Item::Break => flush(self, &mut group, &mut out),
                Item::Text(_) | Item::Row(_) => group.push(item),
            }
        }
        flush(self, &mut group, &mut out);
        out
    }

    fn row(
        &mut self,
        item: &Item,
        first: bool,
        last: bool,
        plugin: &str,
        surface: &str,
        press: &impl Fn((String, UiInput)) -> Press,
    ) -> NodeId {
        let look = self.look;
        let mut inner = Vec::new();
        let mut cells = look.row.clone();
        if first {
            cells.extend(look.first.iter().cloned());
        }
        if last {
            cells.extend(look.last.iter().cloned());
        }
        let mut on_press = None;
        match item {
            Item::Text(t) => {
                let line = self.text(&look.subtitle, t);
                inner.push(self.node(ElementKind::VIEW, look.column.clone(), vec![line], None));
                cells.push(Cell::new(a11y::LABEL, Value::Str(t.clone())));
            }
            Item::Row(r) => {
                let icon = self
                    .icons
                    .then(|| look.icons.get(&r.key).or_else(|| look.icons.get("*")))
                    .flatten();
                if let Some(u) = icon {
                    let mut c = vec![Cell::new(uri(), Value::Str(u.clone()))];
                    c.extend(look.icon.iter().cloned());
                    inner.push(self.node(ElementKind::IMAGE, c, Vec::new(), None));
                }
                let mut lines = vec![self.text(&look.title, &r.title)];
                if let Some(s) = &r.subtitle {
                    lines.push(self.text(&look.subtitle, s));
                }
                inner.push(self.node(ElementKind::VIEW, look.column.clone(), lines, None));
                match r.end {
                    End::Nothing => {}
                    End::Chevron => {
                        let chevron =
                            self.node(ElementKind::IMAGE, look.chevron.clone(), Vec::new(), None);
                        inner.push(self.node(
                            ElementKind::VIEW,
                            look.chevron_box.clone(),
                            vec![chevron],
                            None,
                        ));
                    }
                    End::Switch(on) => {
                        let state = |base: &[Cell], on_cells: &[Cell], off: &[Cell]| {
                            let mut c = base.to_vec();
                            c.extend(if on { on_cells } else { off }.iter().cloned());
                            c
                        };
                        let thumb = self.node(
                            ElementKind::VIEW,
                            state(&look.thumb, &look.thumb_on, &look.thumb_off),
                            Vec::new(),
                            None,
                        );
                        let track = self.node(
                            ElementKind::VIEW,
                            state(&look.track, &look.track_on, &look.track_off),
                            vec![thumb],
                            None,
                        );
                        inner.push(self.node(
                            ElementKind::VIEW,
                            look.switch_box.clone(),
                            vec![track],
                            None,
                        ));
                        cells.push(Cell::new(a11y::CHECKED, Value::Bool(on)));
                    }
                }
                let role = match r.end {
                    End::Switch(_) => Role::Switch,
                    _ => Role::Button,
                };
                let label = match &r.subtitle {
                    Some(s) => format!("{}, {s}", r.title),
                    None => r.title.clone(),
                };
                cells.push(Cell::new(a11y::ROLE, Value::Str(role.name().into())));
                cells.push(Cell::new(a11y::LABEL, Value::Str(label)));
                cells.push(Cell::new(
                    a11y::ID,
                    Value::Str(format!("rekindle.{plugin}.{}", r.key)),
                ));
                if let Some(p) = &r.press {
                    on_press = Some(handler(plugin, surface, &r.key));
                    self.presses
                        .insert((surface.into(), r.key.clone()), press(p.clone()));
                }
            }
            Item::Header(_) | Item::Break => {}
        }
        let inner = self.node(ElementKind::VIEW, look.inner.clone(), inner, None);
        self.node(ElementKind::VIEW, cells, vec![inner], on_press)
    }

    fn finish(self, under: Selector, after: Option<Selector>, root: NodeId) -> Rendered {
        Rendered {
            mounts: vec![Mount {
                under,
                after,
                root,
                nodes: self.nodes,
            }],
            presses: self.presses,
        }
    }
}

#[cfg(test)]
mod tests {
    use rekindle_wire_inject::Work;

    use super::*;

    fn look() -> Look {
        Look::from_json(
            r#"{
            "under": {"cell": {"prop": "accessibilityId", "value": {"str": "list"}}},
            "modal_under": {"kind": "stack"},
            "section": [], "header": [{"prop": "role", "value": {"str": "header"}}],
            "group": [], "row": [], "first": [{"prop": "radiusTopLeft", "value": {"f64": 12}}],
            "last": [{"prop": "radiusBottomLeft", "value": {"f64": 12}}],
            "inner": [], "icon": [], "column": [], "title": [], "subtitle": [],
            "chevron_box": [], "chevron": [], "switch_box": [], "track": [],
            "track_on": [{"prop": "background", "value": {"rgba": "5865f2ff"}}],
            "track_off": [{"prop": "background", "value": {"rgba": "0000001f"}}],
            "thumb": [], "thumb_on": [{"prop": "translateX", "value": {"f64": 28}}],
            "thumb_off": [{"prop": "translateX", "value": {"f64": 0}}],
            "divider": [], "rule": [], "scrim": [], "card": [],
            "icons": {"*": "res://plugin", "developer-mode": "res://warning"}
        }"#,
        )
        .unwrap()
    }

    // The Manager's own card: id is `open:<plugin>`, the action id
    // `<verb>:<plugin>`.
    fn card(id: &str, action: Option<&str>, state: &str) -> CardItem {
        CardItem {
            id: format!("open:{id}"),
            title: id.to_uppercase(),
            meta: "0.1.0 · rekindle".into(),
            text: String::new(),
            state: state.into(),
            action: action.map(|a| (format!("{a}:{id}"), "Turn".into())),
        }
    }

    fn grid() -> Vec<Widget> {
        vec![
            Widget::Tabs {
                id: "tab".into(),
                tabs: vec!["Installed".into()],
                selected: 0,
            },
            Widget::Cards(vec![
                card("dev.a", Some("disable"), "On"),
                card("dev.b", Some("enable"), "Off"),
                card("dev.c", None, "Off: developer mode is off"),
            ]),
        ]
    }

    fn row(items: &[Item], key: &str) -> Row {
        items
            .iter()
            .find_map(|i| match i {
                Item::Row(r) if r.key == key => Some(r.clone()),
                _ => None,
            })
            .unwrap()
    }

    fn node<'m>(r: &'m Rendered, id: &str) -> &'m MountNode {
        r.mounts
            .iter()
            .flat_map(|m| &m.nodes)
            .find(|n| {
                n.cells
                    .iter()
                    .any(|c| c.prop == a11y::ID && c.value == Value::Str(id.into()))
            })
            .unwrap()
    }

    fn cell<'n>(n: &'n MountNode, p: PropId) -> Option<&'n Value> {
        n.cells.iter().find(|c| c.prop == p).map(|c| &c.value)
    }

    #[test]
    fn the_grid_is_a_switch_per_plugin_and_one_for_developer_mode() {
        let items = section_items(&grid(), false);
        assert_eq!(items[0], Item::Header("rekindle".into()));
        let a = row(&items, "on:dev.a");
        assert_eq!(a.end, End::Switch(true));
        // The node's key is stable `on:<plugin>`, but the press drives the
        // Manager's own `disable:<plugin>` control.
        assert_eq!(a.press, Some(("disable:dev.a".into(), UiInput::Clicked)));
        assert_eq!(a.subtitle.as_deref(), Some("0.1.0 · rekindle · On"));
        let b = row(&items, "on:dev.b");
        assert_eq!(b.end, End::Switch(false));
        assert_eq!(b.press, Some(("enable:dev.b".into(), UiInput::Clicked)));
        // A plugin that can't be turned on here opens its page.
        let c = row(&items, "open:dev.c");
        assert_eq!(c.end, End::Chevron);
        assert_eq!(c.press, Some(("open:dev.c".into(), UiInput::Clicked)));
        let dev = row(&items, "developer-mode");
        assert_eq!(dev.end, End::Switch(false));
        assert_eq!(
            dev.press,
            Some(("developer-mode-on".into(), UiInput::Clicked))
        );
        let on = section_items(&grid(), true);
        assert_eq!(
            row(&on, "developer-mode").press,
            Some(("developer-mode-off".into(), UiInput::Clicked))
        );
    }

    #[test]
    fn a_page_of_its_own_is_shown_as_its_elements() {
        let page = vec![
            Widget::Heading("Turn on developer mode?".into()),
            Widget::Text("Plugins from outside rekindle can do anything.".into()),
            Widget::Button {
                id: "cancel".into(),
                label: "Cancel".into(),
            },
        ];
        let items = section_items(&page, false);
        assert_eq!(items[1], Item::Header("Turn on developer mode?".into()));
        assert!(matches!(&items[2], Item::Text(_)));
        assert_eq!(
            row(&items, "cancel").press,
            Some(("cancel".into(), UiInput::Clicked))
        );
    }

    #[test]
    fn the_section_mounts_rows_whose_presses_report_the_panels_controls() {
        let look = look();
        let r = section(&look, &section_items(&grid(), false));
        Work {
            mounts: r.mounts.clone(),
            ..Work::default()
        }
        .check()
        .unwrap();
        let a = node(&r, "rekindle.rekindle.on:dev.a");
        assert_eq!(cell(a, a11y::CHECKED), Some(&Value::Bool(true)));
        assert_eq!(cell(a, a11y::ROLE), Some(&Value::Str("switch".into())));
        assert_eq!(
            a.on_press,
            Some(handler(manager::OWNER, SECTION, "on:dev.a"))
        );
        assert!(cell(a, PropId::from_name("radiusTopLeft").unwrap()).is_some());
        assert_eq!(
            r.presses[&(SECTION.to_string(), "on:dev.a".to_string())],
            Press::Ui {
                plugin: manager::OWNER.into(),
                panel: manager::PANEL.into(),
                element: "disable:dev.a".into(),
                value: UiInput::Clicked,
            }
        );
        let dev = node(&r, "rekindle.rekindle.developer-mode");
        assert!(cell(dev, PropId::from_name("radiusBottomLeft").unwrap()).is_some());

        // A flipped switch keeps every node and changes only cells.
        let mut flipped = grid();
        if let Widget::Cards(c) = &mut flipped[1] {
            c[0].action = Some(("enable:dev.a".into(), "Turn on".into()));
            c[0].state = "Off".into();
        }
        let again = section(&look, &section_items(&flipped, false));
        let shape = |m: &Mount| {
            m.nodes
                .iter()
                .map(|n| (n.id, n.kind.clone(), n.children.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(shape(&again.mounts[0]), shape(&r.mounts[0]));
        assert_eq!(
            cell(
                node(&again, "rekindle.rekindle.on:dev.a"),
                a11y::CHECKED
            ),
            Some(&Value::Bool(false))
        );
    }

    #[test]
    fn a_modal_is_a_layer_that_closes_and_a_card_of_rows() {
        let panel = PanelItem {
            plugin: "dev.a".into(),
            id: "bookmarks".into(),
            title: "Bookmarks".into(),
            widgets: vec![
                Widget::Text("No bookmarks yet.".into()),
                Widget::Button {
                    id: "close".into(),
                    label: "Close".into(),
                },
            ],
        };
        let r = modal(&look(), "dev.a", &panel);
        let m = &r.mounts[0];
        assert!(m.root >= MODAL_IDS);
        let layer = m.nodes.iter().find(|n| n.id == m.root).unwrap();
        assert_eq!(layer.kind, ElementKind::VIEW);
        // A modal's rows have no icons.
        assert!(!m.nodes.iter().any(|n| n.kind == ElementKind::IMAGE
            && n.cells.iter().any(|c| c.value == Value::Str("res://plugin".into()))));
        assert_eq!(layer.on_press, Some(handler("dev.a", MODAL_SCRIM, "")));
        assert_eq!(
            r.presses[&(MODAL_SCRIM.to_string(), String::new())],
            Press::Close {
                plugin: "dev.a".into()
            }
        );
        assert_eq!(
            r.presses[&(MODAL.to_string(), "close".to_string())],
            Press::Ui {
                plugin: "dev.a".into(),
                panel: "bookmarks".into(),
                element: "close".into(),
                value: UiInput::Clicked,
            }
        );
        // No place for modals, no modal.
        let mut no = look();
        no.modal_under = None;
        assert_eq!(modal(&no, "dev.a", &panel), Rendered::default());
    }
}
