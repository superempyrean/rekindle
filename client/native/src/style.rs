//! What rekindle wants an element to look like, in plain terms. Nothing here
//! is a gpui type; building an element turns it into bytes at recovered
//! offsets.

use std::sync::Arc;

use crate::client::{Ctx, Focus, Key};

/// A colour as 0..1 channels, like gpui's `Rgba`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    /// `0xRRGGBBAA`.
    pub fn hex(v: u32) -> Self {
        let c = |shift: u32| ((v >> shift) & 0xff) as f32 / 255.0;
        Rgba {
            r: c(24),
            g: c(16),
            b: c(8),
            a: c(0),
        }
    }

    /// gpui's own `Hsla -> Rgba` conversion.
    pub fn from_hsla([h, s, l, a]: [f32; 4]) -> Self {
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let x = c * (1.0 - ((h * 6.0) % 2.0 - 1.0).abs());
        let m = l - c / 2.0;
        let (cm, xm) = (c + m, x + m);
        let (r, g, b) = match (h * 6.0).floor() as i32 {
            0 | 6 => (cm, xm, m),
            1 => (xm, cm, m),
            2 => (m, cm, xm),
            3 => (m, xm, cm),
            4 => (xm, m, cm),
            _ => (cm, m, xm),
        };
        Rgba {
            r: r.clamp(0.0, 1.0),
            g: g.clamp(0.0, 1.0),
            b: b.clamp(0.0, 1.0),
            a,
        }
    }

    /// gpui's own `Rgba -> Hsla` conversion (h, s, l, a, all 0..1). The
    /// framework layer finds colours in the scene by these values, so they
    /// must come out exactly as gpui computes them.
    #[expect(
        clippy::float_cmp,
        reason = "gpui's conversion compares exactly, and the results must match it bit for bit"
    )]
    pub fn to_hsla(self) -> [f32; 4] {
        let (r, g, b) = (self.r, self.g, self.b);
        let max = r.max(g.max(b));
        let min = r.min(g.min(b));
        let delta = max - min;
        let l = f32::midpoint(max, min);
        let s = if l == 0.0 || l == 1.0 {
            0.0
        } else if l < 0.5 {
            delta / (2.0 * l)
        } else {
            delta / (2.0 - 2.0 * l)
        };
        let h = if delta == 0.0 {
            0.0
        } else if max == r {
            ((g - b) / delta).rem_euclid(6.0) / 6.0
        } else if max == g {
            ((b - r) / delta + 2.0) / 6.0
        } else {
            ((r - g) / delta + 4.0) / 6.0
        };
        [h, s, l, self.a]
    }
}

/// A length for sizes and insets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Len {
    /// Logical pixels.
    Px(f32),
    /// A fraction of the parent (1.0 = 100%).
    Frac(f32),
    Auto,
}

/// A value for each side of a box.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Edges<T> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

impl<T: Copy> Edges<T> {
    /// `v` on every side.
    pub fn all(v: T) -> Self {
        Edges {
            top: v,
            right: v,
            bottom: v,
            left: v,
        }
    }

    /// `x` on the left and right, `y` on the top and bottom.
    pub fn xy(x: T, y: T) -> Self {
        Edges {
            top: y,
            right: x,
            bottom: y,
            left: x,
        }
    }
}

/// A flex direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dir {
    Row,
    Column,
}

/// Where flex items sit on an axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Align {
    Start,
    Center,
    End,
}

/// An element's style: each `None` (or `false`) leaves gpui's default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Style {
    pub width: Option<Len>,
    pub height: Option<Len>,
    /// Out of flow, placed by `inset` against the parent.
    pub absolute: bool,
    pub inset: Edges<Option<Len>>,
    pub padding: Edges<Option<f32>>,
    pub gap: Option<f32>,
    pub radius: Option<f32>,
    pub background: Option<Rgba>,
    /// Width (all sides) and colour.
    pub border: Option<(f32, Rgba)>,
    pub text_color: Option<Rgba>,
    pub text_size: Option<f32>,
    pub line_height: Option<f32>,
    /// `display: flex` with this direction.
    pub flex: Option<Dir>,
    pub align_items: Option<Align>,
    pub justify: Option<Align>,
    /// Background while hovered (gpui's `hover` style; gpui decides hover).
    pub hover_background: Option<Rgba>,
    /// Block the mouse for whatever is behind (gpui's `occlude`).
    pub occlude: bool,
    /// `visibility`: `Some(false)` lays the element out but doesn't paint it
    /// (or its children); a hover or group-hover style can set `Some(true)`.
    /// Not `display: none`: gpui recomputes styles between prepaint and
    /// paint, after updating the hit test, so a hover-toggled display can
    /// paint children it never prepainted, which panics. Zed's
    /// `visible_on_hover` toggles visibility for the same reason.
    pub visible: Option<bool>,
    /// Font weight (400 normal, 600 semibold, 700 bold).
    pub font_weight: Option<f32>,
    /// The mouse cursor over the element.
    pub cursor: Option<Cursor>,
    /// Scroll the children vertically, as gpui's `overflow_y_scroll()` does,
    /// keeping the position under this key across frames. Give the element
    /// a height. A layout file without the scroll entries leaves it unclipped
    /// and still ([`crate::can_scroll`]).
    pub scroll_y: Option<String>,
}

/// A mouse cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cursor {
    /// A pointing hand (clickable).
    Pointer,
    /// An I-beam (text).
    Text,
}

/// What a click on an element runs (left button, the element hovered as
/// gpui's hit test sees it), with the client's window and app. Compared by
/// identity.
#[derive(Clone)]
pub struct Handler(pub(crate) Arc<dyn Fn(&mut Ctx) + Send + Sync>);

/// What a key press on a focused element runs. Compared by identity.
#[derive(Clone)]
pub struct KeyHandler(pub(crate) Arc<KeyFn>);

type KeyFn = dyn Fn(&mut Ctx, &Key) + Send + Sync;

impl std::fmt::Debug for Handler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Handler")
    }
}

impl PartialEq for Handler {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for KeyHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeyHandler")
    }
}

impl PartialEq for KeyHandler {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// A div: its style, its children, and what makes it interactive.
#[derive(Clone, Debug, PartialEq)]
pub struct Div {
    pub style: Style,
    pub children: Vec<Node>,
    pub on_click: Option<Handler>,
    /// Key presses while this element (tracking `focus`) is focused.
    pub on_key: Option<KeyHandler>,
    /// gpui's `track_focus`: this element is where `focus` is.
    pub focus: Option<Focus>,
    /// Name this element as a gpui group, for descendants' group-hover styles.
    pub group: Option<String>,
    /// Style applied while the named group is hovered (gpui decides).
    pub group_hover: Option<(String, Style)>,
    /// Quads and images rekindle paints itself inside this element,
    /// relative to its origin (`crate::can_paint`). Needs a width and height
    /// in pixels.
    pub paint: Option<Arc<[crate::Paint]>>,
}

/// An element tree rekindle wants in the client's window.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Div(Box<Div>),
    Text(String),
}

impl Node {
    /// A div of `style` holding `children`.
    pub fn div(style: Style, children: Vec<Node>) -> Node {
        Node::Div(Box::new(Div {
            style,
            children,
            on_click: None,
            on_key: None,
            focus: None,
            group: None,
            group_hover: None,
            paint: None,
        }))
    }

    /// A div of `style` (a width and height in pixels) that rekindle paints
    /// `items` in, as one element ([`crate::can_paint`]).
    pub fn painted(style: Style, items: Vec<crate::Paint>) -> Node {
        Node::div(style, vec![]).with_div(|d| d.paint = Some(items.into()))
    }

    /// A run of text.
    pub fn text(s: impl Into<String>) -> Node {
        Node::Text(s.into())
    }

    /// Make this (div) element a gpui group named `name`.
    pub fn group(self, name: impl Into<String>) -> Node {
        self.with_div(|d| d.group = Some(name.into()))
    }

    /// Apply `style` to this (div) element while group `name` is hovered.
    pub fn group_hover(self, name: impl Into<String>, style: Style) -> Node {
        self.with_div(|d| d.group_hover = Some((name.into(), style)))
    }

    /// Run `f` when this (div) element is clicked.
    pub fn on_click(self, f: impl Fn() + Send + Sync + 'static) -> Node {
        self.on_click_cx(move |_| f())
    }

    /// Run `f` with the client's window and app when this (div) element is
    /// clicked (to focus, refresh, ...).
    pub fn on_click_cx(self, f: impl Fn(&mut Ctx) + Send + Sync + 'static) -> Node {
        self.with_div(|d| d.on_click = Some(Handler(Arc::new(f))))
    }

    /// Make this (div) element where `focus` is, and run `f` on key presses
    /// while it's focused.
    pub fn focusable(
        self,
        focus: Focus,
        f: impl Fn(&mut Ctx, &Key) + Send + Sync + 'static,
    ) -> Node {
        self.with_div(|d| {
            d.focus = Some(focus);
            d.on_key = Some(KeyHandler(Arc::new(f)));
        })
    }

    /// `self` with `f` applied, if it is a div; text is returned as it is.
    fn with_div(self, f: impl FnOnce(&mut Div)) -> Node {
        match self {
            Node::Div(mut d) => {
                f(&mut d);
                Node::Div(d)
            }
            text @ Node::Text(_) => text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(clippy::float_cmp, reason = "pure colours and greys convert exactly")]
    fn hsla_matches_gpui_for_known_colours() {
        // gpui: rgb(0xff0000) -> h 0, s 1, l 0.5; rgb(0x00ff00) -> h 1/3; 0x0000ff -> 2/3.
        let red = Rgba::hex(0xff0000ff).to_hsla();
        assert_eq!(red, [0.0, 1.0, 0.5, 1.0]);
        assert!((Rgba::hex(0x00ff00ff).to_hsla()[0] - 1.0 / 3.0).abs() < 1e-6);
        assert!((Rgba::hex(0x0000ffff).to_hsla()[0] - 2.0 / 3.0).abs() < 1e-6);
        // And back (gpui's Hsla -> Rgba).
        let back = Rgba::from_hsla(Rgba::hex(0x336699cc).to_hsla());
        let want = Rgba::hex(0x336699cc);
        assert!(
            (back.r - want.r).abs() < 1e-5
                && (back.g - want.g).abs() < 1e-5
                && (back.b - want.b).abs() < 1e-5
        );
        // Greys have no hue or saturation.
        assert_eq!(Rgba::hex(0x808080ff).to_hsla()[..2], [0.0, 0.0]);
        // Alpha passes through.
        assert!((Rgba::hex(0x00000080).to_hsla()[3] - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn builders_apply_to_divs_and_leave_text_alone() {
        let d = Node::div(Style::default(), vec![])
            .group("g")
            .on_click(|| {});
        let Node::Div(d) = d else { panic!("a div") };
        assert_eq!(d.group.as_deref(), Some("g"));
        assert!(d.on_click.is_some());
        assert_eq!(Node::text("t").group("g"), Node::text("t"));
    }
}
