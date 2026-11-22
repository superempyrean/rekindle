//! Chat-bar buttons through the framework layer: the composer row found by
//! shape in each frame, and rekindle's buttons placed left of the client's.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rekindle_native::{Align, Dir, Edges, Len, Node, Rgba, Style, client};

use crate::registry::Button;
use crate::{Click, discovery, queue_click, split_key, window};

static ON: AtomicBool = AtomicBool::new(false);
static BUTTONS: Mutex<Vec<Button>> = Mutex::new(Vec::new());
static FOUND_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Where the chat bar's first (leftmost) button was last found, and the gap
/// between its buttons: where rekindle's buttons go, to its left.
static CHAT_BAR: Mutex<Option<(discovery::Rect, f32)>> = Mutex::new(None);
/// Where rekindle placed its own buttons, so discovery doesn't count them as
/// the client's.
static OWN_BUTTONS: Mutex<Vec<discovery::Rect>> = Mutex::new(Vec::new());
/// Clicks on rekindle's chat-bar buttons, through the framework layer or
/// the client's registry.
pub(crate) static BUTTON_CLICKS: AtomicUsize = AtomicUsize::new(0);

type FrameDump = Option<(Vec<(f32, f32, f32, f32)>, Vec<(f32, f32, f32, f32)>)>;
static LAST_FRAME: Mutex<FrameDump> = Mutex::new(None);

/// Contribute the plugins' chat-bar buttons through the framework layer: the
/// composer row is found each frame by shape ([`discovery::find_chat_bar`])
/// from the client's own hitboxes and quads, and the buttons are real
/// elements placed in it. No client function is involved.
pub fn set_framework_buttons(on: bool, buttons: Vec<Button>) {
    ON.store(on, Ordering::SeqCst);
    *BUTTONS.lock().unwrap() = buttons;
}

/// Whether the framework layer places chat-bar buttons.
pub(crate) fn framework_buttons_on() -> bool {
    ON.load(Ordering::SeqCst)
}

/// Where rekindle placed its own buttons last frame, left to right.
pub(crate) fn own_buttons() -> Vec<discovery::Rect> {
    OWN_BUTTONS.lock().unwrap().clone()
}

/// The chat bar's first button, where it was last found.
pub(crate) fn first_button() -> Option<discovery::Rect> {
    CHAT_BAR.lock().unwrap().map(|(first, _)| first)
}

/// The last frame's geometry as the framework layer read it (diagnostics):
/// `None` if the record read failed, else (hitboxes, filled) as (x, y, w, h).
pub fn last_frame_geometry() -> FrameDump {
    LAST_FRAME.lock().unwrap().clone()
}

/// Frames in which the framework layer found the chat bar (for status).
pub fn chat_bar_frames() -> usize {
    FOUND_FRAMES.load(Ordering::SeqCst)
}

/// Clicks on rekindle's chat-bar buttons, as routed by the client's gpui.
pub fn button_clicks() -> usize {
    BUTTON_CLICKS.load(Ordering::SeqCst)
}

/// rekindle's chat-bar buttons as real elements, left of the client's own,
/// sized like them: hover background, a tooltip shown while the button (a
/// gpui group) is hovered, click routed back as [`Click::Button`].
pub(crate) fn nodes() -> Vec<Node> {
    let mut own = OWN_BUTTONS.lock().unwrap();
    own.clear();
    if !framework_buttons_on() {
        return Vec::new();
    }
    let Some((first, gap)) = *CHAT_BAR.lock().unwrap() else {
        return Vec::new();
    };
    let mut buttons = BUTTONS.lock().unwrap().clone();
    // rekindle's own, left of the plugins'.
    if window::has_button() {
        buttons.insert(0, window::button());
    }
    let mut x = first.x;
    let mut nodes = Vec::new();
    for b in buttons.iter().rev() {
        x -= first.w + gap;
        let (y, w, h) = (first.y, first.w, first.h);
        own.push(discovery::Rect::new(x, y, w, h));
        let group = format!("rekindle-button-{}", b.key);
        let mut children = vec![Node::text(b.glyph.clone())];
        if !b.tooltip.is_empty() {
            children.push(tooltip(&b.tooltip, w, &group));
        }
        let key = b.key.clone();
        nodes.push(
            Node::div(
                Style {
                    absolute: true,
                    inset: Edges {
                        top: Some(Len::Px(y)),
                        right: None,
                        bottom: None,
                        left: Some(Len::Px(x)),
                    },
                    width: Some(Len::Px(w)),
                    height: Some(Len::Px(h)),
                    flex: Some(Dir::Row),
                    align_items: Some(Align::Center),
                    justify: Some(Align::Center),
                    radius: Some(4.0),
                    hover_background: Some(Rgba::hex(0xffffff1a)),
                    text_color: Some(Rgba::hex(0xb5bac1ff)),
                    text_size: Some((h * 0.5).clamp(10.0, 20.0)),
                    occlude: true,
                    ..Default::default()
                },
                children,
            )
            .group(group)
            .on_click(move || {
                BUTTON_CLICKS.fetch_add(1, Ordering::SeqCst);
                if key == window::BUTTON_KEY {
                    return window::open(!window::is_open());
                }
                let (plugin, item) = split_key(&key);
                queue_click(Click::Button { plugin, item });
            }),
        );
    }
    nodes
}

/// A button's tooltip, shown while the button's gpui group is hovered.
fn tooltip(text: &str, button_width: f32, group: &str) -> Node {
    Node::div(
        Style {
            absolute: true,
            visible: Some(false),
            // Centred over the button: a wide, input-free strip above it.
            inset: Edges {
                top: None,
                right: None,
                bottom: Some(Len::Frac(1.0)),
                left: Some(Len::Px(-100.0)),
            },
            width: Some(Len::Px(button_width + 200.0)),
            padding: Edges {
                top: None,
                right: None,
                bottom: Some(6.0),
                left: None,
            },
            flex: Some(Dir::Row),
            justify: Some(Align::Center),
            ..Default::default()
        },
        vec![Node::div(
            Style {
                background: Some(Rgba::hex(0x111214f0)),
                radius: Some(4.0),
                padding: Edges::xy(Some(8.0), Some(4.0)),
                text_color: Some(Rgba::hex(0xf2f3f5ff)),
                text_size: Some(12.0),
                line_height: Some(16.0),
                ..Default::default()
            },
            vec![Node::text(text.to_string())],
        )],
    )
    .group_hover(
        group.to_string(),
        Style {
            visible: Some(true),
            ..Default::default()
        },
    )
}

fn to_rect(r: &client::Rect) -> discovery::Rect {
    discovery::Rect::new(r.0, r.1, r.2, r.3)
}

/// The frame GPUI just drew, as recorded: every hitbox and every filled
/// quad (a solid, non-transparent fill), including those replayed from
/// the previous frame for cached views. `None` if the record doesn't look
/// like one (wrong layout for this build): then nothing is located.
///
/// # Safety
///
/// `window` is the client's live `Window`, on its UI thread.
unsafe fn read_frame(window: *const u8) -> Option<(Vec<discovery::Rect>, Vec<discovery::Rect>)> {
    // SAFETY: the caller's contract.
    let f = unsafe { client::frame(window)? };
    let filled = f
        .quads
        .iter()
        .filter(|(_, c)| c.is_some_and(|c| c[3] > 0.0))
        .map(|(r, _)| to_rect(r))
        .collect();
    Some((f.hitboxes.iter().map(to_rect).collect(), filled))
}

/// Find the composer row from this frame's geometry (not counting
/// rekindle's own buttons) for next frame's buttons to sit in.
///
/// # Safety
///
/// As [`read_frame`].
pub(crate) unsafe fn locate(window: *const u8) {
    // SAFETY: the caller's contract.
    let frame = unsafe { read_frame(window) };
    *LAST_FRAME.lock().unwrap() = frame.as_ref().map(|(h, f)| {
        let t = |r: &discovery::Rect| (r.x, r.y, r.w, r.h);
        (h.iter().map(t).collect(), f.iter().map(t).collect())
    });
    let mut bar = CHAT_BAR.lock().unwrap();
    *bar = None;
    if !framework_buttons_on() {
        return;
    }
    let Some((mut hitboxes, mut filled)) = frame else {
        return;
    };
    // SAFETY: the caller's contract.
    let Some(v) = (unsafe { client::viewport(window) }) else {
        return;
    };
    let own = OWN_BUTTONS.lock().unwrap().clone();
    let ours = |r: &discovery::Rect| {
        own.iter()
            .any(|o| (o.x - r.x).abs() < 1.0 && (o.y - r.y).abs() < 1.0 && (o.w - r.w).abs() < 1.0)
    };
    hitboxes.retain(|r| !ours(r));
    filled.retain(|r| !ours(r));
    let Some(found) = discovery::find_chat_bar(&hitboxes, &filled, v) else {
        return;
    };
    let Some(first) = found.buttons.first().copied() else {
        return;
    };
    let gap = found
        .buttons
        .get(1)
        .map_or(4.0, |b| (b.x - (first.x + first.w)).max(2.0));
    *bar = Some((first, gap));
    FOUND_FRAMES.fetch_add(1, Ordering::SeqCst);
}
