//! An inspector for any gpui app. In inspect mode, the element under the
//! pointer gets a border, and a panel next to the pointer lists what gpui
//! drew there: its bounds, fill, border, draw order, hitbox and text, and
//! every other element under the pointer, outermost first. A click selects
//! the element instead of reaching the app, and everything known about it
//! goes to the inspector's window (`inspect_link`). In normal mode the
//! mouse works as usual and only the selection stays outlined.
//!
//! It reads the frame gpui just drew (`rekindle_native::client::frame`), so
//! it sees what was painted, not the element tree. An element that paints
//! nothing and takes no input (a plain layout div) isn't in the frame, and
//! isn't listed.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rekindle_native::client::FrameRecord;
use rekindle_native::{Dir, Edges, Len, Node, Rgba, Style};

type Rect = (f32, f32, f32, f32);

static ON: AtomicBool = AtomicBool::new(false);
/// Inspect mode: hover shows, clicks select.
static INSPECTING: AtomicBool = AtomicBool::new(false);
static PICK: Mutex<Option<Pick>> = Mutex::new(None);
/// Where a click landed in inspect mode, for the next frame to select at.
static CLICK: Mutex<Option<(f32, f32)>> = Mutex::new(None);
static SELECTED: Mutex<Option<(Pick, serde_json::Value)>> = Mutex::new(None);
static SWALLOWED: AtomicUsize = AtomicUsize::new(0);

/// The border's colour. The inspector draws nothing else in it and the
/// client is unlikely to, so a quad bordered in it is the inspector's own.
const BORDER: u32 = 0xff3ea5fe;
/// The fill inside the border. gpui paints a div's border as a quad of
/// its own, after the children, so this one is told apart by its fill.
const TINT: u32 = 0xff3ea51c;
/// The panel's background, unique for the same reason: whatever is inside
/// a quad filled with it is the panel.
const PANEL: u32 = 0x0e0f11fd;
/// The panel's background, for checks.
pub(crate) const PANEL_RGBA: u32 = PANEL;
/// The selection's outline, in its own colour for the same reason.
const SELECT: u32 = 0x3ea5fffe;

/// The border's colour (HSLA), for checks.
pub(crate) fn hex_border() -> [f32; 4] {
    hsla(BORDER)
}
const PANEL_WIDTH: f32 = 340.0;
const LINE: f32 = 16.0;
/// Elements listed under the pointer, innermost last.
const STACK: usize = 10;

pub(crate) fn enable(on: bool) {
    ON.store(on, Ordering::SeqCst);
    if !on {
        *PICK.lock().unwrap() = None;
    }
}

pub fn enabled() -> bool {
    ON.load(Ordering::SeqCst)
}

/// `REKINDLE_INSPECT=1` turns it on, in inspect mode, and opens the link
/// to its window.
pub(crate) fn enable_from_env() {
    if std::env::var("REKINDLE_INSPECT").is_ok_and(|v| v == "1") {
        enable(true);
        INSPECTING.store(true, Ordering::SeqCst);
        crate::inspect_link::start();
    }
}

/// Inspect mode (clicks select) or normal mode (the mouse works as usual).
pub(crate) fn set_inspecting(on: bool) {
    INSPECTING.store(on, Ordering::SeqCst);
    if !on {
        *PICK.lock().unwrap() = None;
    }
    crate::inspect_link::broadcast(&serde_json::json!({ "type": "mode", "inspect": on }));
}

pub(crate) fn inspecting() -> bool {
    enabled() && INSPECTING.load(Ordering::SeqCst)
}

/// A click in inspect mode: the next frame selects what's under it.
pub(crate) fn clicked(x: f32, y: f32) {
    *CLICK.lock().unwrap() = Some((x, y));
}

/// A press or release the inspector kept from the app.
pub(crate) fn swallowed() {
    SWALLOWED.fetch_add(1, Ordering::SeqCst);
}

/// Presses and releases kept from the app so far.
pub(crate) fn swallowed_count() -> usize {
    SWALLOWED.load(Ordering::SeqCst)
}

pub(crate) fn clear_selection() {
    *SELECTED.lock().unwrap() = None;
    crate::inspect_link::broadcast(&serde_json::json!({ "type": "selected", "detail": null }));
}

/// The selected element.
pub(crate) fn selected() -> Option<Pick> {
    SELECTED.lock().unwrap().as_ref().map(|(p, _)| p.clone())
}

/// What the window is told when it connects.
pub(crate) fn hello() -> serde_json::Value {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let detail = SELECTED.lock().unwrap().as_ref().map(|(_, d)| d.clone());
    serde_json::json!({
        "type": "hello",
        "pid": std::process::id(),
        "exe": exe,
        "inspect": inspecting(),
        "selected": detail,
    })
}

/// What the inspector picked from the last frame.
pub(crate) fn last_pick() -> Option<Pick> {
    PICK.lock().unwrap().clone()
}

/// One painted element: a quad, a hitbox, or both with the same bounds.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Item {
    pub(crate) rect: Rect,
    pub(crate) quad: bool,
    pub(crate) hitbox: bool,
    pub(crate) fill: Option<[f32; 4]>,
    pub(crate) border: Option<[f32; 4]>,
    /// gpui's draw order (quads only).
    pub(crate) order: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Pick {
    pub(crate) pointer: (f32, f32),
    /// The innermost element under the pointer.
    pub(crate) target: Item,
    /// Every element under the pointer, outermost first; `target` is last.
    pub(crate) stack: Vec<Item>,
    /// Glyphs inside the target, and the colour most of them are.
    pub(crate) glyphs: usize,
    pub(crate) text_color: Option<[f32; 4]>,
    pub(crate) root_view: String,
}

fn same(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 2e-3)
}

fn hsla(rgba: u32) -> [f32; 4] {
    Rgba::hex(rgba).to_hsla()
}

fn same_rect(a: Rect, b: Rect) -> bool {
    (a.0 - b.0).abs() < 0.5
        && (a.1 - b.1).abs() < 0.5
        && (a.2 - b.2).abs() < 0.5
        && (a.3 - b.3).abs() < 0.5
}

fn under(r: Rect, p: (f32, f32)) -> bool {
    p.0 >= r.0 && p.1 >= r.1 && p.0 < r.0 + r.2 && p.1 < r.1 + r.3
}

fn inside(outer: Rect, inner: Rect) -> bool {
    inner.0 >= outer.0 - 0.5
        && inner.1 >= outer.1 - 0.5
        && inner.0 + inner.2 <= outer.0 + outer.2 + 0.5
        && inner.1 + inner.3 <= outer.1 + outer.3 + 0.5
}

/// What's under `pointer` in the frame `f`, leaving out the inspector's own
/// border and panel.
pub(crate) fn pick(f: &FrameRecord, pointer: (f32, f32), root_view: String) -> Option<Pick> {
    let (border, tint, panel, select) = (hsla(BORDER), hsla(TINT), hsla(PANEL), hsla(SELECT));
    let panels: Vec<Rect> = f
        .quads
        .iter()
        .filter(|(_, c)| c.is_some_and(|c| same(c, panel)))
        .map(|(r, _)| *r)
        .collect();
    let ours = |r: Rect| panels.iter().any(|p| inside(*p, r));
    let mut items: Vec<Item> = Vec::new();
    for (i, (r, fill)) in f.quads.iter().enumerate() {
        let b = f.quad_borders.get(i).copied();
        let own = b.is_some_and(|b| same(b, border) || same(b, select))
            || fill.is_some_and(|c| same(c, tint));
        if r.2 <= 0.0 || r.3 <= 0.0 || !under(*r, pointer) || ours(*r) || own {
            continue;
        }
        items.push(Item {
            rect: *r,
            quad: true,
            hitbox: false,
            fill: fill.filter(|c| c[3] > 0.0),
            border: b.filter(|c| c[3] > 0.0),
            order: f.quad_orders.get(i).copied(),
        });
    }
    for r in &f.hitboxes {
        if r.2 <= 0.0 || r.3 <= 0.0 || !under(*r, pointer) || ours(*r) {
            continue;
        }
        match items.iter_mut().find(|i| same_rect(i.rect, *r)) {
            Some(i) => i.hitbox = true,
            None => items.push(Item {
                rect: *r,
                quad: false,
                hitbox: true,
                fill: None,
                border: None,
                order: None,
            }),
        }
    }
    // Outermost first: by area, then by draw order (what's drawn later is on top).
    items.sort_by(|a, b| {
        let area = |i: &Item| i.rect.2 * i.rect.3;
        area(b)
            .total_cmp(&area(a))
            .then(a.order.unwrap_or(0).cmp(&b.order.unwrap_or(0)))
    });
    // Two quads with the same bounds (a fill, then a border over it) are one
    // element to the reader: keep the later, with what both say.
    let mut stack: Vec<Item> = Vec::new();
    for i in items {
        match stack.last_mut() {
            Some(last) if same_rect(last.rect, i.rect) => {
                last.hitbox |= i.hitbox;
                last.quad |= i.quad;
                last.fill = i.fill.or(last.fill);
                last.border = i.border.or(last.border);
                last.order = i.order.or(last.order);
            }
            _ => stack.push(i),
        }
    }
    let target = stack.last()?.clone();
    let mut colours: Vec<([f32; 4], usize)> = Vec::new();
    let mut glyphs = 0;
    for ((g, _), c) in f.glyphs.iter().zip(&f.glyph_colors) {
        if inside(target.rect, *g) && !ours(*g) {
            glyphs += 1;
            match colours.iter_mut().find(|(k, _)| same(*k, *c)) {
                Some((_, n)) => *n += 1,
                None => colours.push((*c, 1)),
            }
        }
    }
    let text_color = colours.iter().max_by_key(|(_, n)| *n).map(|(c, _)| *c);
    let skip = stack.len().saturating_sub(STACK);
    stack.drain(..skip);
    Some(Pick {
        pointer,
        target,
        stack,
        glyphs,
        text_color,
        root_view,
    })
}

/// gpui's HSLA (all 0 to 1) as `#rrggbb`, or `#rrggbbaa` when not opaque.
pub(crate) fn hex(c: [f32; 4]) -> String {
    let [h, s, l, a] = c;
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let channel = |t: f32| {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    let (r, g, b) = if s == 0.0 {
        let v = (l.clamp(0.0, 1.0) * 255.0).round() as u8;
        (v, v, v)
    } else {
        (channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0))
    };
    let a = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

fn size(r: Rect) -> String {
    format!("{}×{}", r.2.round(), r.3.round())
}

/// The panel's text, one string per line.
pub(crate) fn lines(p: &Pick) -> Vec<String> {
    let t = &p.target;
    let mut out = Vec::new();
    if !p.root_view.is_empty() {
        out.push(format!("root view  {}", p.root_view));
    }
    let kind = match (t.quad, t.hitbox) {
        (true, true) => "quad, hitbox",
        (true, false) => "quad",
        _ => "hitbox",
    };
    out.push(format!(
        "{kind}  {} at {}, {}",
        size(t.rect),
        t.rect.0.round(),
        t.rect.1.round()
    ));
    let mut paint = Vec::new();
    if let Some(c) = t.fill {
        paint.push(format!("fill {}", hex(c)));
    }
    if let Some(c) = t.border {
        paint.push(format!("border {}", hex(c)));
    }
    if let Some(o) = t.order {
        paint.push(format!("order {o}"));
    }
    if !paint.is_empty() {
        out.push(paint.join("  "));
    }
    if p.glyphs > 0 {
        let colour = p
            .text_color
            .map(|c| format!(" in {}", hex(c)))
            .unwrap_or_default();
        let s = if p.glyphs == 1 { "" } else { "s" };
        out.push(format!("text  {} glyph{s}{colour}", p.glyphs));
    }
    out.push(format!(
        "pointer  {}, {}",
        p.pointer.0.round(),
        p.pointer.1.round()
    ));
    if p.stack.len() > 1 {
        out.push("under the pointer:".into());
        for (depth, i) in p.stack.iter().enumerate() {
            let what = match (i.fill, i.hitbox) {
                (Some(c), true) => format!("{} hitbox", hex(c)),
                (Some(c), false) => hex(c),
                (None, true) => "hitbox".into(),
                (None, false) => "quad".into(),
            };
            out.push(format!(
                "{}{} {what}",
                "  ".repeat(depth.min(6)),
                size(i.rect)
            ));
        }
    }
    out
}

/// After the client drew a frame: pick what's under the pointer.
///
/// # Safety
///
/// `window` is the client's gpui `Window`, right after it drew, on its UI
/// thread.
pub(crate) unsafe fn observe(window: *const u8) {
    use rekindle_native::client;
    if !enabled() {
        return;
    }
    let click = CLICK.lock().unwrap().take();
    if !inspecting() && click.is_none() {
        return;
    }
    // SAFETY: the caller's contract, for each read of the window below.
    let (frame, root, viewport, scale, pointer) = unsafe {
        (
            client::frame(window),
            client::root_view_type(window).unwrap_or_default(),
            client::viewport(window),
            client::scale_factor(window),
            client::mouse_position(window),
        )
    };
    let Some(f) = frame else {
        return;
    };
    if let Some(at) = click
        && let Some(p) = pick(&f, at, root.clone())
    {
        let mut d = detail(&f, &p);
        d["viewport"] = json_pair(viewport);
        d["scale"] = scale.map(|k| serde_json::json!(k)).unwrap_or_default();
        crate::inspect_link::broadcast(
            &serde_json::json!({ "type": "selected", "detail": d.clone() }),
        );
        *SELECTED.lock().unwrap() = Some((p, d));
    }
    if !inspecting() {
        return;
    }
    let picked = pointer.and_then(|p| pick(&f, p, root));
    let mut last = PICK.lock().unwrap();
    let moved = last.as_ref().map(|l| l.target.rect) != picked.as_ref().map(|p| p.target.rect);
    if moved && let Some(p) = &picked {
        crate::inspect_link::broadcast(
            &serde_json::json!({ "type": "hover", "lines": lines(p), "target": item_json(&p.target) }),
        );
    }
    *last = picked;
}
fn json_pair(v: Option<(f32, f32)>) -> serde_json::Value {
    v.map(|(a, b)| serde_json::json!([a, b]))
        .unwrap_or_default()
}

fn item_json(i: &Item) -> serde_json::Value {
    serde_json::json!({
        "bounds": [i.rect.0, i.rect.1, i.rect.2, i.rect.3],
        "quad": i.quad,
        "hitbox": i.hitbox,
        "fill": i.fill.map(hex),
        "fill_hsla": i.fill,
        "border": i.border.map(hex),
        "border_hsla": i.border,
        "order": i.order,
    })
}

/// Everything the frame says about the picked element: itself, what it's
/// inside, and what's inside it (quads, hitboxes, text by line and colour).
pub(crate) fn detail(f: &FrameRecord, p: &Pick) -> serde_json::Value {
    let t = p.target.rect;
    let (border, tint, panel, select) = (hsla(BORDER), hsla(TINT), hsla(PANEL), hsla(SELECT));
    let panels: Vec<Rect> = f
        .quads
        .iter()
        .filter(|(_, c)| c.is_some_and(|c| same(c, panel)))
        .map(|(r, _)| *r)
        .collect();
    let ours = |r: Rect| panels.iter().any(|q| inside(*q, r));
    let mut quads: Vec<Item> = f
        .quads
        .iter()
        .enumerate()
        .filter(|(i, (r, fill))| {
            let b = f.quad_borders.get(*i).copied();
            let own = b.is_some_and(|b| same(b, border) || same(b, select))
                || fill.is_some_and(|c| same(c, tint));
            inside(t, *r) && !same_rect(*r, t) && !ours(*r) && !own
        })
        .map(|(i, (r, fill))| Item {
            rect: *r,
            quad: true,
            hitbox: f.hitboxes.iter().any(|h| same_rect(*h, *r)),
            fill: fill.filter(|c| c[3] > 0.0),
            border: f.quad_borders.get(i).copied().filter(|c| c[3] > 0.0),
            order: f.quad_orders.get(i).copied(),
        })
        .collect();
    quads.sort_by_key(|i| i.order.unwrap_or(0));
    let hitboxes: Vec<serde_json::Value> = f
        .hitboxes
        .iter()
        .filter(|h| {
            inside(t, **h)
                && !same_rect(**h, t)
                && !ours(**h)
                && !quads.iter().any(|q| same_rect(q.rect, **h))
        })
        .map(|h| serde_json::json!([h.0, h.1, h.2, h.3]))
        .collect();
    // Text: the glyphs inside, by colour, and grouped into lines by their
    // vertical extent.
    let mut colours: Vec<([f32; 4], usize)> = Vec::new();
    let mut text_lines: Vec<Rect> = Vec::new();
    for ((g, _), c) in f.glyphs.iter().zip(&f.glyph_colors) {
        if !inside(t, *g) || ours(*g) {
            continue;
        }
        match colours.iter_mut().find(|(k, _)| same(*k, *c)) {
            Some((_, n)) => *n += 1,
            None => colours.push((*c, 1)),
        }
        let mid = g.1 + g.3 / 2.0;
        match text_lines
            .iter_mut()
            .find(|l| mid >= l.1 && mid <= l.1 + l.3)
        {
            Some(l) => {
                let (x0, y0) = (l.0.min(g.0), l.1.min(g.1));
                let (x1, y1) = ((l.0 + l.2).max(g.0 + g.2), (l.1 + l.3).max(g.1 + g.3));
                *l = (x0, y0, x1 - x0, y1 - y0);
            }
            None => text_lines.push(*g),
        }
    }
    text_lines.sort_by(|a, b| a.1.total_cmp(&b.1));
    let ancestors: Vec<serde_json::Value> = p.stack[..p.stack.len().saturating_sub(1)]
        .iter()
        .map(item_json)
        .collect();
    serde_json::json!({
        "pointer": [p.pointer.0, p.pointer.1],
        "root_view": p.root_view,
        "target": item_json(&p.target),
        "ancestors": ancestors,
        "quads": quads.iter().map(item_json).collect::<Vec<_>>(),
        "hitboxes": hitboxes,
        "text": {
            "glyphs": p.glyphs,
            "colors": colours.iter().map(|(c, n)| serde_json::json!({ "color": hex(*c), "glyphs": n })).collect::<Vec<_>>(),
            "lines": text_lines.iter().map(|l| serde_json::json!([l.0, l.1, l.2, l.3])).collect::<Vec<_>>(),
        },
        "summary": lines(p),
    })
}

/// The border and panel, as elements for rekindle's layer. Plain divs with
/// no listeners, so gpui gives them no hitbox and the pointer goes through
/// to the client.
pub(crate) fn nodes(viewport: Option<(f32, f32)>) -> Vec<Node> {
    if !enabled() {
        return Vec::new();
    }
    let at = |x: f32, y: f32| Edges {
        top: Some(Len::Px(y)),
        right: None,
        bottom: None,
        left: Some(Len::Px(x)),
    };
    let mut out = Vec::new();
    if let Some(s) = selected() {
        let r = s.target.rect;
        out.push(Node::div(
            Style {
                absolute: true,
                inset: at(r.0 - 1.0, r.1 - 1.0),
                width: Some(Len::Px(r.2 + 2.0)),
                height: Some(Len::Px(r.3 + 2.0)),
                border: Some((2.0, Rgba::hex(SELECT))),
                ..Default::default()
            },
            vec![],
        ));
    }
    if !inspecting() {
        return out;
    }
    let Some(p) = last_pick() else { return out };
    let r = p.target.rect;
    let border = Node::div(
        Style {
            absolute: true,
            inset: at(r.0, r.1),
            width: Some(Len::Px(r.2)),
            height: Some(Len::Px(r.3)),
            background: Some(Rgba::hex(TINT)),
            border: Some((1.0, Rgba::hex(BORDER))),
            ..Default::default()
        },
        vec![],
    );
    let text = lines(&p);
    // Beside the pointer, on whichever side has room.
    let (vw, vh) = viewport.unwrap_or((f32::MAX, f32::MAX));
    let height = text.len() as f32 * LINE + 16.0;
    let (px, py) = p.pointer;
    let x = if px + 16.0 + PANEL_WIDTH <= vw {
        px + 16.0
    } else {
        (px - 16.0 - PANEL_WIDTH).max(0.0)
    };
    let y = if py + 16.0 + height <= vh {
        py + 16.0
    } else {
        (py - 16.0 - height).max(0.0)
    };
    let panel = Node::div(
        Style {
            absolute: true,
            inset: at(x, y),
            width: Some(Len::Px(PANEL_WIDTH)),
            flex: Some(Dir::Column),
            padding: Edges::all(Some(8.0)),
            background: Some(Rgba::hex(PANEL)),
            radius: Some(4.0),
            text_color: Some(Rgba::hex(0xe8eaedff)),
            text_size: Some(12.0),
            line_height: Some(LINE),
            ..Default::default()
        },
        text.into_iter().map(Node::text).collect(),
    );
    out.extend([border, panel]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> FrameRecord {
        let fill = |rgba: u32| Some(hsla(rgba));
        FrameRecord {
            images: Vec::new(),
            image_sizes: Vec::new(),
            hitboxes: vec![(100.0, 100.0, 40.0, 20.0)],
            quads: vec![
                ((0.0, 0.0, 800.0, 600.0), fill(0x313338ff)),
                ((80.0, 90.0, 200.0, 60.0), fill(0x2b2d31ff)),
                ((100.0, 100.0, 40.0, 20.0), fill(0x5865f2ff)),
                // The inspector's own tint, border and panel.
                ((100.0, 100.0, 40.0, 20.0), fill(TINT)),
                ((100.0, 100.0, 40.0, 20.0), None),
                ((110.0, 110.0, 340.0, 100.0), fill(PANEL)),
                ((118.0, 118.0, 30.0, 10.0), fill(0xffffffff)),
            ],
            quad_orders: vec![1, 2, 3, 9, 9, 10, 11],
            quad_borders: vec![
                [0.0; 4],
                [0.0; 4],
                [0.0; 4],
                [0.0; 4],
                hsla(BORDER),
                [0.0; 4],
                [0.0; 4],
            ],
            glyphs: vec![
                ((104.0, 104.0, 6.0, 10.0), 4),
                ((111.0, 104.0, 6.0, 10.0), 4),
            ],
            glyph_colors: vec![hsla(0xffffffff), hsla(0xffffffff)],
        }
    }

    #[test]
    fn picks_the_innermost_element_and_leaves_its_own_out() {
        let p = pick(&frame(), (120.0, 115.0), "app::Root".into()).unwrap();
        assert_eq!(p.target.rect, (100.0, 100.0, 40.0, 20.0));
        assert!(p.target.quad && p.target.hitbox);
        assert_eq!(p.target.fill.map(hex).as_deref(), Some("#5865f2"));
        assert_eq!(p.stack.len(), 3, "{:?}", p.stack);
        assert_eq!(
            (p.glyphs, p.text_color.map(hex).as_deref()),
            (2, Some("#ffffff"))
        );
        let text = lines(&p);
        assert_eq!(text[1], "quad, hitbox  40×20 at 100, 100");
        assert!(
            text.iter().any(|l| l == "    40×20 #5865f2 hitbox"),
            "{text:?}"
        );
    }

    #[test]
    fn nothing_under_the_pointer_is_no_pick() {
        assert!(pick(&frame(), (900.0, 900.0), String::new()).is_none());
    }

    #[test]
    fn hex_round_trips() {
        for v in [0x313338ffu32, 0x5865f2ff, 0xffffff80, 0x000000ff] {
            let want = if v & 0xff == 0xff {
                format!("#{:06x}", v >> 8)
            } else {
                format!("#{v:08x}")
            };
            assert_eq!(hex(hsla(v)), want);
        }
    }
}
