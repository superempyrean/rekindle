//! Lowering the semantic tree to native GPUI elements.
//!
//! The output is still a description, not bytes: [`GpuiDiv`] and
//! [`GpuiText`] name the fields a real `Div`/`StyleRefinement`/text element
//! has. The desktop backend resolves those fields' byte offsets from the
//! client build at runtime and writes them, then the client's own layout and
//! paint do the rest. Nothing in this module draws.
//!
//! The vocabulary intentionally composes controls out of `Div`s where GPUI
//! has no single component: a toggle is the client's own track and thumb
//! shapes, a field is its label/value row. Because every value comes from
//! the client's tokens and every box is the client's `Div`, the result is
//! indistinguishable from a hand-written client element.

use crate::theme::{Color, Theme, Token};
use crate::ui::{
    Align, Border, ButtonKind, Component, Justify, Length, Node, Padding, TextStyle, Ui,
    button_colors,
};

/// A GPUI length: auto, pixels, or a fraction of the parent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GpuiLen {
    Auto,
    Px(f32),
    Fraction(f32),
}

impl From<Length> for GpuiLen {
    fn from(value: Length) -> Self {
        match value {
            Length::Auto => Self::Auto,
            Length::Px(v) => Self::Px(v),
            Length::Full => Self::Fraction(1.0),
            Length::Fraction(v) => Self::Fraction(v),
        }
    }
}

/// The flex axis of a `Div`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpuiAxis {
    #[default]
    Row,
    Column,
}

/// `align_items`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpuiAlign {
    #[default]
    Start,
    Center,
    End,
    Stretch,
}

/// `justify_content`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpuiJustify {
    #[default]
    Start,
    Center,
    End,
    Between,
}

/// The fields of a `StyleRefinement` a lowered element writes.
///
/// Every field is named exactly as the client's derived `Debug::fmt` names
/// it, so the runtime offset decoder can find it on any build.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuiStyle {
    pub background: Option<Color>,
    pub text_color: Option<Color>,
    pub border_color: Option<Color>,
    /// Top, right, bottom, left.
    pub border_widths: Option<[f32; 4]>,
    /// Top left, top right, bottom right, bottom left.
    pub corner_radii: Option<[f32; 4]>,
    /// Top, right, bottom, left.
    pub padding: Option<[GpuiLen; 4]>,
    /// Width, height.
    pub gap: Option<[GpuiLen; 2]>,
    pub size: Option<[GpuiLen; 2]>,
    pub min_size: Option<[GpuiLen; 2]>,
    pub max_size: Option<[GpuiLen; 2]>,
    pub opacity: Option<f32>,
    pub flex_grow: Option<f32>,
    pub flex_shrink: Option<f32>,
    pub flex_direction: GpuiAxis,
    pub align_items: Option<GpuiAlign>,
    pub justify_content: Option<GpuiJustify>,
}

/// One `Div`.
#[derive(Clone, Debug, PartialEq)]
pub struct GpuiDiv {
    pub style: GpuiStyle,
    pub children: Vec<GpuiNode>,
    /// The id an interaction reports, if any.
    pub id: Option<String>,
    /// The semantic event a click reports (`press`), if any.
    pub on_press: Option<String>,
    pub focusable: bool,
}

/// One text element.
#[derive(Clone, Debug, PartialEq)]
pub struct GpuiText {
    pub text: String,
    pub font_size: f32,
    pub line_height: f32,
    pub color: Option<Color>,
    /// 400 regular, 500 medium, 600 semibold.
    pub weight: u16,
}

/// An icon by the client's semantic name.
#[derive(Clone, Debug, PartialEq)]
pub struct GpuiIcon {
    pub name: String,
    pub size: f32,
    pub color: Option<Color>,
}

/// A lowered element.
#[expect(
    clippy::large_enum_variant,
    reason = "a tree is mostly Divs, so boxing one variant would box almost every node and add an allocation per element"
)]
#[derive(Clone, Debug, PartialEq)]
pub enum GpuiNode {
    Div(GpuiDiv),
    Text(GpuiText),
    Icon(GpuiIcon),
}

impl GpuiDiv {
    fn new(style: GpuiStyle, children: Vec<GpuiNode>) -> Self {
        Self {
            style,
            children,
            id: None,
            on_press: None,
            focusable: false,
        }
    }
}

/// Lower `ui` with `theme`.
#[must_use]
pub fn lower(ui: &Ui, theme: &Theme) -> GpuiNode {
    lower_node(&ui.root, theme, None, None)
}

/// Lower a bare node, with `parent_text` as the inherited text colour.
#[must_use]
pub fn lower_node(
    node: &Node,
    theme: &Theme,
    parent_text: Option<Color>,
    inherited_id: Option<&str>,
) -> GpuiNode {
    let text_color = node
        .style
        .text_color
        .as_ref()
        .map(|b| theme.color(b, Color::hex(0xdbdee1)))
        .or(parent_text);
    match &node.component {
        Component::Row => stack(node, theme, GpuiAxis::Row, text_color, inherited_id),
        Component::Column => stack(node, theme, GpuiAxis::Column, text_color, inherited_id),
        Component::Surface => surface(node, theme, text_color, inherited_id),
        Component::Text { text, style, color } => {
            let (size, weight) = text_role(*style);
            let color = color
                .as_ref()
                .map(|b| theme.color(b, default_text(theme)))
                .or(text_color)
                .unwrap_or_else(|| theme.color(&node.default_text_color(), default_text(theme)));
            GpuiNode::Text(GpuiText {
                text: text.clone(),
                font_size: size,
                line_height: size * 1.3,
                color: Some(color),
                weight,
            })
        }
        Component::Button {
            label,
            icon,
            variant,
            disabled,
        } => button(
            node,
            theme,
            label,
            icon.as_deref(),
            *variant,
            *disabled,
            inherited_id,
        ),
        Component::Toggle { label, value } => {
            toggle(node, theme, label, *value, text_color, inherited_id)
        }
        Component::Icon { name, color } => GpuiNode::Icon(GpuiIcon {
            name: name.clone(),
            size: 16.0,
            color: color
                .as_ref()
                .map(|b| theme.color(b, default_text(theme)))
                .or(text_color),
        }),
        Component::Field { label, value } => {
            field(node, theme, label, value, text_color, inherited_id)
        }
        Component::Separator => {
            let style = GpuiStyle {
                size: Some([GpuiLen::Fraction(1.0), GpuiLen::Px(1.0)]),
                background: Some(theme.tokens.resolve_or(
                    &Token::new(Token::BORDER_SUBTLE),
                    Color::hex_alpha(0xffffff14),
                )),
                ..GpuiStyle::default()
            };
            GpuiNode::Div(GpuiDiv::new(style, Vec::new()))
        }
    }
}

fn default_text(theme: &Theme) -> Color {
    theme
        .tokens
        .resolve_or(&Token::new(Token::TEXT_DEFAULT), Color::hex(0xdbdee1))
}

fn text_role(style: TextStyle) -> (f32, u16) {
    match style {
        TextStyle::Heading => (20.0, 600),
        TextStyle::Body | TextStyle::Muted => (14.0, 400),
        TextStyle::Small => (12.0, 400),
    }
}

fn padding(lengths: Padding) -> [GpuiLen; 4] {
    [
        lengths.top.into(),
        lengths.right.into(),
        lengths.bottom.into(),
        lengths.left.into(),
    ]
}

fn size(lengths: [Length; 2]) -> [GpuiLen; 2] {
    [lengths[0].into(), lengths[1].into()]
}

fn base_style(node: &Node, theme: &Theme) -> GpuiStyle {
    let border = node.style.border.as_ref();
    GpuiStyle {
        gap: node
            .style
            .gap
            .map(|gap| [GpuiLen::Px(gap), GpuiLen::Px(gap)]),
        padding: node.style.padding.map(padding),
        background: node
            .style
            .background
            .as_ref()
            .map(|b| theme.color(b, Color::hex(0))),
        text_color: node
            .style
            .text_color
            .as_ref()
            .map(|b| theme.color(b, default_text(theme))),
        border_color: border.map(|b| theme.color(&b.color, Color::hex(0))),
        border_widths: border.map(|b| [b.width; 4]),
        corner_radii: border.map(|b| [b.radius; 4]),
        opacity: node.style.opacity,
        size: node.style.size.map(size),
        min_size: node.style.min_size.map(size),
        max_size: node.style.max_size.map(size),
        flex_grow: node.style.flex_grow,
        flex_shrink: node.style.flex_shrink,
        ..GpuiStyle::default()
    }
}

fn stack(
    node: &Node,
    theme: &Theme,
    axis: GpuiAxis,
    text_color: Option<Color>,
    inherited_id: Option<&str>,
) -> GpuiNode {
    let mut style = base_style(node, theme);
    style.flex_direction = axis;
    style.align_items = node.style.align_items.map(|a| match a {
        Align::Start => GpuiAlign::Start,
        Align::Center => GpuiAlign::Center,
        Align::End => GpuiAlign::End,
        Align::Stretch => GpuiAlign::Stretch,
    });
    style.justify_content = node.style.justify_content.map(|j| match j {
        Justify::Start => GpuiJustify::Start,
        Justify::Center => GpuiJustify::Center,
        Justify::End => GpuiJustify::End,
        Justify::Between => GpuiJustify::Between,
    });
    let children = node
        .children
        .iter()
        .map(|child| {
            lower_node(
                child,
                theme,
                text_color,
                child.id.as_deref().or(inherited_id),
            )
        })
        .collect();
    let mut div = GpuiDiv::new(style, children);
    div.id.clone_from(&node.id);
    GpuiNode::Div(div)
}

fn surface(
    node: &Node,
    theme: &Theme,
    text_color: Option<Color>,
    inherited_id: Option<&str>,
) -> GpuiNode {
    let mut style = base_style(node, theme);
    style.flex_direction = GpuiAxis::Column;
    if style.background.is_none() {
        style.background = Some(theme.tokens.resolve_or(
            &Token::new(Token::BACKGROUND_SURFACE_HIGH),
            Color::hex(0x232328),
        ));
    }
    if style.corner_radii.is_none() {
        style.corner_radii = Some([8.0; 4]);
    }
    if style.padding.is_none() {
        style.padding = Some([GpuiLen::Px(12.0); 4]);
    }
    if style.align_items.is_none() {
        style.align_items = Some(GpuiAlign::Stretch);
    }
    let children = node
        .children
        .iter()
        .map(|child| {
            lower_node(
                child,
                theme,
                text_color,
                child.id.as_deref().or(inherited_id),
            )
        })
        .collect();
    let mut div = GpuiDiv::new(style, children);
    div.id.clone_from(&node.id);
    GpuiNode::Div(div)
}

fn button(
    node: &Node,
    theme: &Theme,
    label: &str,
    icon: Option<&str>,
    kind: ButtonKind,
    disabled: bool,
    inherited_id: Option<&str>,
) -> GpuiNode {
    let (background, foreground) = button_colors(kind, theme);
    let mut style = base_style(node, theme);
    style.flex_direction = GpuiAxis::Row;
    style.background = Some(style.background.unwrap_or(background));
    style.corner_radii = style.corner_radii.or(Some([6.0; 4]));
    style.padding = style.padding.or(Some([
        GpuiLen::Px(8.0),
        GpuiLen::Px(14.0),
        GpuiLen::Px(8.0),
        GpuiLen::Px(14.0),
    ]));
    style.gap = style.gap.or(Some([GpuiLen::Px(6.0); 2]));
    style.align_items = style.align_items.or(Some(GpuiAlign::Center));
    style.justify_content = style.justify_content.or(Some(GpuiJustify::Center));
    if disabled {
        style.opacity = Some(style.opacity.unwrap_or(0.5));
    }
    let mut children = Vec::new();
    if let Some(icon) = icon {
        children.push(GpuiNode::Icon(GpuiIcon {
            name: icon.to_string(),
            size: 16.0,
            color: Some(foreground),
        }));
    }
    children.push(GpuiNode::Text(GpuiText {
        text: label.to_string(),
        font_size: 14.0,
        line_height: 18.0,
        color: Some(foreground),
        weight: 500,
    }));
    let mut div = GpuiDiv::new(style, children);
    div.id = node.id.clone().or_else(|| inherited_id.map(str::to_string));
    div.on_press = node.id.clone().or_else(|| inherited_id.map(str::to_string));
    div.focusable = !disabled;
    GpuiNode::Div(div)
}

fn toggle(
    node: &Node,
    theme: &Theme,
    label: &str,
    value: bool,
    text_color: Option<Color>,
    inherited_id: Option<&str>,
) -> GpuiNode {
    let track = if value {
        theme
            .tokens
            .resolve_or(&Token::new(Token::BACKGROUND_BRAND), Color::hex(0x5865f2))
    } else {
        theme.tokens.resolve_or(
            &Token::new(Token::BACKGROUND_MOD_NORMAL),
            Color::hex_alpha(0xffffff29),
        )
    };
    let thumb = theme
        .tokens
        .resolve_or(&Token::new(Token::TEXT_STRONG), Color::hex(0xffffff));
    let track_style = GpuiStyle {
        size: Some([GpuiLen::Px(36.0), GpuiLen::Px(20.0)]),
        background: Some(track),
        corner_radii: Some([10.0; 4]),
        align_items: Some(if value {
            GpuiAlign::End
        } else {
            GpuiAlign::Start
        }),
        padding: Some([GpuiLen::Px(2.0); 4]),
        flex_shrink: Some(0.0),
        ..GpuiStyle::default()
    };
    let thumb_style = GpuiStyle {
        size: Some([GpuiLen::Px(16.0), GpuiLen::Px(16.0)]),
        background: Some(thumb),
        corner_radii: Some([8.0; 4]),
        ..GpuiStyle::default()
    };
    let track = GpuiNode::Div(GpuiDiv::new(
        track_style,
        vec![GpuiNode::Div(GpuiDiv::new(thumb_style, Vec::new()))],
    ));

    let label = GpuiNode::Text(GpuiText {
        text: label.to_string(),
        font_size: 14.0,
        line_height: 18.0,
        color: Some(text_color.unwrap_or_else(|| default_text(theme))),
        weight: 400,
    });
    let mut style = base_style(node, theme);
    style.flex_direction = GpuiAxis::Row;
    style.gap = style.gap.or(Some([GpuiLen::Px(8.0); 2]));
    style.align_items = style.align_items.or(Some(GpuiAlign::Center));
    style.justify_content = style.justify_content.or(Some(GpuiJustify::Between));
    style.size = style.size.or(Some([GpuiLen::Fraction(1.0), GpuiLen::Auto]));
    let mut div = GpuiDiv::new(style, vec![label, track]);
    div.id = node.id.clone().or_else(|| inherited_id.map(str::to_string));
    GpuiNode::Div(div)
}

fn field(
    node: &Node,
    theme: &Theme,
    label: &str,
    value: &str,
    text_color: Option<Color>,
    inherited_id: Option<&str>,
) -> GpuiNode {
    let label_color = theme
        .tokens
        .resolve_or(&Token::new(Token::TEXT_SUBTLE), Color::hex(0xb5bac1));
    let value_color = text_color.unwrap_or_else(|| default_text(theme));
    let label = GpuiNode::Text(GpuiText {
        text: label.to_string(),
        font_size: 14.0,
        line_height: 18.0,
        color: Some(label_color),
        weight: 400,
    });
    let value = GpuiNode::Text(GpuiText {
        text: value.to_string(),
        font_size: 14.0,
        line_height: 18.0,
        color: Some(value_color),
        weight: 500,
    });
    let mut style = base_style(node, theme);
    style.flex_direction = GpuiAxis::Row;
    style.size = style.size.or(Some([GpuiLen::Fraction(1.0), GpuiLen::Auto]));
    style.align_items = style.align_items.or(Some(GpuiAlign::Center));
    style.justify_content = style.justify_content.or(Some(GpuiJustify::Between));
    style.gap = style.gap.or(Some([GpuiLen::Px(8.0); 2]));
    let mut div = GpuiDiv::new(style, vec![label, value]);
    div.id = node.id.clone().or_else(|| inherited_id.map(str::to_string));
    GpuiNode::Div(div)
}

/// A border as a `(width, colour, radius)` triple.
#[must_use]
pub fn border(theme: &Theme, border: &Border, fallback: Color) -> (f32, Color, f32) {
    (
        border.width,
        theme.color(&border.color, fallback),
        border.radius,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Brush, Tokens};
    use crate::ui::{Node, Padding, Ui};

    fn theme() -> Theme {
        Theme::new("test", Tokens::defaults())
    }

    #[test]
    fn card_lowers_to_a_native_column() {
        let ui = Ui::card("Rekindle", "One API.");
        let node = lower(&ui, &theme());
        let GpuiNode::Div(surface) = node else {
            panic!("surface is a Div");
        };
        assert_eq!(surface.style.flex_direction, GpuiAxis::Column);
        assert_eq!(surface.style.padding, Some([GpuiLen::Px(16.0); 4]));
        assert_eq!(surface.style.corner_radii, Some([8.0; 4]));
        assert_eq!(surface.style.background, Some(Color::hex(0x232328)));
        assert_eq!(surface.children.len(), 2);
        let GpuiNode::Text(heading) = &surface.children[0] else {
            panic!("heading is text");
        };
        assert!((heading.font_size - 20.0).abs() < f32::EPSILON);
        assert_eq!(heading.weight, 600);
        assert_eq!(heading.color, Some(Color::hex(0xf2f3f5)));
    }

    #[test]
    fn primary_button_uses_the_brand_token() {
        let node = lower_node(
            &Node::button("save", "Save").primary(),
            &theme(),
            None,
            None,
        );
        let GpuiNode::Div(button) = node else {
            panic!("button is a Div");
        };
        assert_eq!(button.style.background, Some(Color::hex(0x5865f2)));
        assert_eq!(button.style.corner_radii, Some([6.0; 4]));
        assert_eq!(button.on_press.as_deref(), Some("save"));
        assert!(button.focusable);
        let GpuiNode::Text(label) = &button.children[0] else {
            panic!("label is text");
        };
        assert_eq!(label.text, "Save");
        assert_eq!(label.color, Some(Color::hex(0xf2f3f5)));
    }

    #[test]
    fn toggle_is_track_and_thumb() {
        let node = lower_node(&Node::toggle("on", "Enabled", true), &theme(), None, None);
        let GpuiNode::Div(row) = node else {
            panic!("toggle is a Div");
        };
        assert_eq!(row.style.justify_content, Some(GpuiJustify::Between));
        let GpuiNode::Div(track) = &row.children[1] else {
            panic!("track is a Div");
        };
        assert_eq!(
            track.style.size,
            Some([GpuiLen::Px(36.0), GpuiLen::Px(20.0)])
        );
        assert_eq!(track.style.background, Some(Color::hex(0x5865f2)));
        assert_eq!(track.style.align_items, Some(GpuiAlign::End));
        let GpuiNode::Div(thumb) = &track.children[0] else {
            panic!("thumb is a Div");
        };
        assert_eq!(thumb.style.corner_radii, Some([8.0; 4]));
    }

    #[test]
    fn field_is_label_and_value() {
        let node = lower_node(&Node::field("Version", "1.2.3"), &theme(), None, None);
        let GpuiNode::Div(row) = node else {
            panic!("field is a Div");
        };
        assert_eq!(row.style.justify_content, Some(GpuiJustify::Between));
        assert_eq!(row.children.len(), 2);
    }

    #[test]
    fn a_custom_token_reaches_the_style() {
        let mut tokens = Tokens::defaults();
        tokens.set("background-surface-high", Color::hex(0x101820));
        let theme = Theme::new("custom", tokens);
        let ui = Ui::new(
            Node::surface(vec![Node::text("hi")])
                .background(Brush::token(Token::BACKGROUND_SURFACE_HIGH)),
        );
        let GpuiNode::Div(surface) = lower(&ui, &theme) else {
            panic!("surface is a Div");
        };
        assert_eq!(surface.style.background, Some(Color::hex(0x101820)));
    }

    #[test]
    fn sizes_and_gaps_map_by_name() {
        let style = crate::ui::Style::new()
            .gap(4.0)
            .padding(Padding::xy(8.0, 4.0))
            .grow(1.0);
        let node = Node::row(vec![]).style(style);
        let GpuiNode::Div(div) = lower_node(&node, &theme(), None, None) else {
            panic!("row is a Div");
        };
        assert_eq!(div.style.gap, Some([GpuiLen::Px(4.0); 2]));
        assert_eq!(
            div.style.padding,
            Some([
                GpuiLen::Px(4.0),
                GpuiLen::Px(8.0),
                GpuiLen::Px(4.0),
                GpuiLen::Px(8.0)
            ])
        );
        assert_eq!(div.style.flex_grow, Some(1.0));
    }
}
