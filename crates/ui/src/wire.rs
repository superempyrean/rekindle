//! Lowering the semantic tree to the wire a split GPUI client applies.
//!
//! The output is the same kind of retained tree the Android/iOS guest sends:
//! `Create`/`Children`/`Root` ops whose nodes are `view` or `text` with the
//! platform's own property ids. A button is a `view` with a text child and a
//! `press` listener, a toggle is a `view` with a track and a thumb, a field is
//! a row of two texts. Colours are `0xAARRGGBB`, lengths `f64` logical pixels,
//! padding the wire's four-edge quad (left, top, right, bottom).
//!
//! The names are resolved through [`rekindle_wire::PropId::from_name`], so a
//! backend that does not carry a property simply does not receive it; nothing
//! here invents an id.

use rekindle_wire::turn::{NodeOp, Turn, create_props};
use rekindle_wire::{ElementKind, NodeId, PropId, PropSet, Value};

use crate::theme::{Brush, Color, Theme, Token};
use crate::ui::{Component, Justify, Length, Node, Padding, TextStyle, Ui, button_colors};

/// One node of the lowered wire tree.
#[derive(Clone, Debug, PartialEq)]
pub struct WireNode {
    pub id: NodeId,
    pub kind: ElementKind,
    pub props: PropSet,
    pub children: Vec<WireNode>,
}

/// Lower `ui` with `theme`.
#[must_use]
pub fn lower(ui: &Ui, theme: &Theme) -> WireNode {
    let mut next = 1;
    lower_node(&ui.root, theme, None, &mut next)
}

/// Lower a bare node. IDs are assigned in pre-order from 1.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per semantic component, kept next to the props it writes"
)]
#[must_use]
pub fn lower_node(
    node: &Node,
    theme: &Theme,
    inherited_text: Option<Color>,
    next: &mut NodeId,
) -> WireNode {
    let id = *next;
    *next += 1;
    let text_color = node
        .style
        .text_color
        .as_ref()
        .map(|b| theme.color(b, Color::hex(0xdbdee1)))
        .or(inherited_text);
    match &node.component {
        Component::Row | Component::Column | Component::Surface => {
            let mut props = container_props(node, theme);
            if matches!(node.component, Component::Column | Component::Surface) {
                put(&mut props, "direction", Value::Str("column".into()));
            }
            let color = text_color.or_else(|| Some(default_text(theme)));
            let children = node
                .children
                .iter()
                .map(|child| lower_node(child, theme, color, next))
                .collect();
            WireNode {
                id,
                kind: ElementKind::VIEW,
                props: create_props(props),
                children,
            }
        }
        Component::Text { text, style, color } => {
            let color = color
                .as_ref()
                .map(|b| theme.color(b, default_text(theme)))
                .or(text_color)
                .unwrap_or_else(|| default_text(theme));
            let (size, weight) = text_role(*style);
            let mut props = Vec::new();
            put(&mut props, "text", Value::Str(text.clone()));
            put(&mut props, "color", Value::color(color.argb()));
            put(&mut props, "fontSize", Value::F64(f64::from(size)));
            put(&mut props, "fontWeight", Value::I32(weight));
            put(&mut props, "lineHeight", Value::F64(f64::from(size * 1.3)));
            WireNode {
                id,
                kind: ElementKind::TEXT,
                props: create_props(props),
                children: Vec::new(),
            }
        }
        Component::Button {
            label,
            icon,
            variant,
            disabled,
        } => {
            let (background, foreground) = button_colors(*variant, theme);
            let mut props = Vec::new();
            put(&mut props, "background", Value::color(background.argb()));
            put(&mut props, "radius", Value::F64(6.0));
            put(&mut props, "padding", Value::Quad([14.0, 8.0, 14.0, 8.0]));
            put(&mut props, "gap", Value::F64(6.0));
            put(&mut props, "align", Value::Str("center".into()));
            put(&mut props, "justify", Value::Str("center".into()));
            if *disabled {
                put(&mut props, "opacity", Value::F64(0.5));
                put(&mut props, "accessibilityDisabled", Value::Bool(true));
            }
            if let Some(event) = &node.id {
                put(
                    &mut props,
                    "on",
                    Value::Array(vec![Value::Str("press".into())]),
                );
                put(&mut props, "id", Value::Str(event.clone()));
            }
            if let Some(icon) = icon {
                put(&mut props, "accessibilityLabel", Value::Str(icon.clone()));
            }
            let mut children = Vec::new();
            let _ = next;
            if icon.is_some() {
                children.push(icon_node(
                    next,
                    icon.as_deref().unwrap_or_default(),
                    foreground,
                ));
            }
            children.push(WireNode {
                id: {
                    let id = *next;
                    *next += 1;
                    id
                },
                kind: ElementKind::TEXT,
                props: create_props(vec![
                    (
                        PropId::from_name("text").unwrap(),
                        Value::Str(label.clone()),
                    ),
                    (
                        PropId::from_name("color").unwrap(),
                        Value::color(foreground.argb()),
                    ),
                    (PropId::from_name("fontSize").unwrap(), Value::F64(14.0)),
                    (PropId::from_name("fontWeight").unwrap(), Value::I32(500)),
                ]),
                children: Vec::new(),
            });
            WireNode {
                id,
                kind: ElementKind::VIEW,
                props: create_props(props),
                children,
            }
        }
        Component::Toggle { label, value } => {
            let track = if *value {
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
            let mut props = Vec::new();
            put(&mut props, "direction", Value::Str("row".into()));
            put(&mut props, "align", Value::Str("center".into()));
            put(&mut props, "justify", Value::Str("between".into()));
            put(&mut props, "gap", Value::F64(8.0));
            if let Some(event) = &node.id {
                put(
                    &mut props,
                    "on",
                    Value::Array(vec![Value::Str("change".into())]),
                );
                put(&mut props, "id", Value::Str(event.clone()));
                put(&mut props, "accessibilityLabel", Value::Str(label.clone()));
            }
            let label_id = *next;
            *next += 1;
            let label = WireNode {
                id: label_id,
                kind: ElementKind::TEXT,
                props: create_props(vec![
                    (
                        PropId::from_name("text").unwrap(),
                        Value::Str(label.clone()),
                    ),
                    (
                        PropId::from_name("color").unwrap(),
                        Value::color(text_color.unwrap_or_else(|| default_text(theme)).argb()),
                    ),
                    (PropId::from_name("fontSize").unwrap(), Value::F64(14.0)),
                ]),
                children: Vec::new(),
            };
            let track_id = *next;
            *next += 1;
            let thumb_id = *next;
            *next += 1;
            let thumb = WireNode {
                id: thumb_id,
                kind: ElementKind::VIEW,
                props: create_props(vec![
                    (PropId::from_name("width").unwrap(), Value::F64(16.0)),
                    (PropId::from_name("height").unwrap(), Value::F64(16.0)),
                    (PropId::from_name("radius").unwrap(), Value::F64(8.0)),
                    (
                        PropId::from_name("background").unwrap(),
                        Value::color(thumb.argb()),
                    ),
                ]),
                children: Vec::new(),
            };
            let track = WireNode {
                id: track_id,
                kind: ElementKind::VIEW,
                props: create_props(vec![
                    (PropId::from_name("width").unwrap(), Value::F64(36.0)),
                    (PropId::from_name("height").unwrap(), Value::F64(20.0)),
                    (PropId::from_name("radius").unwrap(), Value::F64(10.0)),
                    (
                        PropId::from_name("background").unwrap(),
                        Value::color(track.argb()),
                    ),
                    (
                        PropId::from_name("align").unwrap(),
                        Value::Str(if *value { "end".into() } else { "start".into() }),
                    ),
                    (PropId::from_name("padding").unwrap(), Value::Quad([2.0; 4])),
                ]),
                children: vec![thumb],
            };
            WireNode {
                id,
                kind: ElementKind::VIEW,
                props: create_props(props),
                children: vec![label, track],
            }
        }
        Component::Icon { name, color } => {
            let color = color
                .as_ref()
                .map(|b| theme.color(b, default_text(theme)))
                .or(text_color)
                .unwrap_or_else(|| default_text(theme));
            icon_node(next, name, color)
        }
        Component::Field { label, value } => {
            let label_color = theme
                .tokens
                .resolve_or(&Token::new(Token::TEXT_SUBTLE), Color::hex(0xb5bac1));
            let value_color = text_color.unwrap_or_else(|| default_text(theme));
            let mut props = Vec::new();
            put(&mut props, "direction", Value::Str("row".into()));
            put(&mut props, "justify", Value::Str("between".into()));
            put(&mut props, "align", Value::Str("center".into()));
            put(&mut props, "gap", Value::F64(8.0));
            let label_id = *next;
            *next += 1;
            let value_id = *next;
            *next += 1;
            WireNode {
                id,
                kind: ElementKind::VIEW,
                props: create_props(props),
                children: vec![
                    text_node(label_id, label, label_color, 14.0, 400),
                    text_node(value_id, value, value_color, 14.0, 500),
                ],
            }
        }
        Component::Separator => {
            let mut props = Vec::new();
            put(&mut props, "width", Value::F64(1.0));
            put(&mut props, "height", Value::F64(1.0));
            put(
                &mut props,
                "background",
                Value::color(
                    theme
                        .tokens
                        .resolve_or(
                            &Token::new(Token::BORDER_SUBTLE),
                            Color::hex_alpha(0xffffff14),
                        )
                        .argb(),
                ),
            );
            WireNode {
                id,
                kind: ElementKind::VIEW,
                props: create_props(props),
                children: Vec::new(),
            }
        }
    }
}

fn icon_node(next: &mut NodeId, name: &str, color: Color) -> WireNode {
    let id = *next;
    *next += 1;
    WireNode {
        id,
        kind: ElementKind::TEXT,
        props: create_props(vec![
            (
                PropId::from_name("text").unwrap(),
                Value::Str(name.to_string()),
            ),
            (
                PropId::from_name("color").unwrap(),
                Value::color(color.argb()),
            ),
            (PropId::from_name("fontSize").unwrap(), Value::F64(16.0)),
            (
                PropId::from_name("accessibilityLabel").unwrap(),
                Value::Str(name.to_string()),
            ),
        ]),
        children: Vec::new(),
    }
}

fn text_node(id: NodeId, text: &str, color: Color, size: f64, weight: i32) -> WireNode {
    WireNode {
        id,
        kind: ElementKind::TEXT,
        props: create_props(vec![
            (
                PropId::from_name("text").unwrap(),
                Value::Str(text.to_string()),
            ),
            (
                PropId::from_name("color").unwrap(),
                Value::color(color.argb()),
            ),
            (PropId::from_name("fontSize").unwrap(), Value::F64(size)),
            (PropId::from_name("fontWeight").unwrap(), Value::I32(weight)),
        ]),
        children: Vec::new(),
    }
}

fn container_props(node: &Node, theme: &Theme) -> PropSet {
    let mut props = Vec::new();
    if let Some(gap) = node.style.gap {
        put(&mut props, "gap", Value::F64(f64::from(gap)));
    }
    if let Some(padding) = node.style.padding {
        put(&mut props, "padding", Value::Quad(quad(padding)));
    }
    if let Some(background) = &node.style.background {
        put(
            &mut props,
            "background",
            Value::color(theme.color(background, Color::hex(0)).argb()),
        );
    }
    if let Some(border) = &node.style.border {
        put(
            &mut props,
            "borderColor",
            Value::color(theme.color(&border.color, Color::hex(0)).argb()),
        );
        put(
            &mut props,
            "borderWidth",
            Value::F64(f64::from(border.width)),
        );
        put(&mut props, "radius", Value::F64(f64::from(border.radius)));
    }
    if let Some(opacity) = node.style.opacity {
        put(&mut props, "opacity", Value::F64(f64::from(opacity)));
    }
    if let Some([w, h]) = node.style.size {
        put_len(&mut props, "width", w);
        put_len(&mut props, "height", h);
    }
    if let Some([w, h]) = node.style.min_size {
        put_len(&mut props, "minWidth", w);
        put_len(&mut props, "minHeight", h);
    }
    if let Some([w, h]) = node.style.max_size {
        put_len(&mut props, "maxWidth", w);
        put_len(&mut props, "maxHeight", h);
    }
    if let Some(align) = node.style.align_items {
        let name = match align {
            crate::ui::Align::Start => "start",
            crate::ui::Align::Center => "center",
            crate::ui::Align::End => "end",
            crate::ui::Align::Stretch => "stretch",
        };
        put(&mut props, "align", Value::Str(name.into()));
    }
    if let Some(justify) = node.style.justify_content {
        let name = match justify {
            Justify::Start => "start",
            Justify::Center => "center",
            Justify::End => "end",
            Justify::Between => "between",
        };
        put(&mut props, "justify", Value::Str(name.into()));
    }
    props
}

fn put(props: &mut PropSet, name: &str, value: Value) {
    if let Some(prop) = PropId::from_name(name) {
        props.push((prop, value));
    }
}

fn put_len(props: &mut PropSet, name: &str, length: Length) {
    match length {
        Length::Auto => {}
        Length::Px(v) | Length::Fraction(v) => {
            put(props, name, Value::F64(f64::from(v)));
        }
        Length::Full => put(props, name, Value::Str("100%".into())),
    }
}

fn quad(padding: Padding) -> [f32; 4] {
    [
        length(padding.left),
        length(padding.top),
        length(padding.right),
        length(padding.bottom),
    ]
}

fn length(length: Length) -> f32 {
    match length {
        Length::Px(v) => v,
        _ => 0.0,
    }
}

fn default_text(theme: &Theme) -> Color {
    theme
        .tokens
        .resolve_or(&Token::new(Token::TEXT_DEFAULT), Color::hex(0xdbdee1))
}

fn text_role(style: TextStyle) -> (f32, i32) {
    match style {
        TextStyle::Heading => (20.0, 600),
        TextStyle::Body | TextStyle::Muted => (14.0, 400),
        TextStyle::Small => (12.0, 400),
    }
}

/// Flatten a lowered tree into one turn: a `Create` per node in pre-order,
/// a `Children` per parent, and the root.
#[must_use]
pub fn to_turn(root: &WireNode) -> Turn {
    let mut ops = Vec::new();
    walk(root, &mut ops);
    ops.push(NodeOp::Root { node: root.id });
    Turn { ops }
}

fn walk(node: &WireNode, ops: &mut Vec<NodeOp>) {
    ops.push(NodeOp::Create {
        node: node.id,
        kind: node.kind.clone(),
        props: node.props.clone(),
    });
    if !node.children.is_empty() {
        ops.push(NodeOp::Children {
            node: node.id,
            children: node.children.iter().map(|c| c.id).collect(),
        });
        for child in &node.children {
            walk(child, ops);
        }
    }
}

/// The wire's `0xAARRGGBB` for a brush.
#[must_use]
pub fn argb(theme: &Theme, brush: &Brush) -> u32 {
    theme.argb(brush, Color::hex(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Tokens;

    fn theme() -> Theme {
        Theme::new("test", Tokens::defaults())
    }

    fn props(node: &WireNode) -> Vec<(&'static str, String)> {
        node.props
            .iter()
            .filter_map(|(id, value)| Some((id.name()?, format!("{value:?}"))))
            .collect()
    }

    #[test]
    fn row_is_a_view_with_its_children() {
        let ui = Ui::new(Node::row(vec![Node::text("hello"), Node::text("world")]).gap(8.0));
        let root = lower(&ui, &theme());
        assert_eq!(root.kind, ElementKind::VIEW);
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].kind, ElementKind::TEXT);
        let names: Vec<&str> = props(&root).iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"gap"));
    }

    #[test]
    fn text_carries_text_size_and_colour() {
        let root = lower_node(&Node::heading("Rekindle"), &theme(), None, &mut 1);
        let values: Vec<(&str, String)> = props(&root);
        assert!(
            values
                .iter()
                .any(|(n, v)| *n == "text" && v.contains("Rekindle"))
        );
        assert!(
            values
                .iter()
                .any(|(n, v)| *n == "fontSize" && v.contains("20"))
        );
        assert!(values.iter().any(|(n, _)| *n == "color"));
    }

    #[test]
    fn button_is_a_view_with_a_press_listener() {
        let root = lower_node(
            &Node::button("save", "Save").primary(),
            &theme(),
            None,
            &mut 1,
        );
        let values = props(&root);
        assert!(values.iter().any(|(n, _)| *n == "on"));
        let background = root
            .props
            .iter()
            .find(|(id, _)| id.name() == Some("background"))
            .and_then(|(_, value)| value.as_int());
        assert_eq!(background, Some(i64::from(0xff5865f2_u32)));
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].kind, ElementKind::TEXT);
    }

    #[test]
    fn toggle_is_label_and_track() {
        let root = lower_node(&Node::toggle("on", "Enabled", true), &theme(), None, &mut 1);
        assert_eq!(root.children.len(), 2);
        let track = &root.children[1];
        assert_eq!(track.children.len(), 1);
        assert!(
            track.children[0]
                .props
                .iter()
                .any(|(id, _)| id.name() == Some("radius"))
        );
    }

    #[test]
    fn turn_creates_every_node_and_sets_the_root() {
        let ui = Ui::new(Node::row(vec![Node::text("a"), Node::text("b")]));
        let turn = to_turn(&lower(&ui, &theme()));
        let creates = turn
            .ops
            .iter()
            .filter(|op| matches!(op, NodeOp::Create { .. }))
            .count();
        assert_eq!(creates, 3);
        assert!(matches!(turn.ops.last(), Some(NodeOp::Root { node: 1 })));
        let children = turn
            .ops
            .iter()
            .filter(|op| matches!(op, NodeOp::Children { .. }))
            .count();
        assert_eq!(children, 1);
    }

    #[test]
    fn the_same_tree_lowers_to_both_backends() {
        let ui = Ui::card("Rekindle", "Once.");
        let wire = to_turn(&lower(&ui, &theme()));
        let gpui = crate::gpui::lower(&ui, &theme());
        assert!(!wire.ops.is_empty());
        assert!(matches!(gpui, crate::gpui::GpuiNode::Div(_)));
    }
}
