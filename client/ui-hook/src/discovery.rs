//! Locate a client surface from GPUI's frame output at runtime, instead of from
//! a stored client byte signature.
//!
//! The idea: GPUI's pipeline is the one stable thing across client rebuilds.
//! Every frame records each clickable element's bounds (its hitboxes) and the
//! scene's filled rectangles, including what GPUI replayed for cached views. So
//! we identify a surface by its *live geometry*, re-derived every frame, rather
//! than by a byte pattern of the client function that built it.
//!
//! This module is client-free: it names no client type and reads no
//! `layout.json`. The input is plain rectangles; the framework layer reads them
//! from GPUI's frame record after `draw_roots` (see the crate root).
//! [`find_chat_bar`] is the whole technique for one surface.

/// A rectangle in window pixels. Plain data so the rule is testable without
/// constructing GPUI values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) w: f32,
    pub(crate) h: f32,
}

impl Rect {
    pub(crate) fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    fn bottom(&self) -> f32 {
        self.y + self.h
    }

    fn right(&self) -> f32 {
        self.x + self.w
    }

    /// Is `inner` fully within `self` (small tolerance for sub-pixel rounding)?
    fn contains(&self, inner: &Rect) -> bool {
        const EPS: f32 = 0.5;
        inner.x >= self.x - EPS
            && inner.y >= self.y - EPS
            && inner.right() <= self.right() + EPS
            && inner.bottom() <= self.bottom() + EPS
    }
}

/// The chat-bar surface, as located purely from geometry.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ChatBarRegion {
    /// The composer container the bar sits in.
    pub(crate) container: Rect,
    /// The chat-bar action buttons, left to right.
    pub(crate) buttons: Vec<Rect>,
}

/// Identify the chat bar from a frame's hitboxes and filled quads.
///
/// `hitboxes` are the bounds every clickable element registered this frame;
/// `filled` are the scene's rectangles that actually paint a background;
/// `window` is `(width, height)`. Returns `None` when no composer is present.
///
/// The rule is semantic, not positional: it keys on the shape of the composer
/// row, so it follows a resized window or an added button with nothing stored:
///
/// 1. the composer is the bottom-most filled rect spanning most of the width;
/// 2. the clickable elements inside it are the row;
/// 3. the widest of those is the text field; drop it and the "+" attach button
///    to its left;
/// 4. what remains, to the right of the field and sharing a common height, is
///    the chat-bar buttons in x order.
pub(crate) fn find_chat_bar(
    hitboxes: &[Rect],
    filled: &[Rect],
    window: (f32, f32),
) -> Option<ChatBarRegion> {
    let (_win_w, win_h) = window;

    // Candidate composer containers: filled rects that are *row-like*: short
    // relative to the window and much wider than tall. This rejects the
    // full-window background and tall side panels without assuming the composer
    // spans the whole width (it doesn't, between the sidebars).
    let mut candidates: Vec<Rect> = filled
        .iter()
        .copied()
        .filter(|r| r.h <= win_h * 0.25 && r.w >= r.h * 3.0 && r.w >= 150.0)
        .collect();
    // Bottom-most first: the composer sits at the foot of the window.
    candidates.sort_by(|a, b| b.bottom().total_cmp(&a.bottom()));

    for container in candidates {
        if let Some(buttons) = buttons_in(&container, hitboxes) {
            return Some(ChatBarRegion { container, buttons });
        }
    }
    None
}

/// Given a candidate composer container, return its chat-bar buttons, or `None`
/// if it doesn't look like a composer row.
fn buttons_in(container: &Rect, hitboxes: &[Rect]) -> Option<Vec<Rect>> {
    let inside: Vec<Rect> = hitboxes
        .iter()
        .copied()
        .filter(|h| container.contains(h))
        .collect();
    if inside.len() < 2 {
        return None;
    }

    // The text field is the widest element, and it must fill most of the
    // container, because a composer is mostly one wide input. A row of similar-width
    // icons (e.g. the user-area bar in the sidebar foot) has no element that
    // dominates its container, and is rejected here.
    let field = inside.iter().copied().max_by(|a, b| a.w.total_cmp(&b.w))?;
    if field.w < container.w * 0.5 {
        return None;
    }

    // Buttons are to the right of the field (the "+" attach is to its left);
    // keep the uniform-height run and order them left to right.
    let mut buttons: Vec<Rect> = inside.into_iter().filter(|h| h.x > field.x).collect();
    if buttons.is_empty() {
        return None;
    }
    buttons = largest_equal_height(buttons);
    buttons.sort_by(|a, b| a.x.total_cmp(&b.x));
    Some(buttons)
}

/// Keep the largest subset of rects whose heights agree (to the nearest pixel).
/// A stray hitbox of a different height in the row is discarded.
fn largest_equal_height(rects: Vec<Rect>) -> Vec<Rect> {
    use std::collections::HashMap;
    let mut groups: HashMap<i32, Vec<Rect>> = HashMap::new();
    for r in rects {
        groups.entry(r.h.round() as i32).or_default().push(r);
    }
    groups
        .into_values()
        .max_by_key(std::vec::Vec::len)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    // A representative composer row in a 1200x800 window: container, "+" attach
    // (left), wide text field, then N uniform buttons on the right.
    fn layout(win_w: f32, n_buttons: usize) -> (Vec<Rect>, Vec<Rect>, (f32, f32)) {
        let win = (win_w, 800.0);
        let container = Rect::new(16.0, 740.0, win_w - 32.0, 44.0);
        let filled = vec![
            Rect::new(0.0, 0.0, win_w, 60.0), // a top bar, also filled but not bottom-most
            container,
        ];
        let mut hitboxes = vec![
            Rect::new(24.0, 748.0, 24.0, 24.0), // "+" attach, left of field
            Rect::new(56.0, 744.0, win_w - 240.0, 36.0), // text field, widest
        ];
        let first_btn = win_w - 32.0 - (n_buttons as f32) * 34.0;
        for i in 0..n_buttons {
            hitboxes.push(Rect::new(first_btn + i as f32 * 34.0, 748.0, 28.0, 28.0));
        }
        (hitboxes, filled, win)
    }

    #[test]
    fn finds_default_button_row() {
        let (h, f, w) = layout(1200.0, 3);
        let region = find_chat_bar(&h, &f, w).expect("chat bar found");
        assert_eq!(region.buttons.len(), 3);
        // left to right
        assert!(region.buttons.windows(2).all(|p| p[0].x < p[1].x));
        // the "+" and the text field are not counted as buttons
        assert!(region.buttons.iter().all(|b| (b.w - 28.0).abs() < 0.5));
    }

    #[test]
    fn follows_an_added_button() {
        let (h, f, w) = layout(1200.0, 4);
        let region = find_chat_bar(&h, &f, w).expect("chat bar found");
        assert_eq!(
            region.buttons.len(),
            4,
            "discovery follows a registered button, nothing stored"
        );
    }

    #[test]
    fn follows_a_resized_window() {
        let (h, f, w) = layout(760.0, 3);
        let region = find_chat_bar(&h, &f, w).expect("chat bar found");
        assert_eq!(region.buttons.len(), 3);
        assert!(region.container.w >= 760.0 * 0.5);
    }

    #[test]
    fn ignores_a_stray_height_in_the_row() {
        let (mut h, f, w) = layout(1200.0, 3);
        // a lone taller hitbox to the right of the field is not a chat-bar button
        h.push(Rect::new(1140.0, 744.0, 40.0, 40.0));
        let region = find_chat_bar(&h, &f, w).expect("chat bar found");
        assert_eq!(region.buttons.len(), 3);
    }

    #[test]
    fn no_composer_no_surface() {
        // only narrow/top filled rects: nothing spans the width near the bottom
        let filled = vec![Rect::new(0.0, 0.0, 200.0, 40.0)];
        let hitboxes = vec![Rect::new(10.0, 10.0, 20.0, 20.0)];
        assert!(find_chat_bar(&hitboxes, &filled, (1200.0, 800.0)).is_none());
    }
}
