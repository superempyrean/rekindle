//! The semantic vocabulary a plugin writes once.
//!
//! A tree is made of nine components: row, column, text, button, toggle,
//! icon, field, surface and separator. Each backend maps them to its own
//! native controls; the plugin never names a wire prop or a GPUI type.
//!
//! ```ignore
//! let card = Node::surface(vec![
//!     Node::heading("Rekindle"),
//!     Node::text("Rendered by the client's own widgets."),
//!     Node::row(vec![
//!         Node::toggle("enabled", "Enable", true),
//!         Node::button("apply", "Apply").primary(),
//!     ])
//!     .gap(8.0),
//! ])
//! .padding(Padding::all(16.0))
//! .background(Brush::token(Token::BACKGROUND_MOD_SUBTLE));
//! ```

use serde::{Deserialize, Serialize};

use crate::theme::{Brush, Color, Token};

/// The axis a stack lays its children along.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Axis {
    #[default]
    Row,
    Column,
}

/// Cross-axis alignment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
    Stretch,
}

/// Main-axis distribution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    Between,
}

/// A length in the client's logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum Length {
    #[default]
    Auto,
    Px(f32),
    /// Fill the parent.
    Full,
    /// A fraction of the parent, 0..=1.
    Fraction(f32),
}

impl Length {
    #[must_use]
    pub const fn px(value: f32) -> Self {
        Self::Px(value)
    }
}

/// Four edge lengths, clockwise from the top.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Padding {
    pub top: Length,
    pub right: Length,
    pub bottom: Length,
    pub left: Length,
}

impl Padding {
    #[must_use]
    pub fn all(value: f32) -> Self {
        Self::xy(value, value)
    }

    #[must_use]
    pub fn xy(x: f32, y: f32) -> Self {
        Self {
            top: Length::Px(y),
            right: Length::Px(x),
            bottom: Length::Px(y),
            left: Length::Px(x),
        }
    }

    #[must_use]
    pub fn new(top: f32, right: f32, bottom: f32, left: f32) -> Self {
        Self {
            top: Length::Px(top),
            right: Length::Px(right),
            bottom: Length::Px(bottom),
            left: Length::Px(left),
        }
    }
}

/// How prominent a button is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ButtonKind {
    #[default]
    Secondary,
    Primary,
    Danger,
}

/// A text role. The client decides the actual size and weight.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TextStyle {
    /// A section title.
    Heading,
    #[default]
    Body,
    /// A caption under a value.
    Small,
    /// A secondary label.
    Muted,
}

/// A border: width in logical pixels, a colour, and a corner radius.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Border {
    pub width: f32,
    pub color: Brush,
    pub radius: f32,
}

impl Border {
    #[must_use]
    pub fn new(width: f32, color: Brush, radius: f32) -> Self {
        Self {
            width,
            color,
            radius,
        }
    }
}

/// What a node is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Component {
    /// Children laid out left to right.
    Row,
    /// Children laid out top to bottom.
    Column,
    Text {
        text: String,
        style: TextStyle,
        color: Option<Brush>,
    },
    Button {
        label: String,
        #[serde(default)]
        icon: Option<String>,
        variant: ButtonKind,
        #[serde(default)]
        disabled: bool,
    },
    Toggle {
        label: String,
        value: bool,
    },
    Icon {
        name: String,
        color: Option<Brush>,
    },
    /// A key/value line, the client's own field row.
    Field {
        label: String,
        value: String,
    },
    /// A raised area: the client's own panel/card surface.
    Surface,
    Separator,
}

/// A node: a component, the style it asks for, its event id and children.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub component: Component,
    #[serde(default)]
    pub style: Style,
    /// The id an interaction reports. Buttons and toggles require one; other
    /// nodes may carry one for hit testing.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub children: Vec<Node>,
}

/// The style a node asks for, in platform-neutral terms.
///
/// Every field maps by name to a `StyleRefinement` field on the GPUI backend
/// and to wire cells on the mobile backend. A field a backend does not have
/// is dropped there, never approximated by drawing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Style {
    #[serde(default)]
    pub gap: Option<f32>,
    #[serde(default)]
    pub padding: Option<Padding>,
    #[serde(default)]
    pub background: Option<Brush>,
    /// The colour text children use when they name none.
    #[serde(default)]
    pub text_color: Option<Brush>,
    #[serde(default)]
    pub border: Option<Border>,
    /// 0..=1.
    #[serde(default)]
    pub opacity: Option<f32>,
    #[serde(default)]
    pub size: Option<[Length; 2]>,
    #[serde(default)]
    pub min_size: Option<[Length; 2]>,
    #[serde(default)]
    pub max_size: Option<[Length; 2]>,
    #[serde(default)]
    pub flex_grow: Option<f32>,
    #[serde(default)]
    pub flex_shrink: Option<f32>,
    #[serde(default)]
    pub align_items: Option<Align>,
    #[serde(default)]
    pub justify_content: Option<Justify>,
}

impl Style {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = Some(gap);
        self
    }

    #[must_use]
    pub fn padding(mut self, padding: Padding) -> Self {
        self.padding = Some(padding);
        self
    }

    #[must_use]
    pub fn background(mut self, brush: impl Into<Brush>) -> Self {
        self.background = Some(brush.into());
        self
    }

    #[must_use]
    pub fn text_color(mut self, brush: impl Into<Brush>) -> Self {
        self.text_color = Some(brush.into());
        self
    }

    #[must_use]
    pub fn border(mut self, border: Border) -> Self {
        self.border = Some(border);
        self
    }

    #[must_use]
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = Some(opacity);
        self
    }

    #[must_use]
    pub fn grow(mut self, grow: f32) -> Self {
        self.flex_grow = Some(grow);
        self
    }

    #[must_use]
    pub fn align(mut self, align: Align) -> Self {
        self.align_items = Some(align);
        self
    }

    #[must_use]
    pub fn justify(mut self, justify: Justify) -> Self {
        self.justify_content = Some(justify);
        self
    }
}

impl Node {
    fn new(component: Component) -> Self {
        Self {
            component,
            style: Style::default(),
            id: None,
            children: Vec::new(),
        }
    }

    #[must_use]
    pub fn row(children: Vec<Node>) -> Self {
        Self {
            children,
            ..Self::new(Component::Row)
        }
    }

    #[must_use]
    pub fn column(children: Vec<Node>) -> Self {
        Self {
            children,
            ..Self::new(Component::Column)
        }
    }

    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::new(Component::Text {
            text: text.into(),
            style: TextStyle::Body,
            color: None,
        })
    }

    #[must_use]
    pub fn heading(text: impl Into<String>) -> Self {
        Self::new(Component::Text {
            text: text.into(),
            style: TextStyle::Heading,
            color: None,
        })
    }

    #[must_use]
    pub fn muted(text: impl Into<String>) -> Self {
        Self::new(Component::Text {
            text: text.into(),
            style: TextStyle::Muted,
            color: None,
        })
    }

    #[must_use]
    pub fn small(text: impl Into<String>) -> Self {
        Self::new(Component::Text {
            text: text.into(),
            style: TextStyle::Small,
            color: None,
        })
    }

    /// A button. `id` is what a press reports to the plugin.
    #[must_use]
    pub fn button(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: Some(id.into()),
            ..Self::new(Component::Button {
                label: label.into(),
                icon: None,
                variant: ButtonKind::Secondary,
                disabled: false,
            })
        }
    }

    /// A toggle. `id` is what a change reports.
    #[must_use]
    pub fn toggle(id: impl Into<String>, label: impl Into<String>, value: bool) -> Self {
        Self {
            id: Some(id.into()),
            ..Self::new(Component::Toggle {
                label: label.into(),
                value,
            })
        }
    }

    #[must_use]
    pub fn icon(name: impl Into<String>) -> Self {
        Self::new(Component::Icon {
            name: name.into(),
            color: None,
        })
    }

    #[must_use]
    pub fn field(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self::new(Component::Field {
            label: label.into(),
            value: value.into(),
        })
    }

    #[must_use]
    pub fn surface(children: Vec<Node>) -> Self {
        Self {
            children,
            ..Self::new(Component::Surface)
        }
    }

    #[must_use]
    pub fn separator() -> Self {
        Self::new(Component::Separator)
    }

    #[must_use]
    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    #[must_use]
    pub fn gap(mut self, gap: f32) -> Self {
        self.style.gap = Some(gap);
        self
    }

    #[must_use]
    pub fn padding(mut self, padding: Padding) -> Self {
        self.style.padding = Some(padding);
        self
    }

    #[must_use]
    pub fn background(mut self, brush: impl Into<Brush>) -> Self {
        self.style.background = Some(brush.into());
        self
    }

    #[must_use]
    pub fn text_color(mut self, brush: impl Into<Brush>) -> Self {
        self.style.text_color = Some(brush.into());
        self
    }

    #[must_use]
    pub fn border(mut self, border: Border) -> Self {
        self.style.border = Some(border);
        self
    }

    #[must_use]
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.style.opacity = Some(opacity);
        self
    }

    #[must_use]
    pub fn primary(mut self) -> Self {
        if let Component::Button { variant, .. } = &mut self.component {
            *variant = ButtonKind::Primary;
        }
        self
    }

    #[must_use]
    pub fn danger(mut self) -> Self {
        if let Component::Button { variant, .. } = &mut self.component {
            *variant = ButtonKind::Danger;
        }
        self
    }

    #[must_use]
    pub fn disabled(mut self) -> Self {
        if let Component::Button { disabled, .. } = &mut self.component {
            *disabled = true;
        }
        self
    }

    #[must_use]
    pub fn child(mut self, child: Node) -> Self {
        self.children.push(child);
        self
    }

    /// The event id, for a button or toggle.
    #[must_use]
    pub fn event_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// A sensible text colour for this component when the caller named none.
    #[must_use]
    pub fn default_text_color(&self) -> Brush {
        match &self.component {
            Component::Text {
                style: TextStyle::Muted,
                ..
            } => Brush::token(Token::TEXT_MUTED),
            Component::Text {
                style: TextStyle::Small,
                ..
            }
            | Component::Field { .. } => Brush::token(Token::TEXT_SUBTLE),
            Component::Text {
                style: TextStyle::Heading,
                ..
            } => Brush::token(Token::TEXT_STRONG),
            _ => Brush::token(Token::TEXT_DEFAULT),
        }
    }
}

/// A complete plugin UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ui {
    pub root: Node,
}

impl Ui {
    #[must_use]
    pub fn new(root: Node) -> Self {
        Self { root }
    }

    /// A surface card with a heading and a body, the common settings shape.
    #[must_use]
    pub fn card(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self::new(
            Node::surface(vec![
                Node::heading(title),
                Node::text(body).text_color(Token::TEXT_SUBTLE),
            ])
            .gap(6.0)
            .padding(Padding::all(16.0))
            .background(Token::BACKGROUND_SURFACE_HIGH)
            .border(Border::new(1.0, Brush::token(Token::BORDER_SUBTLE), 8.0)),
        )
    }
}

/// A colour pair a backend uses for a component's own chrome.
#[must_use]
pub fn button_colors(kind: ButtonKind, theme: &crate::theme::Theme) -> (Color, Color) {
    match kind {
        ButtonKind::Primary => (
            theme
                .tokens
                .resolve_or(&Token::new(Token::BACKGROUND_BRAND), Color::hex(0x5865f2)),
            theme
                .tokens
                .resolve_or(&Token::new(Token::TEXT_STRONG), Color::hex(0xffffff)),
        ),
        ButtonKind::Danger => (
            theme.tokens.resolve_or(
                &Token::new("background-feedback-critical"),
                Color::hex(0xf23f43),
            ),
            theme
                .tokens
                .resolve_or(&Token::new("text-strong"), Color::hex(0xffffff)),
        ),
        ButtonKind::Secondary => (
            theme.tokens.resolve_or(
                &Token::new(Token::BACKGROUND_MOD_SUBTLE),
                Color::hex_alpha(0xffffff1f),
            ),
            theme
                .tokens
                .resolve_or(&Token::new(Token::TEXT_DEFAULT), Color::hex(0xdbdee1)),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_has_the_expected_shape() {
        let ui = Ui::card("Rekindle", "Plugins, once.");
        let Component::Surface = ui.root.component else {
            panic!("card root is a surface");
        };
        assert_eq!(ui.root.children.len(), 2);
        assert_eq!(ui.root.style.gap, Some(6.0));
        assert!(matches!(
            ui.root.children[0].component,
            Component::Text {
                style: TextStyle::Heading,
                ..
            }
        ));
    }

    #[test]
    fn button_ids_are_event_ids() {
        let b = Node::button("apply", "Apply").primary();
        assert_eq!(b.event_id(), Some("apply"));
        assert!(matches!(
            b.component,
            Component::Button {
                variant: ButtonKind::Primary,
                ..
            }
        ));
    }

    #[test]
    fn serde_round_trip() {
        let ui = Ui::new(
            Node::row(vec![
                Node::icon("settings"),
                Node::toggle("on", "Enabled", true),
                Node::button("save", "Save").primary(),
            ])
            .gap(8.0)
            .padding(Padding::all(12.0)),
        );
        let json = serde_json::to_string(&ui).unwrap();
        let back: Ui = serde_json::from_str(&json).unwrap();
        assert_eq!(ui, back);
    }
}
