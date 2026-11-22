//! rekindle's window: the panels of rekindle's settings page (the plugin
//! grid, and the panels of the plugin whose page is open) in a window
//! rekindle draws over the client itself, for a client whose settings page
//! rekindle can't add to. Real elements in the framework layer, so it needs
//! nothing of the client's but GPUI. A button rekindle puts in the chat bar
//! opens it, and so does Ctrl+Shift+, ([`SHORTCUT`]), which doesn't depend
//! on rekindle finding the chat bar. Escape, or a click outside it, closes
//! it. Its body scrolls where the layout lets rekindle's elements scroll
//! (`rekindle_native::can_scroll`).

use std::sync::atomic::{AtomicBool, Ordering};

use rekindle_native::{Align, Ctx, Dir, Edges, Len, Node, Rgba, Style};

static OPEN: AtomicBool = AtomicBool::new(false);
static BUTTON: AtomicBool = AtomicBool::new(false);

/// The scroll position of the window's body (`rekindle_native::scroll_y_position`).
pub(crate) const SCROLL_KEY: &str = "rekindle-window";
/// The chat-bar button's key, among the plugins' buttons.
pub(crate) const BUTTON_KEY: &str = "rekindle/window";

/// Show or hide the window.
pub(crate) fn open(on: bool) {
    OPEN.store(on, Ordering::SeqCst);
}

pub(crate) fn is_open() -> bool {
    OPEN.load(Ordering::SeqCst)
}

/// Whether rekindle's chat-bar button opens the window: on where the
/// client's settings page has no page of rekindle's.
pub fn set_button(on: bool) {
    BUTTON.store(on, Ordering::SeqCst);
}

pub(crate) fn has_button() -> bool {
    BUTTON.load(Ordering::SeqCst)
}

/// The keyboard shortcut that opens and closes the window: Ctrl+Shift+,
/// (Ctrl+, opens the client's settings). It works whether or not the chat
/// bar has rekindle's button, so there is always a way in.
pub(crate) const SHORTCUT: &str = "Ctrl+Shift+,";

/// Whether `key` is [`SHORTCUT`]. gpui names the key `,`, or `<` where it
/// names the shifted key.
pub(crate) fn shortcut(key: &rekindle_native::Key) -> bool {
    key.control && key.shift && !key.alt && !key.platform && (key.key == "," || key.key == "<")
}

/// The chat-bar button's glyph and tooltip.
pub(crate) fn button() -> crate::registry::Button {
    crate::registry::Button {
        key: BUTTON_KEY.into(),
        glyph: "\u{2699}".into(),
        tooltip: format!("rekindle ({SHORTCUT})"),
    }
}

const MARGIN: f32 = 48.0;
const HEADER: f32 = 52.0;
const MAX_WIDTH: f32 = 760.0;

/// The window's elements for this frame, over a `viewport` (logical
/// pixels): a scrim that closes it when clicked, and the window.
pub(crate) fn nodes(ctx: &mut Ctx, viewport: Option<(f32, f32)>) -> Vec<Node> {
    let Some((vw, vh)) = viewport.filter(|_| is_open()) else {
        return Vec::new();
    };
    let width = (vw - 2.0 * MARGIN).clamp(320.0, MAX_WIDTH);
    let height = (vh - 2.0 * MARGIN).max(240.0);
    let scrim = Node::div(
        Style {
            absolute: true,
            inset: Edges::all(Some(Len::Px(0.0))),
            background: Some(Rgba::hex(0x000000a6)),
            occlude: true,
            ..Default::default()
        },
        vec![],
    )
    .on_click(|| open(false));
    let close = Node::div(
        Style {
            absolute: true,
            inset: Edges {
                top: Some(Len::Px(12.0)),
                right: Some(Len::Px(12.0)),
                bottom: None,
                left: None,
            },
            padding: Edges::xy(Some(10.0), Some(4.0)),
            radius: Some(6.0),
            hover_background: Some(Rgba::hex(0xffffff1a)),
            cursor: Some(rekindle_native::Cursor::Pointer),
            ..Default::default()
        },
        vec![Node::text("Close")],
    )
    .on_click(|| open(false));
    let header = Node::div(
        Style {
            height: Some(Len::Px(HEADER)),
            flex: Some(Dir::Row),
            align_items: Some(Align::Center),
            padding: Edges::xy(Some(16.0), None),
            border: Some((1.0, Rgba::hex(0xffffff14))),
            ..Default::default()
        },
        vec![
            Node::div(
                Style {
                    font_weight: Some(700.0),
                    text_size: Some(16.0),
                    ..Default::default()
                },
                vec![Node::text("rekindle")],
            ),
            close,
        ],
    );
    // A layout that can't scroll can't clip either: the window grows to fit
    // its body instead.
    let scrolls = rekindle_native::can_scroll();
    let body = Node::div(
        Style {
            height: scrolls.then_some(Len::Px(height - HEADER)),
            padding: Edges::all(Some(16.0)),
            scroll_y: Some(SCROLL_KEY.into()),
            ..Default::default()
        },
        vec![crate::panel::settings_content(ctx)],
    );
    let window = Node::div(
        Style {
            absolute: true,
            inset: Edges {
                top: Some(Len::Px(MARGIN)),
                right: None,
                bottom: None,
                left: Some(Len::Px((vw - width) / 2.0)),
            },
            width: Some(Len::Px(width)),
            height: scrolls.then_some(Len::Px(height)),
            flex: Some(Dir::Column),
            // The client's own surface colour, so a theme recolours the
            // window along with the client.
            background: Some(Rgba::hex(0x313338ff)),
            border: Some((1.0, Rgba::hex(0xffffff1f))),
            radius: Some(8.0),
            text_color: Some(Rgba::hex(0xf2f3f5ff)),
            text_size: Some(14.0),
            line_height: Some(20.0),
            occlude: true,
            ..Default::default()
        },
        vec![header, body],
    );
    vec![scrim, window]
}
