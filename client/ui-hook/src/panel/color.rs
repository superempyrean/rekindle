//! The colour picker element: a swatch, a hex field, a
//! saturation/value square and a hue bar.

use std::sync::Mutex;

use rekindle_native::{Ctx, Cursor, Dir, Edges, Len, Node, Rgba, Style, client};

use super::field::text_field_with;
use super::{FIELD_BORDER, UiInput, column, muted, report_ui, row};
use crate::picker;

/// The first colour picker's colour as hex, and whether the plugin has
/// shown that colour back.
pub(crate) fn picked() -> Option<(String, bool)> {
    let all = PICKED.lock().unwrap();
    let p = all.values().next()?;
    let hex = p.hex();
    let settled = p.sent.is_empty() && p.shown == hex;
    Some((hex, settled))
}

// ---- colour picker ----

/// One colour picker's state. HSV is kept here, not derived from the
/// plugin's colour each frame, so the hue survives a trip through grey
/// or black.
struct Picked {
    /// The last colour the plugin showed, as hex.
    shown: String,
    hsv: picker::Hsv,
    alpha: u8,
    /// Colours reported to the plugin that it hasn't shown back yet.
    sent: Vec<String>,
}

impl Picked {
    fn of(hex: &str) -> Self {
        let (rgb, alpha) = picker::parse_hex(hex).unwrap_or(([0, 0, 0], 255));
        Picked {
            shown: hex.to_string(),
            hsv: picker::rgb_to_hsv(rgb),
            alpha,
            sent: Vec::new(),
        }
    }

    fn hex(&self) -> String {
        picker::to_hex(picker::hsv_to_rgb(self.hsv), self.alpha)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Part {
    Square,
    Hue,
}

/// The press that started in a picker and hasn't been released.
struct Drag {
    key: String,
    target: (String, String, String),
    part: Part,
    bounds: client::Rect,
    /// When a colour last went to the plugin: a drag reports at most
    /// every [`DRAG_REPORT`], and once more on release.
    reported: std::time::Instant,
}

const DRAG_REPORT: std::time::Duration = std::time::Duration::from_millis(40);

static PICKED: Mutex<std::collections::BTreeMap<String, Picked>> =
    Mutex::new(std::collections::BTreeMap::new());
static DRAG: Mutex<Option<Drag>> = Mutex::new(None);

/// Tell the plugin the picker's colour now.
fn report_colour(key: &str, target: &(String, String, String)) {
    let mut all = PICKED.lock().unwrap();
    let Some(p) = all.get_mut(key) else { return };
    let hex = p.hex();
    if p.sent.last() != Some(&hex) {
        p.sent.push(hex);
    }
    let rgba = picker::to_rgba(picker::hsv_to_rgb(p.hsv), p.alpha);
    drop(all);
    report_ui(&target.0, &target.1, &target.2, UiInput::Color(rgba));
}

/// The pointer is at `(x, y)` with a picker pressed: move its colour
/// there. `force` reports it to the plugin whatever the interval.
fn drag_to(x: f32, y: f32, force: bool) {
    let mut drag = DRAG.lock().unwrap();
    let Some(d) = drag.as_mut() else { return };
    if let Some(p) = PICKED.lock().unwrap().get_mut(&d.key) {
        match d.part {
            Part::Square => (p.hsv[1], p.hsv[2]) = picker::sv_at(d.bounds, x, y),
            Part::Hue => p.hsv[0] = picker::hue_at(d.bounds, x),
        }
    }
    if force || d.reported.elapsed() >= DRAG_REPORT {
        d.reported = std::time::Instant::now();
        let (key, target) = (d.key.clone(), d.target.clone());
        drop(drag);
        report_colour(&key, &target);
    }
}

/// A press on one of a picker's parts: take the colour under the pointer
/// and follow the pointer until the button is released.
fn press(key: &str, target: &(String, String, String), part: Part, ctx: &mut Ctx) {
    // SAFETY: a click handler runs with the live window, on the UI thread.
    let pointer = unsafe { client::mouse_position(ctx.window()) };
    let (Some(bounds), Some((x, y))) = (client::pressed_bounds(), pointer) else {
        return;
    };
    *DRAG.lock().unwrap() = Some(Drag {
        key: key.to_string(),
        target: target.clone(),
        part,
        bounds,
        reported: std::time::Instant::now(),
    });
    drag_to(x, y, true);
    ctx.refresh();
}

/// Pointer input while a picker is pressed (from the client's
/// `dispatch_event`): a move drags, a release ends the drag.
///
/// # Safety
///
/// `window` is the client's live `Window`, on its UI thread.
pub(crate) unsafe fn picker_input(window: *mut u8, input: &client::Input) {
    if DRAG.lock().unwrap().is_none() {
        return;
    }
    match *input {
        client::Input::Move(x, y) => drag_to(x, y, false),
        client::Input::Up { x, y, left: true } => {
            drag_to(x, y, true);
            *DRAG.lock().unwrap() = None;
        }
        _ => return,
    }
    // SAFETY: the caller's contract.
    unsafe { client::refresh(window) };
}

fn solid(rgb: [u8; 3], width: f32, height: f32) -> Node {
    Node::div(
        Style {
            width: Some(Len::Px(width)),
            height: Some(Len::Px(height)),
            background: Some(Rgba::hex(picker::to_rgba(rgb, 255))),
            ..Default::default()
        },
        vec![],
    )
}

/// A colour picker: a swatch and a hex field, a saturation/value square
/// and a hue bar. A press on the square or the bar picks the colour
/// under the pointer, and dragging keeps picking until release. Each
/// change is reported to the plugin as `Color`.
pub(super) fn color_picker(
    plugin: &str,
    panel: &str,
    id: &str,
    label: &str,
    hex: &str,
    ctx: &mut Ctx,
) -> Node {
    let key = format!("{plugin}/{panel}/{id}");
    let target = (plugin.to_string(), panel.to_string(), id.to_string());
    let (hsv, alpha, current) = {
        let mut all = PICKED.lock().unwrap();
        let p = all.entry(key.clone()).or_insert_with(|| Picked::of(hex));
        if p.shown != hex {
            match p.sent.iter().position(|s| s == hex) {
                // The plugin caught up to a colour picked here.
                Some(i) => {
                    p.sent.drain(..=i);
                    p.shown = hex.to_string();
                }
                // The plugin changed the colour by itself.
                None => *p = Picked::of(hex),
            }
        }
        (p.hsv, p.alpha, p.hex())
    };
    let rgb = picker::hsv_to_rgb(hsv);

    let swatch = Node::div(
        Style {
            width: Some(Len::Px(24.0)),
            height: Some(Len::Px(24.0)),
            radius: Some(4.0),
            border: Some((1.0, Rgba::hex(FIELD_BORDER))),
            background: Some(Rgba::hex(picker::to_rgba(rgb, alpha))),
            ..Default::default()
        },
        vec![],
    );
    let (k, t) = (key.clone(), target.clone());
    let field = text_field_with(
        &format!("{key}#hex"),
        "",
        &current,
        "#rrggbb",
        ctx,
        move |text| {
            let Some((rgb, alpha)) = picker::parse_hex(&text) else {
                return;
            };
            if let Some(p) = PICKED.lock().unwrap().get_mut(&k) {
                let [h, s, v] = picker::rgb_to_hsv(rgb);
                // A grey keeps the hue it had, and black its saturation too.
                p.hsv = [
                    if s == 0.0 { p.hsv[0] } else { h },
                    if v == 0.0 { p.hsv[1] } else { s },
                    v,
                ];
                p.alpha = alpha;
            }
            report_colour(&k, &t);
        },
    );

    let square = sv_square(&key, &target, hsv);
    let hue = hue_bar(&key, &target, hsv[0]);
    column(
        6.0,
        vec![
            muted(label.to_string()),
            row(8.0, vec![swatch, field]),
            square,
            hue,
        ],
    )
}

/// The saturation/value square at hue `hsv[0]`, with a ring where `hsv`
/// is. A press picks the colour under the pointer.
fn sv_square(key: &str, target: &(String, String, String), hsv: picker::Hsv) -> Node {
    let (cols, rows) = picker::SV_CELLS;
    let cell = (
        picker::SV_SIZE.0 / cols as f32,
        picker::SV_SIZE.1 / rows as f32,
    );
    let mut square: Vec<Node> = (0..rows)
        .map(|row| {
            Node::div(
                Style {
                    flex: Some(Dir::Row),
                    ..Default::default()
                },
                (0..cols)
                    .map(|col| solid(picker::sv_cell(hsv[0], col, row), cell.0, cell.1))
                    .collect(),
            )
        })
        .collect();
    // A ring where the colour is, dark inside light so it shows on any cell.
    let ring = |size: f32, colour: u32| {
        Node::div(
            Style {
                absolute: true,
                inset: Edges {
                    left: Some(Len::Px(hsv[1] * picker::SV_SIZE.0 - size / 2.0)),
                    top: Some(Len::Px((1.0 - hsv[2]) * picker::SV_SIZE.1 - size / 2.0)),
                    ..Default::default()
                },
                width: Some(Len::Px(size)),
                height: Some(Len::Px(size)),
                radius: Some(size / 2.0),
                border: Some((2.0, Rgba::hex(colour))),
                ..Default::default()
            },
            vec![],
        )
    };
    square.push(ring(14.0, 0xffffffff));
    square.push(ring(10.0, 0x000000ff));
    let (k, t) = (key.to_string(), target.clone());
    Node::div(
        Style {
            flex: Some(Dir::Column),
            width: Some(Len::Px(picker::SV_SIZE.0)),
            height: Some(Len::Px(picker::SV_SIZE.1)),
            cursor: Some(Cursor::Pointer),
            ..Default::default()
        },
        square,
    )
    .on_click_cx(move |ctx| press(&k, &t, Part::Square, ctx))
}

/// The hue bar, with a marker at `hue`. A press picks the hue under the
/// pointer.
fn hue_bar(key: &str, target: &(String, String, String), hue: f32) -> Node {
    let hue_cell = picker::HUE_SIZE.0 / picker::HUE_CELLS as f32;
    let mut cells: Vec<Node> = (0..picker::HUE_CELLS)
        .map(|col| solid(picker::hue_cell(col), hue_cell, picker::HUE_SIZE.1))
        .collect();
    cells.push(Node::div(
        Style {
            absolute: true,
            inset: Edges {
                left: Some(Len::Px(hue / 360.0 * picker::HUE_SIZE.0 - 2.0)),
                top: Some(Len::Px(-2.0)),
                ..Default::default()
            },
            width: Some(Len::Px(4.0)),
            height: Some(Len::Px(picker::HUE_SIZE.1 + 4.0)),
            radius: Some(2.0),
            background: Some(Rgba::hex(0xffffffff)),
            border: Some((1.0, Rgba::hex(0x000000ff))),
            ..Default::default()
        },
        vec![],
    ));
    let (k, t) = (key.to_string(), target.clone());
    Node::div(
        Style {
            flex: Some(Dir::Row),
            width: Some(Len::Px(picker::HUE_SIZE.0)),
            height: Some(Len::Px(picker::HUE_SIZE.1)),
            cursor: Some(Cursor::Pointer),
            ..Default::default()
        },
        cells,
    )
    .on_click_cx(move |ctx| press(&k, &t, Part::Hue, ctx))
}
