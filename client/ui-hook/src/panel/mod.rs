//! Plugin panels as real elements: rekindle's settings page, the body of
//! rekindle's window and of a plugin's modal.
//!
//! Each widget reports what the user did as [`Click::Ui`]. The page keeps
//! the value the user has set until the plugin shows it back
//! ([`field`]), so an edit doesn't flicker back while the plugin answers.

mod color;
mod field;
pub mod rows;

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rekindle_native::{Align, Ctx, Cursor, Dir, Edges, Len, Node, Rgba, Style};

use color::color_picker;
pub(crate) use color::{picked, picker_input};
pub(crate) use field::{field_value, focused_field};
use field::{local_value, set_local, text_field};

use crate::{Click, queue_click};

/// A plugin panel rekindle shows on its settings page.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelItem {
    pub plugin: String,
    /// The plugin's panel id (interactions are reported against it).
    pub id: String,
    pub title: String,
    pub widgets: Vec<Widget>,
}

/// One element of a plugin panel, as rekindle's settings page draws it. The
/// page renders these as real elements (`rekindle-native`) and routes interactions back as
/// [`Click::Ui`].
#[derive(Clone, Debug, PartialEq)]
pub enum Widget {
    Heading(String),
    Text(String),
    Separator,
    Toggle {
        id: String,
        label: String,
        value: bool,
    },
    Button {
        id: String,
        label: String,
    },
    TextInput {
        id: String,
        label: String,
        value: String,
        placeholder: String,
    },
    Select {
        id: String,
        label: String,
        options: Vec<String>,
        selected: u32,
    },
    Slider {
        id: String,
        label: String,
        min: f64,
        max: f64,
        step: f64,
        value: f64,
    },
    /// Shown, not editable here (`#rrggbb[aa]`).
    /// A colour picker; `hex` is `#rrggbb` or `#rrggbbaa`.
    Color {
        id: String,
        label: String,
        hex: String,
    },
    /// A row of tabs, one selected. A press reports `Selected(index)`.
    Tabs {
        id: String,
        tabs: Vec<String>,
        selected: u32,
    },
    /// A grid of cards (rekindle's plugin grid).
    Cards(Vec<CardItem>),
    /// The top of a plugin's page, in one row: a back button, the name
    /// with a line under it, a switch (id, label, value) and a button (id,
    /// label).
    Header {
        back: String,
        title: String,
        meta: String,
        toggle: Option<(String, String, bool)>,
        action: Option<(String, String)>,
    },
}

/// The new value of a panel element the user changed (mirrors the runtime's
/// UI values, minus colour, which this page can't edit).
#[derive(Clone, Debug, PartialEq)]
pub enum UiInput {
    Toggled(bool),
    Clicked,
    Text(String),
    Selected(u32),
    Number(f64),
    /// `0xRRGGBBAA`.
    Color(u32),
}

/// One card of [`Widget::Cards`]. A press on the card reports `Clicked` for
/// `id`, and a press on its button `Clicked` for the action's id.
#[derive(Clone, Debug, PartialEq)]
pub struct CardItem {
    pub id: String,
    pub title: String,
    pub meta: String,
    pub text: String,
    pub state: String,
    /// The button's (id, label).
    pub action: Option<(String, String)>,
}

static PANELS: Mutex<Vec<PanelItem>> = Mutex::new(Vec::new());
/// The plugin whose panels the page shows under rekindle's own.
static SHOWN_PLUGIN: Mutex<Option<String>> = Mutex::new(None);

/// How often the client rendered rekindle's settings page: the drive checks
/// the client calls it.
pub(crate) static SETTINGS_RENDERS: AtomicUsize = AtomicUsize::new(0);

/// Show only this plugin's panels under rekindle's own (the plugin whose
/// page is open in the grid), or none of the plugins' panels.
pub fn set_shown_plugin(plugin: Option<String>) {
    *SHOWN_PLUGIN.lock().unwrap() = plugin;
}

/// The plugin whose panels are shown under rekindle's own, if any.
pub fn shown_plugin() -> Option<String> {
    SHOWN_PLUGIN.lock().unwrap().clone()
}

/// Set the panels rekindle's settings page renders.
pub fn set_panels(panels: Vec<PanelItem>) {
    *PANELS.lock().unwrap() = panels;
}

/// The panels the settings page currently renders.
pub fn panels() -> Vec<PanelItem> {
    PANELS.lock().unwrap().clone()
}

/// Report a panel interaction back to rekindle, exactly as the settings page's
/// widgets do when the user clicks or types (drained by
/// [`drain_clicks`](crate::drain_clicks)).
pub fn report_ui(plugin: &str, panel: &str, element: &str, value: UiInput) {
    queue_click(Click::Ui {
        plugin: plugin.into(),
        panel: panel.into(),
        element: element.into(),
        value,
    });
}

/// The render body of rekindle's settings page: rekindle's own panel
/// (the plugin grid), and under it the panels of the plugin whose page
/// is open, with interactive widgets (see [`panel_view`]). As
/// `render_body` words: the client owns them once handed over.
pub(crate) fn settings_body() -> [usize; 2] {
    rekindle_native::render_body(|ctx| {
        SETTINGS_RENDERS.fetch_add(1, Ordering::SeqCst);
        settings_content(ctx)
    })
}

/// What rekindle's settings page shows (see [`settings_body`]), also
/// the body of rekindle's window (`crate::window`).
pub(crate) fn settings_content(ctx: &mut Ctx) -> Node {
    // A plugin id has a dot, and rekindle's owner name has none.
    let of_plugin = |p: &PanelItem| p.plugin.contains('.');
    let shown = SHOWN_PLUGIN.lock().unwrap().clone();
    let mut panels = PANELS.lock().unwrap().clone();
    if panels.iter().any(|p| !of_plugin(p)) {
        panels.retain(|p| !of_plugin(p) || shown.as_deref() == Some(p.plugin.as_str()));
    }
    let mut col = Vec::new();
    if panels.is_empty() {
        col.push(Node::text("No plugin has a settings panel."));
    }
    // A plugin's one panel is its settings. Several keep their titles.
    let of_plugins = panels.iter().filter(|p| of_plugin(p)).count();
    for p in &panels {
        let title = match (of_plugin(p), of_plugins) {
            (false, _) => None,
            (true, 1) => Some("Settings".to_string()),
            (true, _) => Some(p.title.clone()),
        };
        col.push(panel_view(p, title, ctx));
    }
    column(12.0, col)
}

/// A panel's widgets, as a modal's body shows them.
pub(crate) fn panel_body(p: &PanelItem, ctx: &mut Ctx) -> Node {
    column(8.0, widget_nodes(p, ctx))
}

/// A flex column with `gap` between its children.
pub(crate) fn column(gap: f32, children: Vec<Node>) -> Node {
    Node::div(
        Style {
            flex: Some(Dir::Column),
            gap: Some(gap),
            ..Default::default()
        },
        children,
    )
}

const FIELD_BG: u32 = 0xffffff14;
const FIELD_BORDER: u32 = 0xffffff33;
/// rekindle's own controls: the client's blurple, a few steps off.
/// Theming recolours every fill that matches a token's value, and
/// `background-brand` is exactly blurple, so a plugin that changes the
/// brand colour would recolour rekindle's buttons too (a dark pick made
/// them unreadable). No token has this value.
const BUTTON_BG: u32 = 0x5865f6ff;
const MUTED: u32 = 0xffffff80;

fn text(t: impl Into<String>) -> Node {
    Node::text(t)
}

fn muted(t: impl Into<String>) -> Node {
    Node::div(
        Style {
            text_color: Some(Rgba::hex(MUTED)),
            ..Default::default()
        },
        vec![text(t)],
    )
}

fn row(gap: f32, children: Vec<Node>) -> Node {
    Node::div(
        Style {
            flex: Some(Dir::Row),
            align_items: Some(Align::Center),
            gap: Some(gap),
            ..Default::default()
        },
        children,
    )
}

fn button_box(label: &str) -> Node {
    Node::div(
        Style {
            padding: Edges::xy(Some(12.0), Some(4.0)),
            radius: Some(6.0),
            background: Some(Rgba::hex(BUTTON_BG)),
            text_color: Some(Rgba::hex(0xffffffff)),
            cursor: Some(Cursor::Pointer),
            ..Default::default()
        },
        vec![text(label)],
    )
}

/// One panel: a card of its widgets, under `title` if it has one.
fn panel_view(p: &PanelItem, title: Option<String>, ctx: &mut Ctx) -> Node {
    let mut card: Vec<Node> = title
        .map(|t| {
            Node::div(
                Style {
                    font_weight: Some(600.0),
                    text_color: Some(Rgba::hex(MUTED)),
                    text_size: Some(12.0),
                    ..Default::default()
                },
                vec![text(t)],
            )
        })
        .into_iter()
        .collect();
    card.extend(widget_nodes(p, ctx));
    Node::div(
        Style {
            flex: Some(Dir::Column),
            gap: Some(8.0),
            padding: Edges::all(Some(12.0)),
            radius: Some(8.0),
            border: Some((1.0, Rgba::hex(FIELD_BORDER))),
            ..Default::default()
        },
        card,
    )
}

/// A panel's widgets as real elements, each reporting to the plugin
/// against the panel's id.
fn widget_nodes(p: &PanelItem, ctx: &mut Ctx) -> Vec<Node> {
    let mut card = Vec::new();
    for w in &p.widgets {
        let (plugin, panel) = (p.plugin.clone(), p.id.clone());
        let el = match w {
            Widget::Heading(t) => Node::div(
                Style {
                    padding: Edges {
                        top: Some(4.0),
                        ..Default::default()
                    },
                    font_weight: Some(600.0),
                    ..Default::default()
                },
                vec![text(t.clone())],
            ),
            Widget::Text(t) => muted(t.clone()),
            Widget::Separator => Node::div(
                Style {
                    height: Some(Len::Px(1.0)),
                    background: Some(Rgba::hex(FIELD_BORDER)),
                    ..Default::default()
                },
                vec![],
            ),
            Widget::Button { id, label } => {
                let id = id.clone();
                row(
                    0.0,
                    vec![button_box(label).on_click_cx(move |ctx| {
                        report_ui(&plugin, &panel, &id, UiInput::Clicked);
                        ctx.refresh();
                    })],
                )
            }
            Widget::Toggle { id, label, value } => toggle_box(&plugin, &panel, id, label, *value),
            Widget::Header {
                back,
                title,
                meta,
                toggle,
                action,
            } => header_row(
                &plugin,
                &panel,
                back,
                title,
                meta,
                toggle.as_ref(),
                action.as_ref(),
            ),
            Widget::Select {
                id,
                label,
                options,
                selected,
            } => select_row(&plugin, &panel, id, label, options, *selected),
            Widget::Slider {
                id,
                label,
                min,
                max,
                step,
                value,
            } => slider_row(&plugin, &panel, id, label, (*min, *max, *step), *value),
            Widget::Color { id, label, hex } => color_picker(&plugin, &panel, id, label, hex, ctx),
            Widget::Tabs { id, tabs, selected } => tabs_row(&plugin, &panel, id, tabs, *selected),
            Widget::Cards(cards) => card_grid(&plugin, &panel, cards),
            Widget::TextInput {
                id,
                label,
                value,
                placeholder,
            } => text_field(&plugin, &panel, id, label, value, placeholder, ctx),
        };
        card.push(el);
    }
    card
}

/// The top of a plugin's page: a back button, the name with a line under
/// it, and an optional switch and button.
fn header_row(
    plugin: &str,
    panel: &str,
    back: &str,
    title: &str,
    meta: &str,
    toggle: Option<&(String, String, bool)>,
    action: Option<&(String, String)>,
) -> Node {
    let press = |id: &str, label: &str| {
        let (p, n, e) = (plugin.to_string(), panel.to_string(), id.to_string());
        button_box(label).on_click_cx(move |ctx| {
            report_ui(&p, &n, &e, UiInput::Clicked);
            ctx.refresh();
        })
    };
    let name = column(
        0.0,
        vec![
            Node::div(
                Style {
                    font_weight: Some(700.0),
                    ..Default::default()
                },
                vec![text(title)],
            ),
            Node::div(
                Style {
                    text_color: Some(Rgba::hex(MUTED)),
                    text_size: Some(12.0),
                    line_height: Some(16.0),
                    ..Default::default()
                },
                vec![text(meta)],
            ),
        ],
    );
    let mut parts = vec![press(back, "Back"), name];
    if let Some((id, label, value)) = toggle {
        parts.push(toggle_box(plugin, panel, id, label, *value));
    }
    if let Some((id, label)) = action {
        parts.push(press(id, label));
    }
    row(14.0, parts)
}

/// A select, shown as its current option. A press moves to the next one
/// and reports `Selected`.
fn select_row(
    plugin: &str,
    panel: &str,
    id: &str,
    label: &str,
    options: &[String],
    selected: u32,
) -> Node {
    let key = format!("{plugin}/{panel}/{id}");
    let sel: u32 = local_value(&key, &selected.to_string())
        .parse()
        .unwrap_or(selected);
    let n = u32::try_from(options.len()).unwrap_or(u32::MAX).max(1);
    let shown = options.get(sel as usize).cloned().unwrap_or_default();
    let (plugin, panel, id) = (plugin.to_string(), panel.to_string(), id.to_string());
    let picker = Node::div(
        Style {
            padding: Edges::xy(Some(8.0), Some(4.0)),
            radius: Some(6.0),
            background: Some(Rgba::hex(FIELD_BG)),
            cursor: Some(Cursor::Pointer),
            ..Default::default()
        },
        vec![text(format!("{shown} ▸"))],
    )
    .on_click_cx(move |ctx| {
        let next = (sel + 1) % n;
        set_local(&key, &next.to_string());
        report_ui(&plugin, &panel, &id, UiInput::Selected(next));
        ctx.refresh();
    });
    row(8.0, vec![text(label), picker])
}

/// A slider, shown as its value with buttons that step it down and up
/// within (min, max). Each press reports `Number`.
fn slider_row(
    plugin: &str,
    panel: &str,
    id: &str,
    label: &str,
    (min, max, step): (f64, f64, f64),
    value: f64,
) -> Node {
    let key = format!("{plugin}/{panel}/{id}");
    let v: f64 = local_value(&key, &value.to_string())
        .parse()
        .unwrap_or(value);
    let step = if step > 0.0 { step } else { (max - min) / 20.0 };
    let nudge = |label: &str, delta: f64| {
        let (plugin, panel, id, key) = (
            plugin.to_string(),
            panel.to_string(),
            id.to_string(),
            key.clone(),
        );
        button_box(label).on_click_cx(move |ctx| {
            // Not `clamp`, which panics on a range a plugin got backwards.
            let nv = (v + delta).max(min).min(max);
            set_local(&key, &nv.to_string());
            report_ui(&plugin, &panel, &id, UiInput::Number(nv));
            ctx.refresh();
        })
    };
    row(
        8.0,
        vec![
            text(format!("{label}: {}", (v * 100.0).round() / 100.0)),
            nudge("−", -step),
            nudge("+", step),
        ],
    )
}

/// A checkbox and its label. A press flips it and reports `Toggled`.
fn toggle_box(plugin: &str, panel: &str, id: &str, label: &str, value: bool) -> Node {
    let key = format!("{plugin}/{panel}/{id}");
    let on = local_value(&key, if value { "1" } else { "0" }) == "1";
    let (plugin, panel, id) = (plugin.to_string(), panel.to_string(), id.to_string());
    let check = Node::div(
        Style {
            width: Some(Len::Px(16.0)),
            height: Some(Len::Px(16.0)),
            radius: Some(2.0),
            border: Some((1.0, Rgba::hex(FIELD_BORDER))),
            background: Some(Rgba::hex(if on { BUTTON_BG } else { FIELD_BG })),
            ..Default::default()
        },
        vec![],
    );
    Node::div(
        Style {
            flex: Some(Dir::Row),
            align_items: Some(Align::Center),
            gap: Some(8.0),
            cursor: Some(Cursor::Pointer),
            ..Default::default()
        },
        vec![check, text(label.to_string())],
    )
    .on_click_cx(move |ctx| {
        set_local(&key, if on { "0" } else { "1" });
        report_ui(&plugin, &panel, &id, UiInput::Toggled(!on));
        ctx.refresh();
    })
}

// ---- the plugin grid ----

/// An unselected tab's fill, and a card's (the drive finds them by these).
pub(crate) const TAB_BG: u32 = 0xffffff0d;
pub(crate) const CARD_BG: u32 = 0xffffff0a;
pub(crate) const CARD_HOVER: u32 = 0xffffff12;
/// Cards per row.
const COLUMNS: usize = 3;

fn tabs_row(plugin: &str, panel: &str, id: &str, tabs: &[String], selected: u32) -> Node {
    let pills = tabs
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let on = i as u32 == selected;
            let (p, n, e) = (plugin.to_string(), panel.to_string(), id.to_string());
            Node::div(
                Style {
                    padding: Edges::xy(Some(12.0), Some(4.0)),
                    radius: Some(6.0),
                    background: Some(Rgba::hex(if on { BUTTON_BG } else { TAB_BG })),
                    text_color: Some(Rgba::hex(if on { 0xffffffff } else { 0xffffffb3 })),
                    cursor: Some(Cursor::Pointer),
                    ..Default::default()
                },
                vec![text(name.clone())],
            )
            .on_click_cx(move |ctx| {
                report_ui(&p, &n, &e, UiInput::Selected(i as u32));
                ctx.refresh();
            })
        })
        .collect();
    row(6.0, pills)
}

/// Cards in rows of [`COLUMNS`]. A press on a card opens it, and a press
/// on its button does only what the button says.
fn card_grid(plugin: &str, panel: &str, cards: &[CardItem]) -> Node {
    let small = |t: &str, colour: u32| {
        Node::div(
            Style {
                text_color: Some(Rgba::hex(colour)),
                text_size: Some(12.0),
                line_height: Some(16.0),
                ..Default::default()
            },
            vec![text(t.to_string())],
        )
    };
    let one = |c: &CardItem| {
        let mut body = vec![
            Node::div(
                Style {
                    font_weight: Some(600.0),
                    ..Default::default()
                },
                vec![text(c.title.clone())],
            ),
            small(&c.meta, MUTED),
        ];
        if !c.text.is_empty() {
            body.push(small(&c.text, 0xffffffb3));
        }
        let mut foot = Vec::new();
        if !c.state.is_empty() {
            foot.push(small(&c.state, MUTED));
        }
        if let Some((id, label)) = &c.action {
            let (p, n, e) = (plugin.to_string(), panel.to_string(), id.clone());
            foot.push(button_box(label).on_click_cx(move |ctx| {
                report_ui(&p, &n, &e, UiInput::Clicked);
                // The card under the button doesn't open.
                ctx.stop_propagation();
                ctx.refresh();
            }));
        }
        if !foot.is_empty() {
            body.push(Node::div(
                Style {
                    flex: Some(Dir::Row),
                    align_items: Some(Align::Center),
                    gap: Some(8.0),
                    padding: Edges {
                        top: Some(4.0),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                foot,
            ));
        }
        let (p, n, e) = (plugin.to_string(), panel.to_string(), c.id.clone());
        Node::div(
            Style {
                // Three to a row, less the gaps between them.
                width: Some(Len::Frac(0.32)),
                flex: Some(Dir::Column),
                gap: Some(4.0),
                padding: Edges::all(Some(10.0)),
                radius: Some(8.0),
                background: Some(Rgba::hex(CARD_BG)),
                hover_background: Some(Rgba::hex(CARD_HOVER)),
                border: Some((1.0, Rgba::hex(FIELD_BORDER))),
                cursor: Some(Cursor::Pointer),
                ..Default::default()
            },
            body,
        )
        .on_click_cx(move |ctx| {
            report_ui(&p, &n, &e, UiInput::Clicked);
            ctx.refresh();
        })
    };
    column(
        8.0,
        cards
            .chunks(COLUMNS)
            .map(|chunk| {
                Node::div(
                    Style {
                        flex: Some(Dir::Row),
                        gap: Some(8.0),
                        ..Default::default()
                    },
                    chunk.iter().map(one).collect(),
                )
            })
            .collect(),
    )
}
