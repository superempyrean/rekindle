//! What plugins put on screen: panel elements, rich-text spans, panels,
//! and the user's interactions with them.

use serde::{Deserialize, Serialize};

use super::{Color, Icon};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Element {
    Heading {
        text: String,
    },
    Text {
        text: String,
    },
    /// Styled inline runs with host-handled click targets.
    RichText {
        spans: Vec<Span>,
    },
    Separator,
    Toggle {
        id: String,
        label: String,
        value: bool,
    },
    Slider {
        id: String,
        label: String,
        min: f64,
        max: f64,
        step: f64,
        value: f64,
    },
    ColorPicker {
        id: String,
        label: String,
        value: Color,
    },
    TextInput {
        id: String,
        label: String,
        value: String,
        placeholder: String,
        #[serde(default)]
        multiline: bool,
    },
    Button {
        id: String,
        label: String,
        #[serde(default)]
        icon: Option<Icon>,
        #[serde(default)]
        danger: bool,
    },
    Select {
        id: String,
        label: String,
        options: Vec<String>,
        selected: u32,
    },
    /// Rows of cells, one per column.
    Table {
        id: String,
        label: String,
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    StringList {
        id: String,
        label: String,
        items: Vec<String>,
        placeholder: String,
    },
    /// A plugin image (registered with `ui.register-image`) at a size.
    Image {
        image: String,
        width: u32,
        height: u32,
    },
    /// A user drawn the client's way (avatar + name).
    User {
        id: u64,
    },
    /// A key/value row.
    Field {
        key: String,
        value: String,
    },
    /// A row of tabs with one selected. A press reports `Selected(index)`.
    /// Host-only: it is for rekindle's own panel, and the plugin contract
    /// has no such element.
    Tabs {
        id: String,
        tabs: Vec<String>,
        selected: u32,
    },
    /// A grid of cards. Host-only, as [`Element::Tabs`].
    Cards {
        cards: Vec<Card>,
    },
    /// The top of a plugin's page, in one row: a back button (it reports
    /// `Clicked` for `back`), the plugin's name with a line about it, its
    /// switch and one button. Host-only, as [`Element::Tabs`].
    Header {
        back: String,
        title: String,
        meta: String,
        toggle: Option<Switch>,
        action: Option<CardAction>,
    },
}

/// The switch of an [`Element::Header`]: it reports `Toggled`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Switch {
    pub id: String,
    pub label: String,
    pub value: bool,
}

/// One card of [`Element::Cards`]. A press on the card reports `Clicked`
/// for `id`, and a press on its button `Clicked` for the action's id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub title: String,
    /// A short line under the title (version, who it is by).
    pub meta: String,
    pub text: String,
    /// Its state in a word or two ("On", "Installed"), or empty.
    pub state: String,
    pub action: Option<CardAction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardAction {
    pub id: String,
    pub label: String,
}

impl Element {
    /// The interactive element id, if any.
    pub fn id(&self) -> Option<&str> {
        match self {
            Element::Toggle { id, .. }
            | Element::Slider { id, .. }
            | Element::ColorPicker { id, .. }
            | Element::TextInput { id, .. }
            | Element::Button { id, .. }
            | Element::Select { id, .. }
            | Element::Table { id, .. }
            | Element::Tabs { id, .. }
            | Element::StringList { id, .. } => Some(id),
            _ => None,
        }
    }
}

/// A styled text run (`types.span`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub text: String,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub code: bool,
    #[serde(default)]
    pub color: Option<SpanColor>,
    /// What clicking it does (host-handled).
    #[serde(default)]
    pub target: Option<crate::api::Target>,
}

impl Span {
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            bold: false,
            italic: false,
            code: false,
            color: None,
            target: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum SpanColor {
    /// A semantic theme token.
    Token(String),
    Rgba(Color),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    pub id: String,
    pub title: String,
    pub elements: Vec<Element>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum UiValue {
    Toggled(bool),
    Number(f64),
    Color(Color),
    Text(String),
    Selected(u32),
    Clicked,
    Rows(Vec<Vec<String>>),
    Items(Vec<String>),
}

/// The `on-ui` surface of the generated settings page's extra elements.
pub const SURFACE_SETTINGS_PAGE: &str = "settings-page";
/// The `on-ui` surface of a plugin's notice banner (element `button`).
pub const SURFACE_NOTICE: &str = "notice";

/// An interaction with an element on a plugin surface: a panel or modal (its
/// id, which wins), [`SURFACE_SETTINGS_PAGE`], [`SURFACE_NOTICE`], or
/// `message:<id>` (an accessory).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiEvent {
    pub surface: String,
    pub element: String,
    pub value: UiValue,
}
