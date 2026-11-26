//! rekindle's toasts: real elements stacked up from the bottom right of the
//! client's window, newest at the bottom. Each goes after a few seconds, or
//! when its close button is clicked.
//!
//! A card takes no pointer input except on its close button. The rest of
//! it has no hitbox, so a press there reaches whatever the client drew
//! beneath, such as a composer button the toast happens to cover.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rekindle_native::{Align, Cursor, Dir, Edges, Len, Node, Rgba, Style};

/// How a rekindle toast is styled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastStyle {
    Info,
    Success,
    Warning,
    Error,
}

/// How many toasts are kept at once.
pub const MAX_TOASTS: usize = 4;
const LIFETIME: Duration = Duration::from_secs(5);
/// Right inset of the stack, in logical pixels.
const RIGHT: f32 = 16.0;

/// A card's fill: the client's floating-surface colour, so a theme
/// recolours toasts along with its own menus.
pub(crate) const CARD: u32 = 0x111214f0;
/// The close button's fill, and its fill while hovered.
pub(crate) const CLOSE: u32 = 0x1e1f22f0;
pub(crate) const CLOSE_HOVER: u32 = 0x35373cf0;

type Rect = (f32, f32, f32, f32);

struct Toast {
    id: u64,
    text: String,
    style: ToastStyle,
    born: Instant,
    lifetime: Duration,
}

static TOASTS: Mutex<Vec<Toast>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Where the cards were painted in the last frame that was measured.
static CARDS: Mutex<Vec<Rect>> = Mutex::new(Vec::new());

/// Show a toast. At most [`MAX_TOASTS`] are kept, newest last.
pub fn push_toast(text: &str, style: ToastStyle) {
    push_toast_for(text, style, LIFETIME);
}

/// Show a toast for `lifetime` instead of the usual five seconds. Test runs
/// use it to keep one on screen while they press something.
pub fn push_toast_for(text: &str, style: ToastStyle, lifetime: Duration) {
    let mut t = TOASTS.lock().unwrap();
    t.push(Toast {
        id: NEXT_ID.fetch_add(1, Ordering::SeqCst),
        text: text.to_string(),
        style,
        born: Instant::now(),
        lifetime,
    });
    let excess = t.len().saturating_sub(MAX_TOASTS);
    t.drain(..excess);
}

/// The toasts currently shown (for self-tests).
pub fn toast_texts() -> Vec<String> {
    TOASTS
        .lock()
        .unwrap()
        .iter()
        .map(|t| t.text.clone())
        .collect()
}

/// A toast that is up: what a client without a framework layer draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveToast {
    /// Unique for the life of the process.
    pub id: u64,
    pub text: String,
    pub style: ToastStyle,
}

/// The toasts that are up, oldest first. Those whose time is up are taken
/// out first.
pub fn live_toasts() -> Vec<LiveToast> {
    let mut toasts = TOASTS.lock().unwrap();
    toasts.retain(|t| t.born.elapsed() < t.lifetime);
    toasts
        .iter()
        .map(|t| LiveToast {
            id: t.id,
            text: t.text.clone(),
            style: t.style,
        })
        .collect()
}

/// Take the toast `id` down before its time is up.
pub fn dismiss_toast(id: u64) {
    TOASTS.lock().unwrap().retain(|t| t.id != id);
}

/// Where the cards were painted (logical pixels: x, y, width, height), as
/// last measured by [`note_cards`]. For self-tests.
pub fn toast_cards() -> Vec<Rect> {
    CARDS.lock().unwrap().clone()
}

/// Whether any toast is up.
pub(crate) fn any() -> bool {
    !TOASTS.lock().unwrap().is_empty()
}

/// Record where the cards are in a finished frame: the quads in the card
/// fill whose right edge is the stack's, in a window `width` wide.
pub(crate) fn note_cards(quads: &[crate::drive::FrameQuad], width: f32) {
    let want = Rgba::hex(CARD).to_hsla();
    let cards = quads
        .iter()
        .filter(|(r, c)| {
            c.iter().zip(want).all(|(a, b)| (a - b).abs() < 2e-3)
                && (r.0 + r.2 - (width - RIGHT)).abs() < 1.0
        })
        .map(|(r, _)| *r)
        .collect();
    *CARDS.lock().unwrap() = cards;
}

/// The live toasts, stacked `bottom` logical pixels above the window's
/// bottom edge. `None` when there are none.
pub(crate) fn stack(bottom: f32) -> Option<Node> {
    let mut toasts = TOASTS.lock().unwrap();
    toasts.retain(|t| t.born.elapsed() < t.lifetime);
    if toasts.is_empty() {
        CARDS.lock().unwrap().clear();
        return None;
    }
    let cards = toasts.iter().map(card).collect();
    Some(Node::div(
        Style {
            absolute: true,
            inset: Edges {
                top: None,
                right: Some(Len::Px(RIGHT)),
                bottom: Some(Len::Px(bottom)),
                left: None,
            },
            flex: Some(Dir::Column),
            gap: Some(8.0),
            align_items: Some(Align::End),
            ..Default::default()
        },
        cards,
    ))
}

/// One toast's card. It has no listener, hover style or occlusion, so gpui
/// gives it no hitbox.
fn card(t: &Toast) -> Node {
    let accent = match t.style {
        ToastStyle::Info => 0x5865f2ff,
        ToastStyle::Success => 0x23a55aff,
        ToastStyle::Warning => 0xf0b232ff,
        ToastStyle::Error => 0xda373cff,
    };
    Node::div(
        Style {
            flex: Some(Dir::Row),
            align_items: Some(Align::Center),
            padding: Edges {
                top: None,
                right: Some(8.0),
                bottom: None,
                left: None,
            },
            background: Some(Rgba::hex(CARD)),
            border: Some((1.0, Rgba::hex(0xffffff1f))),
            radius: Some(6.0),
            ..Default::default()
        },
        vec![
            Node::div(
                Style {
                    width: Some(Len::Px(4.0)),
                    height: Some(Len::Px(36.0)),
                    background: Some(Rgba::hex(accent)),
                    radius: Some(2.0),
                    ..Default::default()
                },
                vec![],
            ),
            Node::div(
                Style {
                    padding: Edges {
                        top: Some(8.0),
                        right: Some(8.0),
                        bottom: Some(8.0),
                        left: Some(10.0),
                    },
                    text_color: Some(Rgba::hex(0xf2f3f5ff)),
                    text_size: Some(14.0),
                    line_height: Some(20.0),
                    ..Default::default()
                },
                vec![Node::text(t.text.clone())],
            ),
            close(t.id),
        ],
    )
}

/// The card's close button, the only part of a toast that takes a press.
fn close(id: u64) -> Node {
    Node::div(
        Style {
            width: Some(Len::Px(20.0)),
            height: Some(Len::Px(20.0)),
            flex: Some(Dir::Row),
            align_items: Some(Align::Center),
            justify: Some(Align::Center),
            background: Some(Rgba::hex(CLOSE)),
            hover_background: Some(Rgba::hex(CLOSE_HOVER)),
            radius: Some(4.0),
            text_color: Some(Rgba::hex(0xb5bac1ff)),
            text_size: Some(14.0),
            line_height: Some(20.0),
            occlude: true,
            cursor: Some(Cursor::Pointer),
            ..Default::default()
        },
        vec![Node::text("\u{d7}")],
    )
    .on_click(move || dismiss_toast(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every div of `node` that would get a hitbox from gpui: one with a
    /// listener, a hover style, occlusion or a group name.
    fn hit_targets(node: &Node, out: &mut Vec<Style>) {
        let Node::Div(d) = node else { return };
        if d.on_click.is_some()
            || d.on_key.is_some()
            || d.focus.is_some()
            || d.group.is_some()
            || d.style.occlude
            || d.style.hover_background.is_some()
        {
            out.push(d.style.clone());
        }
        for c in &d.children {
            hit_targets(c, out);
        }
    }

    #[test]
    fn only_the_close_buttons_take_input() {
        push_toast("one", ToastStyle::Info);
        push_toast("two", ToastStyle::Error);
        let stack = stack(16.0).expect("a stack");
        let mut targets = Vec::new();
        hit_targets(&stack, &mut targets);
        assert!(targets.len() >= 2, "every card has its close button");
        for s in targets {
            assert_eq!(s.background, Some(Rgba::hex(CLOSE)));
            assert_eq!(
                (s.width, s.height),
                (Some(Len::Px(20.0)), Some(Len::Px(20.0)))
            );
        }
    }

    #[test]
    fn a_held_toast_outlives_the_usual_lifetime() {
        push_toast_for("held", ToastStyle::Info, Duration::from_secs(600));
        {
            let mut t = TOASTS.lock().unwrap();
            let held = t.iter_mut().find(|t| t.text == "held").unwrap();
            held.born = Instant::now().checked_sub(LIFETIME * 2).unwrap();
        }
        let _ = stack(16.0);
        assert!(toast_texts().iter().any(|t| t == "held"));
    }

    #[test]
    fn a_toast_whose_time_is_up_is_not_live() {
        push_toast_for("gone", ToastStyle::Warning, Duration::from_secs(5));
        push_toast_for("kept", ToastStyle::Success, Duration::from_secs(600));
        {
            let mut t = TOASTS.lock().unwrap();
            let gone = t.iter_mut().find(|t| t.text == "gone").unwrap();
            gone.born = Instant::now().checked_sub(LIFETIME * 2).unwrap();
        }
        let live = live_toasts();
        assert!(!live.iter().any(|t| t.text == "gone"));
        assert!(
            live.iter()
                .any(|t| t.text == "kept" && t.style == ToastStyle::Success)
        );
    }

    #[test]
    fn cards_are_the_card_fill_on_the_stacks_edge() {
        let card = Rgba::hex(CARD).to_hsla();
        let other = Rgba::hex(CLOSE).to_hsla();
        note_cards(
            &[
                ((600.0, 500.0, 184.0, 38.0), card),
                // The client's own surface of the same fill, elsewhere.
                ((10.0, 100.0, 200.0, 40.0), card),
                ((756.0, 509.0, 20.0, 20.0), other),
            ],
            800.0,
        );
        assert_eq!(toast_cards(), [(600.0, 500.0, 184.0, 38.0)]);
    }
}
