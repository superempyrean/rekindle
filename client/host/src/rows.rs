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
//! The rows are `rekindle_ui_hook::panel::rows`. The settings screen gets
//! the Manager's menu ([`section`]), and every other view of the Manager's
//! panel is a page over the screen ([`page`]): a bar with a back button and
//! the page's title, and the page's rows in a scroll under it. A press on a
//! row comes back as the [`Click`](rekindle_ui_hook::Click) the panel's own
//! control reports, from [`Rendered::presses`].

use std::collections::BTreeMap;

use rekindle_plugin_host::manager;
use rekindle_ui_hook::panel::rows::{End, Item, Page, widget_items};
use rekindle_ui_hook::{LiveToast, PanelItem, ToastStyle, UiInput, Widget};
use rekindle_wire::a11y::{Role, ids as a11y};
use rekindle_wire::{ElementKind, NodeId, PropId, Value};
use rekindle_wire_inject::{Cell, Handler, Mount, MountNode, RESERVED, Selector, TEXT};

pub(crate) use crate::look::Look;

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
/// The handler surface of a row in a plugin's insert next to one of the
/// client's elements.
pub(crate) const INSERT: &str = "element.insert";

/// The first node id of rekindle's menu.
const SECTION_IDS: NodeId = RESERVED + 0x0200_0000;
/// The first node id of a plugin's modal.
const MODAL_IDS: NodeId = RESERVED + 0x0300_0000;
/// The first node id of the toasts. Each set of toasts gets ids of its
/// own, [`TOAST_STRIDE`] apart, so a new set is created while the old one
/// is taken out without reusing an id.
const TOAST_IDS: NodeId = RESERVED + 0x0500_0000;
const TOAST_STRIDE: NodeId = 0x40;
/// How many sets of toasts get ids before they are used again.
const TOAST_SETS: u32 = 0x1_0000;
/// The first node ids of rekindle's page: two blocks, so a page of another
/// shape is created under new ids while the old one is taken out.
const PAGE_IDS: [NodeId; 2] = [RESERVED + 0x0400_0000, RESERVED + 0x0480_0000];

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

/// A page over the screen: a bar with a back button and the title, then the
/// page's rows, then the panels of the plugin whose page it is. `block` (0
/// or 1) picks the page's node ids. Nothing without a place for pages in
/// the look.
pub(crate) fn page(look: &Look, page: &Page, block: usize) -> Rendered {
    let Some(under) = look.page_under.as_ref().or(look.modal_under.as_ref()) else {
        return Rendered::default();
    };
    let Page {
        title,
        back,
        items,
        panels,
    } = page;
    let mut b = Builder::new(look, PAGE_IDS[block % 2]);
    b.fallback_icon = false;
    let mut sections = b.content(items, manager::OWNER, PAGE, &owner_press);
    for p in panels {
        let (plugin, panel) = (p.plugin.clone(), p.panel.clone());
        sections.extend(
            b.content(&p.items, &p.plugin, PAGE_PANEL, &|(element, value)| {
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
        owner_press((back.clone(), UiInput::Clicked)),
    );
    let title = b.text(&look.page_title.clone(), title);
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

/// A plugin's insert `id`, `widgets` drawn as a panel's rows, as a mount
/// with its nodes from `first` up: under the node `under` finds, before or
/// after the child `before` or `after` finds. A press on a row reports what
/// the same row of a panel `insert:<id>` would.
pub(crate) fn insert(
    look: &Look,
    plugin: &str,
    id: &str,
    widgets: &[Widget],
    at: (Selector, Option<Selector>, Option<Selector>),
    first: NodeId,
) -> Rendered {
    let mut b = Builder::new(look, first);
    b.icons = false;
    // Two plugins' inserts can have rows with the same ids.
    let items: Vec<Item> = widget_items(widgets)
        .into_iter()
        .map(|i| match i {
            Item::Row(mut r) => {
                r.key = format!("{plugin}/{id}/{}", r.key);
                Item::Row(r)
            }
            other => other,
        })
        .collect();
    let panel = format!("insert:{id}");
    let content = b.content(&items, plugin, INSERT, &|(element, value)| Press::Ui {
        plugin: plugin.into(),
        panel: panel.clone(),
        element,
        value,
    });
    let cells = vec![Cell::new(
        a11y::ID,
        Value::Str(format!("rekindle.{plugin}.insert.{id}")),
    )];
    let root = b.node(ElementKind::VIEW, cells, content, None);
    let (under, before, after) = at;
    b.finish(under, before, after, root)
}

/// The toasts that are up as a mount over the screen, oldest on top. `set`
/// counts the sets of toasts shown so far, and picks the mount's ids.
/// `fill`, `0xRRGGBBAA`, is the cards' background in place of the look's:
/// the `toast-background` a plugin set. Nothing when no toast is up, or the
/// look has no place for them.
pub(crate) fn toasts(look: &Look, toasts: &[LiveToast], set: u32, fill: Option<u32>) -> Rendered {
    let Some(under) = &look.modal_under else {
        return Rendered::default();
    };
    if toasts.is_empty() || look.toast_layer.is_empty() {
        return Rendered::default();
    }
    let mut b = Builder::new(look, TOAST_IDS + (set % TOAST_SETS) * TOAST_STRIDE);
    let root = b.next;
    b.next += 1;
    let mut cards = Vec::new();
    for t in toasts.iter().take(rekindle_ui_hook::MAX_TOASTS) {
        let mut children = Vec::new();
        if let Some(bar) = look.toast_bar.get(toast_style(t.style)) {
            children.push(b.node(ElementKind::VIEW, bar.clone(), Vec::new(), None));
        }
        children.push(b.text(&look.toast_text, &t.text));
        let mut cells = look.toast.clone();
        if let Some(fill) = fill {
            cells.retain(|c| c.prop != rekindle_wire::ids::BACKGROUND);
            cells.push(Cell::new(
                rekindle_wire::ids::BACKGROUND,
                rekindle_wire_inject::rgba(fill),
            ));
        }
        cells.push(Cell::new(a11y::LABEL, Value::Str(t.text.clone())));
        cards.push(b.node(ElementKind::VIEW, cells, children, None));
    }
    let mut layer = look.toast_layer.clone();
    layer.push(Cell::new(a11y::ID, Value::Str("rekindle.toasts".into())));
    b.nodes.push(MountNode {
        id: root,
        kind: ElementKind::VIEW,
        cells: layer,
        children: cards,
        on_press: None,
    });
    b.finish(under.clone(), None, None, root)
}

/// The key of a toast's style in [`Look::toast_bar`].
fn toast_style(style: ToastStyle) -> &'static str {
    match style {
        ToastStyle::Info => "info",
        ToastStyle::Success => "success",
        ToastStyle::Warning => "warning",
        ToastStyle::Error => "error",
    }
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

/// The `uri` cell (542) of an image.
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

    /// A row's text field, whose edits report `UiInput::Text` for `key`.
    fn input(
        &mut self,
        plugin: &str,
        surface: &str,
        key: &str,
        (element, value, placeholder): &(String, String, String),
        press: &dyn Fn((String, UiInput)) -> Press,
    ) -> NodeId {
        let prop = |name| PropId::from_name(name).expect("a PropId of the table");
        let mut cells = if self.look.input.is_empty() {
            let mut c = self.look.title.clone();
            if let Some(color) = self.look.subtitle.iter().find(|c| c.prop == prop("color")) {
                c.push(Cell::new(prop("placeholderColor"), color.value.clone()));
            }
            c
        } else {
            self.look.input.clone()
        };
        cells.push(Cell::new(prop("value"), Value::Str(value.clone())));
        if !placeholder.is_empty() {
            cells.push(Cell::new(
                prop("placeholder"),
                Value::Str(placeholder.into()),
            ));
        }
        cells.push(Cell::new(
            a11y::ID,
            Value::Str(format!("rekindle.{plugin}.{key}.input")),
        ));
        let edit = (element.clone(), UiInput::Text(value.clone()));
        self.presses
            .insert((surface.into(), key.into()), press(edit));
        self.node(
            ElementKind::INPUT,
            cells,
            Vec::new(),
            Some(handler(plugin, surface, key)),
        )
    }

    /// The icon of a row with this key, if it has one.
    fn icon(&self, key: &str) -> Option<String> {
        if !self.icons {
            return None;
        }
        self.look.icon(key, self.fallback_icon).map(str::to_string)
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
                if let Some(field) = &r.input {
                    lines.push(self.input(plugin, surface, &r.key, field, press));
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

/// The look the tests draw with.
#[cfg(test)]
pub(crate) fn test_look() -> Look {
    tests::look()
}

#[cfg(test)]
mod tests {
    use rekindle_ui_hook::CardItem;
    use rekindle_ui_hook::panel::rows::{menu_items, page as page_of};
    use rekindle_wire_inject::Work;

    use super::*;

    pub(super) fn look() -> Look {
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
        let look = look();
        let r = page(&look, &page_of(&list(), &[]), 0);
        Work {
            mounts: r.mounts.clone(),
            ..Work::default()
        }
        .check()
        .unwrap();
        assert_eq!(r.mounts[0].under, look.modal_under.clone().unwrap());
        assert!(r.mounts[0].root >= PAGE_IDS[0] && r.mounts[0].root < PAGE_IDS[1]);
        // A look with a place of its own for pages puts them there, and
        // leaves modals where they were.
        let mut sheet = look.clone();
        sheet.page_under = Some(Selector::cell(a11y::ID, "settings-phone"));
        let r2 = page(&sheet, &page_of(&list(), &[]), 0);
        assert_eq!(r2.mounts[0].under, sheet.page_under.clone().unwrap());
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
        let other = page(&look, &page_of(&list(), &[]), 1);
        assert!(other.mounts[0].root >= PAGE_IDS[1]);

        // A flipped switch keeps every node and changes only cells.
        let mut flipped = list();
        if let Widget::Cards(c) = &mut flipped[1] {
            c[0].action = Some(("enable:dev.a".into(), "Turn on".into()));
            c[0].state = "Off".into();
        }
        let again = page(&look, &page_of(&flipped, &[]), 0);
        assert!(again.mounts[0].same_shape(&r.mounts[0]));
        assert_eq!(
            cell(node(&again, "rekindle.rekindle.open:dev.a"), a11y::CHECKED),
            Some(&Value::Bool(false))
        );
    }

    #[test]
    fn a_plugins_own_panels_report_to_the_plugin() {
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
        let r = page(&look(), &page_of(&detail, &[panel]), 0);
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
    fn a_text_field_is_an_input_whose_edits_report_its_text() {
        let panel = PanelItem {
            plugin: "dev.a".into(),
            id: "settings".into(),
            title: "Settings".into(),
            widgets: vec![Widget::TextInput {
                id: "name".into(),
                label: "Name".into(),
                value: "Ann".into(),
                placeholder: "Who".into(),
            }],
        };
        let r = page(
            &look(),
            &page_of(&[Widget::Heading("A".into())], &[panel]),
            0,
        );
        Work {
            mounts: r.mounts.clone(),
            ..Work::default()
        }
        .check()
        .unwrap();
        let field = node(&r, "rekindle.dev.a.settings/name.input");
        assert_eq!(field.kind, ElementKind::INPUT);
        assert_eq!(
            field.on_press,
            Some(handler("dev.a", PAGE_PANEL, "settings/name"))
        );
        let cell = |name| {
            let prop = PropId::from_name(name).unwrap();
            field
                .cells
                .iter()
                .find(|c| c.prop == prop)
                .map(|c| c.value.clone())
        };
        assert_eq!(cell("value"), Some(Value::Str("Ann".into())));
        assert_eq!(cell("placeholder"), Some(Value::Str("Who".into())));
        assert_eq!(
            r.presses[&(PAGE_PANEL.to_string(), "settings/name".to_string())],
            Press::Ui {
                plugin: "dev.a".into(),
                panel: "settings".into(),
                element: "name".into(),
                value: UiInput::Text("Ann".into()),
            }
        );
    }

    #[test]
    fn without_a_place_for_pages_there_is_no_page() {
        let page_widgets = vec![
            Widget::Heading("Turn on developer mode?".into()),
            Widget::Text("Plugins from outside rekindle can do anything.".into()),
            Widget::Button {
                id: "cancel".into(),
                label: "Cancel".into(),
            },
        ];
        let mut no = look();
        no.modal_under = None;
        assert_eq!(
            page(&no, &page_of(&page_widgets, &[]), 0),
            Rendered::default()
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

    fn toast(id: u64, text: &str, style: ToastStyle) -> LiveToast {
        LiveToast {
            id,
            text: text.into(),
            style,
        }
    }

    #[test]
    fn toasts_are_cards_on_a_layer_that_takes_no_press() {
        let mut l = look();
        // No layer in the look, no toasts.
        let up = [
            toast(1, "Saved", ToastStyle::Success),
            toast(2, "Oops", ToastStyle::Error),
        ];
        assert_eq!(toasts(&l, &up, 0, None), Rendered::default());
        l.toast_layer = vec![Cell::new(
            PropId::from_name("absolute").unwrap(),
            Value::Bool(true),
        )];
        l.toast_bar.insert(
            "error".into(),
            vec![Cell::new(
                PropId::from_name("background").unwrap(),
                Value::Str("red".into()),
            )],
        );
        let r = toasts(&l, &up, 3, None);
        assert!(r.presses.is_empty());
        let [m] = r.mounts.as_slice() else {
            panic!("one mount: {:?}", r.mounts)
        };
        assert_eq!(m.root, TOAST_IDS + 3 * TOAST_STRIDE);
        assert!(m.nodes.iter().all(|n| n.on_press.is_none()));
        let root = m.nodes.iter().find(|n| n.id == m.root).unwrap();
        assert_eq!(root.children.len(), 2);
        let texts: Vec<&Value> = m
            .nodes
            .iter()
            .flat_map(|n| &n.cells)
            .filter(|c| c.prop == TEXT)
            .map(|c| &c.value)
            .collect();
        assert_eq!(
            texts,
            [&Value::Str("Saved".into()), &Value::Str("Oops".into())]
        );
        // Only the error toast has a bar: the look has none for success.
        let error = m.nodes.iter().find(|n| n.id == root.children[1]).unwrap();
        assert_eq!(error.children.len(), 2);
        // The next set has ids of its own, and the work takes both.
        let next = toasts(&l, &up[1..], 4, None);
        let ids =
            |r: &Rendered| -> Vec<NodeId> { r.mounts[0].nodes.iter().map(|n| n.id).collect() };
        assert!(ids(&r).iter().all(|i| !ids(&next).contains(i)));
        let mut work = Work::default();
        work.mounts.extend(join([r, next]).mounts);
        work.check().unwrap();
        assert_eq!(toasts(&l, &[], 5, None), Rendered::default());
    }

    #[test]
    fn a_plugins_toast_background_is_the_cards_fill() {
        let mut l = look();
        l.toast_layer = vec![Cell::new(
            PropId::from_name("absolute").unwrap(),
            Value::Bool(true),
        )];
        l.toast = vec![Cell::new(
            rekindle_wire::ids::BACKGROUND,
            rekindle_wire_inject::rgba(0x3232_38ff),
        )];
        let r = toasts(
            &l,
            &[toast(1, "Saved", ToastStyle::Info)],
            0,
            Some(0x2a1f_3dff),
        );
        let fills: Vec<&Value> = r.mounts[0]
            .nodes
            .iter()
            .flat_map(|n| &n.cells)
            .filter(|c| c.prop == rekindle_wire::ids::BACKGROUND)
            .map(|c| &c.value)
            .collect();
        assert_eq!(fills, [&rekindle_wire_inject::rgba(0x2a1f_3dff)]);
    }
}
