//! rekindle's panels as rows: the shape every client that draws rekindle's
//! settings as a list of rows renders, whatever it draws a row with.
//!
//! The Manager's panel shows its grid until the user opens something from
//! it. While it does, the client's settings screen shows the Manager's menu
//! ([`menu_items`]): a row for the plugins, one for the themes, and the
//! developer-mode switch. Every other view of the panel is a [`Page`]: a
//! title, the element its back button reports, the page's rows and, on a
//! plugin's page, the plugin's own panels under them. [`shown`] picks
//! between the two.
//!
//! A press on a row reports what the panel's own control would: the
//! element in [`Row::press`] (or [`Row::track`] for a switch with a press of
//! its own) and its value, for the panel the row is in.

use super::{CardItem, PanelItem, UiInput, Widget};

/// One line of a panel, as rows show it.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    /// The start of a group under a header.
    Header(String),
    /// The start of a group without one.
    Break,
    /// A line of text in a row of its own.
    Text(String),
    Row(Row),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The row's id among its panel's rows.
    pub key: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub end: End,
    /// What a press on the row reports: the panel's element and its value.
    /// `None` for a row a press does nothing on.
    pub press: Option<(String, UiInput)>,
    /// A switch a press on does something of its own: the track's key and
    /// what the press reports. A press on the rest of the row is `press`.
    pub track: Option<(String, (String, UiInput))>,
    /// A text field under the title: the panel's element, its text and
    /// its placeholder. An edit reports `UiInput::Text` for the element.
    pub input: Option<Box<(String, String, String)>>,
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
            input: None,
        }
    }
}

/// What a row ends in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Nothing,
    Chevron,
    Switch(bool),
}

/// A view of the Manager's panel as a page.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub title: String,
    /// The element the page's back button reports, with `Clicked`.
    pub back: String,
    /// The rows of the Manager's panel.
    pub items: Vec<Item>,
    /// The panels of the plugin whose page it is, under the Manager's rows.
    pub panels: Vec<PagePanel>,
}

/// A plugin's panel on its page. A press on a row reports the row's
/// element for `plugin`'s panel `panel`.
#[derive(Clone, Debug, PartialEq)]
pub struct PagePanel {
    pub plugin: String,
    pub panel: String,
    /// The panel's title as a header, then its rows. A row's key is
    /// `<panel>/<element>`, since two panels can have the same element ids.
    pub items: Vec<Item>,
}

/// What rekindle's settings show of the Manager's panel.
#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    /// The panel shows its grid, and the menu's rows stand in for it.
    Menu(Vec<Item>),
    Page(Page),
}

/// What rekindle's settings show now: the menu's rows from `menu`
/// (`Manager::menu`) while the Manager's panel `widgets` shows its grid,
/// and otherwise the page, with `own` under it. `own` is the panels of the
/// plugin whose page is open (`shown_plugin`).
pub fn shown(menu: &[Widget], widgets: &[Widget], own: &[PanelItem]) -> Shown {
    if is_grid(widgets) {
        Shown::Menu(menu_items(menu))
    } else {
        Shown::Page(page(widgets, own))
    }
}

/// The rows of rekindle's menu on the settings screen, from the widgets of
/// `Manager::menu`.
pub fn menu_items(menu: &[Widget]) -> Vec<Item> {
    let mut items = vec![Item::Header("rekindle".into())];
    items.extend(widget_items(menu));
    items
}

/// Whether the Manager's panel shows its grid, which the menu stands in
/// for. Any other view is a page.
pub fn is_grid(widgets: &[Widget]) -> bool {
    widgets.iter().any(|w| matches!(w, Widget::Tabs { .. }))
}

/// A card as a row. Its subtitle is the card's line and its state, those
/// that aren't empty. One of the Manager's plugin cards (`open:<plugin>`)
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
        // The track keeps the key `on:<plugin>` as it flips, so a client
        // that keys its nodes by row keeps the node, while its press is the
        // card's own action.
        row.end = End::Switch(on);
        row.track = Some((format!("on:{plugin}"), (action.clone(), UiInput::Clicked)));
    }
    Item::Row(row)
}

/// A panel's widgets as rows. A select is a row that steps to the next
/// option on a press. Sliders and colours show their value and can't be
/// changed here yet.
pub fn widget_items(widgets: &[Widget]) -> Vec<Item> {
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
                id,
                label,
                value,
                placeholder,
            } => {
                let mut row = Row::new(id, label, End::Nothing, None);
                row.input = Some(Box::new((id.clone(), value.clone(), placeholder.clone())));
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
pub fn page_items(widgets: &[Widget]) -> (String, String, Vec<Item>) {
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

/// A plugin's panel as rows on its page: the panel's title as a header,
/// then its widgets' rows, each keyed `<panel>/<element>`.
pub fn panel_items(p: &PanelItem) -> Vec<Item> {
    let mut items = vec![Item::Header(p.title.clone())];
    items.extend(widget_items(&p.widgets).into_iter().map(|i| match i {
        Item::Row(mut r) => {
            r.key = format!("{}/{}", p.id, r.key);
            Item::Row(r)
        }
        other => other,
    }));
    items
}

/// The Manager's panel `widgets` as a page, with the panels `own` under its
/// rows.
pub fn page(widgets: &[Widget], own: &[PanelItem]) -> Page {
    let (title, back, items) = page_items(widgets);
    Page {
        title,
        back,
        items,
        panels: own
            .iter()
            .map(|p| PagePanel {
                plugin: p.plugin.clone(),
                panel: p.id.clone(),
                items: panel_items(p),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The Manager's own card: id is `open:<plugin>`, the action id
    // `<verb>:<plugin>`.
    fn card(id: &str, action: Option<&str>, state: &str) -> CardItem {
        CardItem {
            id: format!("open:{id}"),
            title: id.to_uppercase(),
            meta: "0.1.0 · rekindle".into(),
            text: "Does a thing.".into(),
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
    }

    #[test]
    fn a_list_is_a_page_whose_rows_open_and_whose_switches_flip() {
        assert!(!is_grid(&list()));
        let (title, back, items) = page_items(&list());
        assert_eq!((title.as_str(), back.as_str()), ("Plugins", "back"));
        assert_eq!(items[0], Item::Header("3 installed, 1 on".into()));
        // The row opens the plugin's page, and its switch is the card's
        // own action. Its line is the card's line and its state.
        let a = row(&items, "open:dev.a");
        assert_eq!(a.press, Some(("open:dev.a".into(), UiInput::Clicked)));
        assert_eq!(a.subtitle.as_deref(), Some("0.1.0 · rekindle · On"));
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
        let Shown::Page(p) = shown(&menu(), &detail, std::slice::from_ref(&panel)) else {
            panic!("a plugin's page is a page")
        };
        assert_eq!((p.title.as_str(), p.back.as_str()), ("A 0.1.0", "back"));
        assert_eq!(
            row(&p.items, "on:dev.a").press,
            Some(("on:dev.a".into(), UiInput::Toggled(false)))
        );
        assert!(
            p.items
                .iter()
                .all(|i| !matches!(i, Item::Row(r) if r.title == "Back"))
        );
        let [own] = p.panels.as_slice() else {
            panic!("{:?}", p.panels)
        };
        assert_eq!(
            (own.plugin.as_str(), own.panel.as_str()),
            ("dev.a", "settings")
        );
        assert_eq!(own.items[0], Item::Header("Settings".into()));
        assert_eq!(
            row(&own.items, "settings/loud").press,
            Some(("loud".into(), UiInput::Toggled(true)))
        );
        // A select steps to its next option, round to the first.
        assert_eq!(
            row(&own.items, "settings/mode").press,
            Some(("mode".into(), UiInput::Selected(0)))
        );
    }

    #[test]
    fn the_grid_shows_the_menu() {
        let grid = vec![Widget::Tabs {
            id: "tab".into(),
            tabs: vec!["Installed".into()],
            selected: 0,
        }];
        assert_eq!(shown(&menu(), &grid, &[]), Shown::Menu(menu_items(&menu())));
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
    }
}
