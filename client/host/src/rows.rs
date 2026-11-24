//! rekindle's panels as rows on a client's wire: its menu on the client's
//! settings screen, its pages over the screen, and a plugin's modal.
//!
//! A client split across a wire has no framework layer to draw rekindle's
//! panels in, so they become mounts of the client's own element kinds. The
//! row shapes come from a [`Look`], the cells each part of a row gets, read
//! from the rows the client's guest builds. Nothing here names the client:
//! every cell a node gets beyond its text, its accessibility cells and its
//! state is in the look.
//!
//! The settings screen gets the Manager's menu ([`menu_items`]): a row for
//! the plugins, one for the themes, and the developer-mode switch. Every
//! other view of the Manager's panel is a page over the screen ([`page`]): a
//! bar with a back button and the page's title, and the page's rows in a
//! scroll under it. A plugin's page also shows the plugin's own panels. A
//! press on a row comes back as the [`Click`](rekindle_ui_hook::Click) the
//! panel's own control reports, from [`Rendered::presses`].

use std::collections::BTreeMap;

use rekindle_plugin_host::manager;
use rekindle_ui_hook::{CardItem, PanelItem, UiInput, Widget};
use rekindle_wire::a11y::{Role, ids as a11y};
use rekindle_wire::{ElementKind, NodeId, PropId, Value};
use rekindle_wire_inject::{Cell, Handler, Mount, MountNode, RESERVED, Selector, TEXT};
use serde::Deserialize;

/// The handler surface of a row in rekindle's menu on the settings screen.
pub(crate) const SECTION: &str = "settings";
/// The handler surface of a row of rekindle's page.
pub(crate) const PAGE: &str = "page";
/// The handler surface of a row of a plugin's panel on its page.
pub(crate) const PAGE_PANEL: &str = "page.panel";
/// The handler surface of a row in a plugin's modal.
pub(crate) const MODAL: &str = "modal";
/// The handler surface of the layer behind a modal: a press there closes it.
pub(crate) const MODAL_SCRIM: &str = "modal.scrim";
/// The handler surface of a modal's card, which a press on does nothing.
pub(crate) const MODAL_CARD: &str = "modal.card";
/// The key of a page's back button, on [`PAGE`].
pub(crate) const PAGE_BACK: &str = "page.back";

/// The first node id of rekindle's menu.
const SECTION_IDS: NodeId = RESERVED + 0x0200_0000;
/// The first node id of a plugin's modal.
const MODAL_IDS: NodeId = RESERVED + 0x0300_0000;
/// The first node ids of rekindle's page: two blocks, so a page of another
/// shape is created under new ids while the old one is taken out.
const PAGE_IDS: [NodeId; 2] = [RESERVED + 0x0400_0000, RESERVED + 0x0480_0000];

/// The cells each part of a row gets, and where the menu, pages and
/// modals go. Read from the client's own rows, so a mount looks like them.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Look {
    /// The node the menu is mounted under.
    pub(crate) under: Selector,
    /// The child of that node the menu goes before, if not last.
    #[serde(default)]
    pub(crate) before: Option<Selector>,
    /// The child of that node the menu goes after, if `before` finds none.
    #[serde(default)]
    pub(crate) after: Option<Selector>,
    /// The node a page or modal is mounted under: one whose children are
    /// drawn over each other, so the last is on top. Without one, pages and
    /// modals aren't shown.
    #[serde(default)]
    pub(crate) modal_under: Option<Selector>,
    /// A group's header with the group under it.
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
    /// What a track adds when a press on it does something of its own.
    #[serde(default)]
    pub(crate) track_press: Vec<Cell>,
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
    /// A page: the layer over the screen, its bar, the bar's back button
    /// and its icon, the title, the scroll and the column in it.
    #[serde(default)]
    pub(crate) page: Vec<Cell>,
    #[serde(default)]
    pub(crate) page_bar: Vec<Cell>,
    #[serde(default)]
    pub(crate) page_back: Vec<Cell>,
    #[serde(default)]
    pub(crate) page_back_icon: Vec<Cell>,
    #[serde(default)]
    pub(crate) page_title: Vec<Cell>,
    #[serde(default)]
    pub(crate) page_scroll: Vec<Cell>,
    #[serde(default)]
    pub(crate) page_content: Vec<Cell>,
    /// The `uri` of each row's icon, by the row's key: the key itself, then
    /// `<verb>:*` for a key `<verb>:<id>`. In the menu, `*` is the icon of a
    /// row with neither. A row with none has no icon.
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
    /// What a press on the row reports: the panel's element and its value.
    /// `None` for a row a press does nothing on.
    pub(crate) press: Option<(String, UiInput)>,
    /// A switch a press on does something of its own: the track's key and
    /// what the press reports. A press on the rest of the row is `press`.
    pub(crate) track: Option<(String, (String, UiInput))>,
}

impl Row {
    fn new(key: &str, title: &str, end: End, press: Option<(String, UiInput)>) -> Row {
        Row {
            key: key.into(),
            title: title.into(),
            subtitle: None,
            end,
            press,
            track: None,
        }
    }
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

/// The rows of rekindle's menu on the settings screen, from the widgets of
/// `Manager::menu`.
pub(crate) fn menu_items(menu: &[Widget]) -> Vec<Item> {
    let mut items = vec![Item::Header("rekindle".into())];
    items.extend(widget_items(menu));
    items
}

/// Whether the Manager's panel shows its grid, which the menu stands in
/// for. Any other view is a page.
pub(crate) fn is_grid(widgets: &[Widget]) -> bool {
    widgets.iter().any(|w| matches!(w, Widget::Tabs { .. }))
}

/// A card as a row. One of the Manager's plugin cards (`open:<plugin>`)
/// opens the plugin's page, and ends in a switch of its own when the card
/// can turn the plugin on or off. Any other card opens what its id says.
fn card_row(c: &CardItem) -> Item {
    let subtitle = [c.meta.as_str(), c.state.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let mut row = Row::new(
        &c.id,
        &c.title,
        End::Chevron,
        Some((c.id.clone(), UiInput::Clicked)),
    );
    row.subtitle = (!subtitle.is_empty()).then_some(subtitle);
    let on = match c.action.as_ref().and_then(|(id, _)| id.split_once(':')) {
        Some(("disable", _)) => Some(true),
        Some(("enable", _)) => Some(false),
        _ => None,
    };
    if let (Some(on), Some((action, _)), Some(plugin)) = (on, &c.action, c.id.strip_prefix("open:"))
    {
        // The track keeps the key `on:<plugin>` as it flips, so its node
        // stays, while its press is the card's own action.
        row.end = End::Switch(on);
        row.track = Some((format!("on:{plugin}"), (action.clone(), UiInput::Clicked)));
    }
    Item::Row(row)
}

/// A panel's widgets as rows. A select is a row that steps to the next
/// option on a press. Text fields, sliders and colours show their value
/// and can't be changed here yet.
pub(crate) fn widget_items(widgets: &[Widget]) -> Vec<Item> {
    let mut items = Vec::new();
    for w in widgets {
        match w {
            Widget::Heading(t) => items.push(Item::Header(t.clone())),
            Widget::Text(t) => items.push(Item::Text(t.clone())),
            Widget::Separator => items.push(Item::Break),
            Widget::Toggle { id, label, value } => items.push(Item::Row(Row::new(
                id,
                label,
                End::Switch(*value),
                Some((id.clone(), UiInput::Toggled(!value))),
            ))),
            Widget::Button { id, label } => items.push(Item::Row(Row::new(
                id,
                label,
                End::Chevron,
                Some((id.clone(), UiInput::Clicked)),
            ))),
            Widget::Header {
                back,
                title,
                meta,
                toggle,
                action,
            } => {
                let mut head = Row::new(
                    toggle.as_ref().map_or("header", |(id, _, _)| id),
                    title,
                    toggle
                        .as_ref()
                        .map_or(End::Nothing, |(_, _, on)| End::Switch(*on)),
                    toggle
                        .as_ref()
                        .map(|(id, _, on)| (id.clone(), UiInput::Toggled(!on))),
                );
                head.subtitle = (!meta.is_empty()).then(|| meta.clone());
                items.push(Item::Row(head));
                if let Some((id, label)) = action {
                    items.push(Item::Row(Row::new(
                        id,
                        label,
                        End::Chevron,
                        Some((id.clone(), UiInput::Clicked)),
                    )));
                }
                items.push(Item::Row(Row::new(
                    back,
                    "Back",
                    End::Chevron,
                    Some((back.clone(), UiInput::Clicked)),
                )));
            }
            Widget::Cards(cards) => items.extend(cards.iter().map(card_row)),
            Widget::Select {
                id,
                label,
                options,
                selected,
            } => {
                let mut row = Row::new(
                    id,
                    label,
                    End::Chevron,
                    (!options.is_empty()).then(|| {
                        let next = (*selected as usize + 1) % options.len();
                        (id.clone(), UiInput::Selected(next as u32))
                    }),
                );
                row.subtitle = options.get(*selected as usize).cloned();
                items.push(Item::Row(row));
            }
            Widget::TextInput {
                id, label, value, ..
            } => {
                let mut row = Row::new(id, label, End::Nothing, None);
                row.subtitle = (!value.is_empty()).then(|| value.clone());
                items.push(Item::Row(row));
            }
            Widget::Slider {
                id, label, value, ..
            } => {
                let mut row = Row::new(id, label, End::Nothing, None);
                row.subtitle = Some(format!("{value}"));
                items.push(Item::Row(row));
            }
            Widget::Color { id, label, hex } => {
                let mut row = Row::new(id, label, End::Nothing, None);
                row.subtitle = Some(hex.clone());
                items.push(Item::Row(row));
            }
            Widget::Tabs { .. } => {}
        }
    }
    items
}

/// A view of the Manager's panel as a page: its title, the element its back
/// button reports, and its rows. The panel's header gives the title and the
/// back; its switch and line stay as the page's first row. A view without a
/// header takes its title from its first heading, and goes back by its
/// Cancel button if it has one.
pub(crate) fn page_items(widgets: &[Widget]) -> (String, String, Vec<Item>) {
    let mut title = None;
    let mut back = None;
    let mut rest = Vec::new();
    for w in widgets {
        match w {
            Widget::Header {
                back: b,
                title: t,
                meta,
                toggle,
                action,
            } if title.is_none() => {
                title = Some(t.clone());
                back = Some(b.clone());
                match toggle {
                    Some((id, _, on)) => {
                        let mut row = Row::new(
                            id,
                            t,
                            End::Switch(*on),
                            Some((id.clone(), UiInput::Toggled(!on))),
                        );
                        row.subtitle = (!meta.is_empty()).then(|| meta.clone());
                        rest.push(Item::Row(row));
                    }
                    None if !meta.is_empty() => rest.push(Item::Header(meta.clone())),
                    None => {}
                }
                if let Some((id, label)) = action {
                    rest.push(Item::Row(Row::new(
                        id,
                        label,
                        End::Chevron,
                        Some((id.clone(), UiInput::Clicked)),
                    )));
                }
            }
            Widget::Heading(t) if title.is_none() => title = Some(t.clone()),
            w => rest.extend(widget_items(std::slice::from_ref(w))),
        }
    }
    let back = back.unwrap_or_else(|| {
        let cancel = widgets
            .iter()
            .any(|w| matches!(w, Widget::Button { id, .. } if id == "cancel"));
        if cancel { "cancel" } else { "back" }.to_string()
    });
    (title.unwrap_or_else(|| "rekindle".into()), back, rest)
}

/// rekindle's menu as a mount on the settings screen, from its rows.
pub(crate) fn section(look: &Look, items: &[Item]) -> Rendered {
    let mut b = Builder::new(look, SECTION_IDS);
    let content = b.content(items, manager::OWNER, SECTION, &owner_press);
    let mut cells = look.section.clone();
    cells.push(Cell::new(a11y::ID, Value::Str("rekindle.settings".into())));
    let root = b.node(ElementKind::VIEW, cells, content, None);
    b.finish(
        look.under.clone(),
        look.before.clone(),
        look.after.clone(),
        root,
    )
}

/// A press on one of rekindle's own rows: an interaction with the
/// Manager's panel.
fn owner_press((element, value): (String, UiInput)) -> Press {
    Press::Ui {
        plugin: manager::OWNER.into(),
        panel: manager::PANEL.into(),
        element,
        value,
    }
}

/// A view of the Manager's panel as a page over the screen: a bar with a
/// back button and the title, then the page's rows, then `panels`, the
/// panels of the plugin whose page it is. `block` (0 or 1) picks the page's
/// node ids. Nothing without a place for pages in the look.
pub(crate) fn page(
    look: &Look,
    widgets: &[Widget],
    panels: &[PanelItem],
    block: usize,
) -> Rendered {
    let Some(under) = &look.modal_under else {
        return Rendered::default();
    };
    let (title, back, items) = page_items(widgets);
    let mut b = Builder::new(look, PAGE_IDS[block % 2]);
    b.fallback_icon = false;
    let mut sections = b.content(&items, manager::OWNER, PAGE, &owner_press);
    for p in panels {
        let mut items = vec![Item::Header(p.title.clone())];
        // A panel's element ids are its own, so its rows' keys say which
        // panel they are in.
        items.extend(widget_items(&p.widgets).into_iter().map(|i| match i {
            Item::Row(mut r) => {
                r.key = format!("{}/{}", p.id, r.key);
                Item::Row(r)
            }
            other => other,
        }));
        let (plugin, panel) = (p.plugin.clone(), p.id.clone());
        sections.extend(
            b.content(&items, &p.plugin, PAGE_PANEL, &|(element, value)| {
                Press::Ui {
                    plugin: plugin.clone(),
                    panel: panel.clone(),
                    element,
                    value,
                }
            }),
        );
    }
    let content = b.node(ElementKind::VIEW, look.page_content.clone(), sections, None);
    let scroll = b.node(
        ElementKind::SCROLL,
        look.page_scroll.clone(),
        vec![content],
        None,
    );

    let icon = b.node(
        ElementKind::IMAGE,
        look.page_back_icon.clone(),
        Vec::new(),
        None,
    );
    let mut back_cells = look.page_back.clone();
    back_cells.extend([
        Cell::new(a11y::ROLE, Value::Str(Role::Button.name().into())),
        Cell::new(a11y::LABEL, Value::Str("Back".into())),
        Cell::new(a11y::ID, Value::Str("rekindle.page.back".into())),
    ]);
    let back_button = b.node(ElementKind::VIEW, back_cells, vec![icon], Some(page_back()));
    b.presses.insert(
        (PAGE.into(), PAGE_BACK.into()),
        owner_press((back, UiInput::Clicked)),
    );
    let title = b.text(&look.page_title.clone(), &title);
    let bar = b.node(
        ElementKind::VIEW,
        look.page_bar.clone(),
        vec![back_button, title],
        None,
    );
    let mut cells = look.page.clone();
    cells.push(Cell::new(a11y::ID, Value::Str("rekindle.page".into())));
    let root = b.node(ElementKind::VIEW, cells, vec![bar, scroll], None);
    b.finish(under.clone(), None, None, root)
}

/// What the system back does while a page is shown: what its back button
/// does.
pub(crate) fn page_back() -> Handler {
    handler(manager::OWNER, PAGE, PAGE_BACK)
}

/// What the system back does while `plugin`'s modal is shown: close it.
pub(crate) fn modal_back(plugin: &str) -> Handler {
    handler(plugin, MODAL_SCRIM, "")
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
    let content = b.content(&items, plugin, MODAL, &|(element, value)| Press::Ui {
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
    scrim.push(Cell::new(
        a11y::ID,
        Value::Str(format!("rekindle.{plugin}.modal")),
    ));
    let root = b.node(
        ElementKind::VIEW,
        scrim,
        vec![card],
        Some(modal_back(plugin)),
    );
    let mut r = b.finish(under.clone(), None, None, root);
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
    /// A row with no icon of its own gets the look's `*`.
    fallback_icon: bool,
    next: NodeId,
    nodes: Vec<MountNode>,
    presses: BTreeMap<(String, String), Press>,
}

impl<'l> Builder<'l> {
    fn new(look: &'l Look, first: NodeId) -> Self {
        Builder {
            look,
            icons: true,
            fallback_icon: true,
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

    /// The icon of a row with this key, if it has one.
    fn icon(&self, key: &str) -> Option<String> {
        if !self.icons {
            return None;
        }
        let icons = &self.look.icons;
        icons
            .get(key)
            .or_else(|| {
                let (verb, _) = key.split_once(':')?;
                icons.get(&format!("{verb}:*"))
            })
            .or_else(|| self.fallback_icon.then(|| icons.get("*")).flatten())
            .cloned()
    }

    /// The groups of `items`, each under its header in a section of its
    /// own. A row with a press gets a handler for `plugin` on `surface`,
    /// and `press` makes what it reports.
    fn content(
        &mut self,
        items: &[Item],
        plugin: &str,
        surface: &str,
        press: &dyn Fn((String, UiInput)) -> Press,
    ) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut header: Option<NodeId> = None;
        let mut group: Vec<&Item> = Vec::new();
        for item in items {
            match item {
                Item::Header(t) => {
                    self.close(&mut header, &mut group, &mut out, plugin, surface, press);
                    header = Some(self.text(&self.look.header.clone(), t));
                }
                Item::Break => {
                    self.close(&mut header, &mut group, &mut out, plugin, surface, press);
                }
                Item::Text(_) | Item::Row(_) => group.push(item),
            }
        }
        self.close(&mut header, &mut group, &mut out, plugin, surface, press);
        out
    }

    /// End the section being built: its header and a group of its rows.
    fn close(
        &mut self,
        header: &mut Option<NodeId>,
        group: &mut Vec<&Item>,
        out: &mut Vec<NodeId>,
        plugin: &str,
        surface: &str,
        press: &dyn Fn((String, UiInput)) -> Press,
    ) {
        let mut children: Vec<NodeId> = header.take().into_iter().collect();
        let rows = std::mem::take(group);
        let n = rows.len();
        let mut list = Vec::new();
        for (i, item) in rows.into_iter().enumerate() {
            if i > 0 {
                let rule = self.node(ElementKind::VIEW, self.look.rule.clone(), Vec::new(), None);
                let divider = self.node(
                    ElementKind::VIEW,
                    self.look.divider.clone(),
                    vec![rule],
                    None,
                );
                list.push(divider);
            }
            list.push(self.row(item, i == 0, i + 1 == n, plugin, surface, press));
        }
        if !list.is_empty() {
            children.push(self.node(ElementKind::VIEW, self.look.group.clone(), list, None));
        }
        if !children.is_empty() {
            out.push(self.node(ElementKind::VIEW, self.look.section.clone(), children, None));
        }
    }

    fn row(
        &mut self,
        item: &Item,
        first: bool,
        last: bool,
        plugin: &str,
        surface: &str,
        press: &dyn Fn((String, UiInput)) -> Press,
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
                if let Some(u) = self.icon(&r.key) {
                    let mut c = vec![Cell::new(uri(), Value::Str(u))];
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
                        let mut track_cells = state(&look.track, &look.track_on, &look.track_off);
                        let track_press = r.track.as_ref().map(|(key, p)| {
                            track_cells.extend(look.track_press.iter().cloned());
                            track_cells.push(Cell::new(
                                a11y::ID,
                                Value::Str(format!("rekindle.{plugin}.{key}")),
                            ));
                            self.presses
                                .insert((surface.into(), key.clone()), press(p.clone()));
                            handler(plugin, surface, key)
                        });
                        let track =
                            self.node(ElementKind::VIEW, track_cells, vec![thumb], track_press);
                        inner.push(self.node(
                            ElementKind::VIEW,
                            look.switch_box.clone(),
                            vec![track],
                            None,
                        ));
                        cells.push(Cell::new(a11y::CHECKED, Value::Bool(on)));
                    }
                }
                // A row whose switch has a press of its own opens
                // something: it is a button, not a switch.
                let role = match r.end {
                    End::Switch(_) if r.track.is_none() => Role::Switch,
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

    fn finish(
        self,
        under: Selector,
        before: Option<Selector>,
        after: Option<Selector>,
        root: NodeId,
    ) -> Rendered {
        Rendered {
            mounts: vec![Mount {
                under,
                after,
                before,
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
            "before": {"kind": "view"},
            "modal_under": {"kind": "stack"},
            "section": [], "header": [{"prop": "role", "value": {"str": "header"}}],
            "group": [], "row": [], "first": [{"prop": "radiusTopLeft", "value": {"f64": 12}}],
            "last": [{"prop": "radiusBottomLeft", "value": {"f64": 12}}],
            "inner": [], "icon": [], "column": [], "title": [], "subtitle": [],
            "chevron_box": [], "chevron": [], "switch_box": [], "track": [],
            "track_on": [{"prop": "background", "value": {"rgba": "5865f2ff"}}],
            "track_off": [{"prop": "background", "value": {"rgba": "0000001f"}}],
            "track_press": [{"prop": "exactHitArea", "value": {"bool": true}}],
            "thumb": [], "thumb_on": [{"prop": "translateX", "value": {"f64": 28}}],
            "thumb_off": [{"prop": "translateX", "value": {"f64": 0}}],
            "divider": [], "rule": [], "scrim": [], "card": [],
            "page": [{"prop": "absolute", "value": {"bool": true}}],
            "page_bar": [], "page_back": [], "page_title": [], "page_scroll": [],
            "page_content": [],
            "page_back_icon": [{"prop": "uri", "value": {"str": "res://back"}}],
            "icons": {"*": "res://plugin", "developer-mode": "res://warning", "open:*": "res://card"}
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

    fn menu() -> Vec<Widget> {
        let list = |id: &str, title: &str| CardItem {
            id: format!("list:{id}"),
            title: title.into(),
            meta: "3 installed, 1 on".into(),
            text: String::new(),
            state: String::new(),
            action: None,
        };
        vec![
            Widget::Cards(vec![list("plugins", "Plugins"), list("themes", "Themes")]),
            Widget::Toggle {
                id: "developer-mode".into(),
                label: "Developer Mode".into(),
                value: false,
            },
        ]
    }

    // The Manager's list of plugins.
    fn list() -> Vec<Widget> {
        vec![
            Widget::Header {
                back: "back".into(),
                title: "Plugins".into(),
                meta: "3 installed, 1 on".into(),
                toggle: None,
                action: None,
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

    fn cell(n: &MountNode, p: PropId) -> Option<&Value> {
        n.cells.iter().find(|c| c.prop == p).map(|c| &c.value)
    }

    fn ui(element: &str, value: UiInput) -> Press {
        Press::Ui {
            plugin: manager::OWNER.into(),
            panel: manager::PANEL.into(),
            element: element.into(),
            value,
        }
    }

    #[test]
    fn the_menu_is_a_row_per_list_and_the_developer_mode_switch() {
        let items = menu_items(&menu());
        assert_eq!(items[0], Item::Header("rekindle".into()));
        let plugins = row(&items, "list:plugins");
        assert_eq!(plugins.end, End::Chevron);
        assert_eq!(
            plugins.press,
            Some(("list:plugins".into(), UiInput::Clicked))
        );
        assert_eq!(plugins.subtitle.as_deref(), Some("3 installed, 1 on"));
        let dev = row(&items, "developer-mode");
        assert_eq!(dev.end, End::Switch(false));
        assert_eq!(
            dev.press,
            Some(("developer-mode".into(), UiInput::Toggled(true)))
        );

        let look = look();
        let r = section(&look, &items);
        Work {
            mounts: r.mounts.clone(),
            ..Work::default()
        }
        .check()
        .unwrap();
        // It goes before the look's `before`, at the top of the list.
        assert_eq!(r.mounts[0].before, look.before);
        let p = node(&r, "rekindle.rekindle.list:plugins");
        assert_eq!(
            p.on_press,
            Some(handler(manager::OWNER, SECTION, "list:plugins"))
        );
        assert_eq!(
            r.presses[&(SECTION.to_string(), "list:plugins".to_string())],
            ui("list:plugins", UiInput::Clicked)
        );
        let dev = node(&r, "rekindle.rekindle.developer-mode");
        assert!(cell(dev, PropId::from_name("radiusBottomLeft").unwrap()).is_some());
        assert_eq!(cell(dev, a11y::ROLE), Some(&Value::Str("switch".into())));
    }

    #[test]
    fn a_list_is_a_page_whose_rows_open_and_whose_switches_flip() {
        assert!(!is_grid(&list()));
        let (title, back, items) = page_items(&list());
        assert_eq!((title.as_str(), back.as_str()), ("Plugins", "back"));
        assert_eq!(items[0], Item::Header("3 installed, 1 on".into()));
        // The row opens the plugin's page, and its switch is the card's
        // own action.
        let a = row(&items, "open:dev.a");
        assert_eq!(a.press, Some(("open:dev.a".into(), UiInput::Clicked)));
        assert_eq!(a.end, End::Switch(true));
        assert_eq!(
            a.track,
            Some((
                "on:dev.a".into(),
                ("disable:dev.a".into(), UiInput::Clicked)
            ))
        );
        // One that can't be turned on here only opens.
        let c = row(&items, "open:dev.c");
        assert_eq!((c.end, c.track), (End::Chevron, None));

        let look = look();
        let r = page(&look, &list(), &[], 0);
        Work {
            mounts: r.mounts.clone(),
            ..Work::default()
        }
        .check()
        .unwrap();
        assert_eq!(r.mounts[0].under, look.modal_under.clone().unwrap());
        assert!(r.mounts[0].root >= PAGE_IDS[0] && r.mounts[0].root < PAGE_IDS[1]);
        let track = node(&r, "rekindle.rekindle.on:dev.a");
        assert_eq!(
            track.on_press,
            Some(handler(manager::OWNER, PAGE, "on:dev.a"))
        );
        assert_eq!(
            cell(track, PropId::from_name("exactHitArea").unwrap()),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            r.presses[&(PAGE.to_string(), "on:dev.a".to_string())],
            ui("disable:dev.a", UiInput::Clicked)
        );
        assert_eq!(
            r.presses[&(PAGE.to_string(), "open:dev.a".to_string())],
            ui("open:dev.a", UiInput::Clicked)
        );
        let a = node(&r, "rekindle.rekindle.open:dev.a");
        assert_eq!(cell(a, a11y::ROLE), Some(&Value::Str("button".into())));
        // The back button and the system back report the header's back.
        let b = node(&r, "rekindle.page.back");
        assert_eq!(b.on_press, Some(page_back()));
        assert_eq!(
            r.presses[&(PAGE.to_string(), PAGE_BACK.to_string())],
            ui("back", UiInput::Clicked)
        );
        // Plugin rows get the `open:*` icon, and nothing falls back to `*`.
        let uris: Vec<&Value> = r.mounts[0]
            .nodes
            .iter()
            .filter_map(|n| cell(n, uri()))
            .collect();
        assert!(uris.contains(&&Value::Str("res://card".into())));
        assert!(!uris.contains(&&Value::Str("res://plugin".into())));

        // The other block, for a page of another shape.
        let other = page(&look, &list(), &[], 1);
        assert!(other.mounts[0].root >= PAGE_IDS[1]);

        // A flipped switch keeps every node and changes only cells.
        let mut flipped = list();
        if let Widget::Cards(c) = &mut flipped[1] {
            c[0].action = Some(("enable:dev.a".into(), "Turn on".into()));
            c[0].state = "Off".into();
        }
        let again = page(&look, &flipped, &[], 0);
        assert!(again.mounts[0].same_shape(&r.mounts[0]));
        assert_eq!(
            cell(node(&again, "rekindle.rekindle.open:dev.a"), a11y::CHECKED),
            Some(&Value::Bool(false))
        );
    }

    #[test]
    fn a_plugins_page_has_its_switch_and_its_own_panels() {
        let detail = vec![
            Widget::Header {
                back: "back".into(),
                title: "A 0.1.0".into(),
                meta: "Part of rekindle".into(),
                toggle: Some(("on:dev.a".into(), "On".into(), true)),
                action: Some(("remove:dev.a".into(), "Remove".into())),
            },
            Widget::Text("Does a thing.".into()),
        ];
        let panel = PanelItem {
            plugin: "dev.a".into(),
            id: "settings".into(),
            title: "Settings".into(),
            widgets: vec![
                Widget::Toggle {
                    id: "loud".into(),
                    label: "Loud".into(),
                    value: false,
                },
                Widget::Select {
                    id: "mode".into(),
                    label: "Mode".into(),
                    options: vec!["one".into(), "two".into()],
                    selected: 1,
                },
            ],
        };
        let (title, back, items) = page_items(&detail);
        assert_eq!((title.as_str(), back.as_str()), ("A 0.1.0", "back"));
        assert_eq!(
            row(&items, "on:dev.a").press,
            Some(("on:dev.a".into(), UiInput::Toggled(false)))
        );
        assert!(
            items
                .iter()
                .all(|i| !matches!(i, Item::Row(r) if r.title == "Back"))
        );

        let r = page(&look(), &detail, &[panel], 0);
        Work {
            mounts: r.mounts.clone(),
            ..Work::default()
        }
        .check()
        .unwrap();
        let loud = node(&r, "rekindle.dev.a.settings/loud");
        assert_eq!(
            loud.on_press,
            Some(handler("dev.a", PAGE_PANEL, "settings/loud"))
        );
        assert_eq!(
            r.presses[&(PAGE_PANEL.to_string(), "settings/loud".to_string())],
            Press::Ui {
                plugin: "dev.a".into(),
                panel: "settings".into(),
                element: "loud".into(),
                value: UiInput::Toggled(true),
            }
        );
        // A select steps to its next option, round to the first.
        assert_eq!(
            r.presses[&(PAGE_PANEL.to_string(), "settings/mode".to_string())],
            Press::Ui {
                plugin: "dev.a".into(),
                panel: "settings".into(),
                element: "mode".into(),
                value: UiInput::Selected(0),
            }
        );
    }

    #[test]
    fn a_page_without_a_header_takes_its_heading_and_goes_back_by_cancel() {
        let page_widgets = vec![
            Widget::Heading("Turn on developer mode?".into()),
            Widget::Text("Plugins from outside rekindle can do anything.".into()),
            Widget::Button {
                id: "cancel".into(),
                label: "Cancel".into(),
            },
        ];
        let (title, back, items) = page_items(&page_widgets);
        assert_eq!(title, "Turn on developer mode?");
        assert_eq!(back, "cancel");
        assert!(matches!(&items[0], Item::Text(_)));
        // No place for pages, no page.
        let mut no = look();
        no.modal_under = None;
        assert_eq!(page(&no, &page_widgets, &[], 0), Rendered::default());
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
        assert!(!m.nodes.iter().any(|n| {
            n.kind == ElementKind::IMAGE
                && n.cells
                    .iter()
                    .any(|c| c.value == Value::Str("res://plugin".into()))
        }));
        assert_eq!(layer.on_press, Some(modal_back("dev.a")));
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
        // With the menu, the presses of both are kept.
        let both = join([section(&look(), &menu_items(&menu())), r.clone()]);
        assert_eq!(both.mounts.len(), 2);
        assert_eq!(both.presses.len(), r.presses.len() + 3);
        // No place for modals, no modal.
        let mut no = look();
        no.modal_under = None;
        assert_eq!(modal(&no, "dev.a", &panel), Rendered::default());
    }
}
