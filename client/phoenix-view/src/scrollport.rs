//! A scroll box as Android's `RecyclerView` keeps it: the offset moves with
//! the rows on screen when rows before them come and go, so the content
//! the user is reading stays put.
//!
//! The guest keeps a chat's rows in a sliding window. Of the list's `count`
//! adapter positions only `first` on are nodes in the tree, about two
//! screens of them, and as the rows shown move it unmounts rows at one end
//! of the window and inserts rows at the other. Android binds a position
//! outside the window as an empty placeholder row as tall as the average
//! row (`p3.q`), and its `LinearLayoutManager` lays out from an anchor row
//! that it keeps in place across the adapter's inserts and removals.
//!
//! [`Port`] does the same. The positions outside the window are space as
//! tall as the rows average, and each frame the offset moves by as much as
//! the anchor row moved: a row shown on the last frame, found again by its
//! node. That happens between layout and prepaint, so the frame that lays
//! out the new rows already draws them in place. A list with `stickToEnd`
//! that stands at its end follows the end instead.
//!
//! The box is three elements: [`PortBox`] around the scroll node's own
//! element, [`RowProbe`] around each row, which keeps the row's layout id,
//! and [`PortContent`] around the content, which reads the rows' layout,
//! settles the offset and draws the content moved by it.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window, point, px,
};
use rekindle_wire::NodeId;

use crate::scroller::{ANIMATED_MS, Scroller};

pub type PortRef = Rc<RefCell<Port>>;

/// `PHOENIX_VIEW_SCROLL_LOG` set: each scroll frame's offset and each
/// anchoring shift go to the guest log.
pub fn logging() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PHOENIX_VIEW_SCROLL_LOG").is_some())
}

/// Android's height for a placeholder row when no row is laid out
/// (`p3.q`), in dp.
const PLACEHOLDER: f32 = 48.;

/// How close to its end, in dp, a list with `stickToEnd` counts as at the
/// end.
const AT_END: f32 = 2.;

/// A row as the last frame laid it out: its index among the node's
/// children, its node, and where it starts along the axis and its length,
/// in dp from the box's start with the content unscrolled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Laid {
    pub index: usize,
    pub node: NodeId,
    pub start: f32,
    pub len: f32,
}

/// Where a guest's `ScrollTo` puts its row, as `gq` reads `align`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Align {
    /// The row's start `offset` dp after the box's start, less the
    /// spec's `offset` (`scrollToPositionWithOffset`).
    Start,
    Center,
    End,
    /// The least scroll that shows the whole row, `offset` dp clear of the
    /// start and `end_offset` of the end.
    Visible {
        end_offset: f32,
        top_only: bool,
    },
}

/// A guest's `ScrollTo` (op 7) on a list: an adapter position and where
/// to put it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Request {
    pub index: i32,
    pub align: Align,
    pub offset: f32,
    pub animated: bool,
}

/// One scroll node's offset, motion and last layout.
#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools, reason = "independent flags")]
pub struct Port {
    /// The offset and its fling, eased scroll or stretch. `scroller.pos`
    /// is the offset; the prepaint applies it.
    pub scroller: Scroller,
    pub horizontal: bool,
    /// What the frame's render gave: `stickToEnd`, the box's padding
    /// before and after the content along the axis, the adapter position
    /// of the first row, the positions in all (if the node says), the
    /// height of a placeholder row, and the view's clock.
    pub stick: bool,
    pub pad: (f32, f32),
    pub first: i32,
    pub count: Option<i32>,
    pub epoch: i64,
    pub est: f32,
    pub now: f64,
    /// Gathered while the frame is laid out.
    box_id: Option<LayoutId>,
    probes: Vec<(usize, NodeId, LayoutId)>,
    /// From the last prepaint: the box's bounds as painted, the rows, the
    /// box's length along the axis and the furthest offset.
    pub bounds: Bounds<Pixels>,
    pub laid: Vec<Laid>,
    pub len: f32,
    pub max: f32,
    /// A list with `stickToEnd` stood at its end at the last frame.
    stuck: bool,
    /// The rows shown at the last frame, the preferred anchor first, with
    /// where each started.
    anchors: Vec<(NodeId, f32)>,
    /// The first position shown at the last frame, where it started, and
    /// the epoch its position counts in: the anchor when the guest
    /// replaced every row shown.
    anchor_at: Option<(i32, f32, i64)>,
    /// The guest's scrolls waiting for the next layout.
    pub requests: Vec<Request>,
    /// The scroll moved at the last frame: dragged, flung, eased or
    /// stretched. When it stops, the list gets `scrollSettle`.
    pub was_moving: bool,
    /// What the last prepaint moved the offset by to keep the anchor in
    /// place, for the log and the tests.
    pub shifted: f32,
}

impl Port {
    pub fn new(horizontal: bool, stick: bool) -> Port {
        Port {
            horizontal,
            stick,
            stuck: stick,
            est: PLACEHOLDER,
            ..Port::default()
        }
    }

    /// The height a placeholder row gets: the rows' average, as Android
    /// takes the average of the rows attached, or 48 dp.
    pub fn placeholder(&self) -> f32 {
        let (n, sum) = self
            .laid
            .iter()
            .filter(|l| l.len > 0.)
            .fold((0usize, 0.), |(n, s), l| (n + 1, s + l.len));
        if n == 0 { PLACEHOLDER } else { sum / n as f32 }
    }

    /// The positions outside the rows, before and after them: how many
    /// placeholders the content holds on each side.
    pub fn placeholders(&self, rows: usize) -> (i32, i32) {
        let before = self.first.max(0);
        let after = self.count.map_or(0, |c| (c - before - rows as i32).max(0));
        (before, after)
    }

    /// Where adapter position `p` starts and how long it is, from the last
    /// layout: a row's own extent, or a placeholder's.
    fn span(&self, p: i32) -> Option<(f32, f32)> {
        let i = p - self.first;
        if i >= 0
            && let Some(l) = self.laid.iter().find(|l| l.index as i32 == i)
        {
            return Some((l.start, l.len));
        }
        if p < self.first {
            return (p >= 0).then_some((self.pad.0 + p as f32 * self.est, self.est));
        }
        let last = self.laid.iter().max_by(|a, b| a.index.cmp(&b.index))?;
        let past = p - (self.first + last.index as i32) - 1;
        (past >= 0).then_some((last.start + last.len + past as f32 * self.est, self.est))
    }

    /// The adapter positions the last frame showed, at least partly: the
    /// first and one past the last, as `findFirstVisibleItemPosition` and
    /// `findLastVisibleItemPosition` give them, placeholders included.
    pub fn shown(&self) -> Option<(i32, i32)> {
        if self.len <= 0. {
            return None;
        }
        let pos = self.scroller.pos;
        let (lo, hi) = (pos, pos + self.len);
        let hit = |s: f32, l: f32| l > 0. && s < hi && s + l > lo;
        let mut first = None;
        let mut last = None;
        let mut see = |p: i32| {
            first = Some(first.map_or(p, |f: i32| f.min(p)));
            last = Some(last.map_or(p, |l: i32| l.max(p)));
        };
        for l in &self.laid {
            if hit(l.start, l.len) {
                see(self.first + l.index as i32);
            }
        }
        let est = self.est.max(1.);
        // Placeholders before the rows.
        if self.first > 0 {
            let a = ((lo - self.pad.0) / est).floor().max(0.) as i32;
            let b = (((hi - self.pad.0) / est).ceil() as i32 - 1).min(self.first - 1);
            for p in [a, b] {
                if (0..self.first).contains(&p) && self.span(p).is_some_and(|(s, l)| hit(s, l)) {
                    see(p);
                }
            }
        }
        // And after them.
        if let (Some(count), Some(last_row)) = (self.count, self.laid.iter().map(|l| l.index).max())
        {
            let next = self.first + last_row as i32 + 1;
            if next < count {
                let (s0, _) = self.span(next)?;
                let a = next + ((lo - s0) / est).floor().max(0.) as i32;
                let b = next + ((hi - s0) / est).ceil() as i32 - 1;
                for p in [a, b.min(count - 1)] {
                    if p >= next && p < count && self.span(p).is_some_and(|(s, l)| hit(s, l)) {
                        see(p);
                    }
                }
            }
        }
        Some((first?, last? + 1))
    }

    /// The offset that puts position `p` where `r` asks, from this frame's
    /// layout, or `None` if the position isn't there.
    fn target(&self, r: &Request) -> Option<f32> {
        let (s, l) = self.span(r.index)?;
        let pos = self.scroller.pos;
        let h = self.len;
        Some(match r.align {
            // `scrollToPositionWithOffset(index, -offset)`: the row's start
            // that far after the padding.
            Align::Start => s - self.pad.0 + r.offset,
            Align::Center => s - (h / 2. - l / 2. - r.offset),
            Align::End => s - (h - self.pad.1 - l),
            Align::Visible {
                end_offset,
                top_only,
            } => {
                let (top, bottom) = (s - pos, s + l - pos);
                let edge = if top_only { top } else { bottom };
                if top < r.offset || edge > h - end_offset {
                    if bottom - top >= h || top < 0. {
                        pos + top - r.offset
                    } else {
                        pos + bottom + end_offset - h
                    }
                } else {
                    pos
                }
            }
        })
    }

    /// The frame's layout is done: read the rows, keep the anchor in
    /// place (or the end, for a list stuck to it), apply the guest's
    /// scrolls, and give the content's offset from its laid-out place.
    fn settle(&mut self, content: Bounds<Pixels>, window: &mut Window) -> f32 {
        let along = |b: Bounds<Pixels>, h: bool| -> (f32, f32) {
            if h {
                (f32::from(b.left()), f32::from(b.size.width))
            } else {
                (f32::from(b.top()), f32::from(b.size.height))
            }
        };
        let h = self.horizontal;
        let Some(box_id) = self.box_id else { return 0. };
        let bounds = window.layout_bounds(box_id);
        let (box_start, box_len) = along(bounds, h);
        let (c_start, c_len) = along(content, h);
        let laid: Vec<Laid> = self
            .probes
            .iter()
            .map(|&(index, node, id)| {
                let (s, l) = along(window.layout_bounds(id), h);
                Laid {
                    index,
                    node,
                    start: s - box_start,
                    len: l,
                }
            })
            .collect();
        let max = ((c_start - box_start) + c_len + self.pad.1 - box_len).max(0.);
        let old_max = self.max;
        self.bounds = bounds;
        self.len = box_len;
        self.laid = laid;
        self.max = max;

        // A list at its end stays there while nothing moves it away;
        // otherwise the anchor row keeps its place on screen.
        let follow = self.stick
            && self.stuck
            && self.scroller.pos >= old_max - AT_END
            && !self.scroller.moving();
        self.shifted = 0.;
        let before = self.scroller.pos;
        if follow {
            self.scroller.pos = max;
        } else if let Some((node, d)) = self.anchors.iter().find_map(|(node, was)| {
            self.laid
                .iter()
                .find(|l| l.node == *node)
                .map(|l| (*node, l.start - was))
        }) {
            if d != 0. {
                self.shifted = d;
                self.scroller.shift(d);
            }
            if logging() && d.abs() > 0.01 {
                crate::guest::log(&format!(
                    "[scroll] shift {d:.1} by #{node}: pos {before:.1} -> {:.1}, max {old_max:.1} -> {max:.1}, first {}, rows {}, est {:.1}",
                    self.scroller.pos,
                    self.first,
                    self.laid.len(),
                    self.est
                ));
            }
        } else if let Some((p, was, epoch)) = self.anchor_at.filter(|a| a.2 == self.epoch)
            && let Some((start, _)) = self.span(p)
        {
            let d = start - was;
            if d != 0. {
                self.shifted = d;
                self.scroller.shift(d);
            }
            if logging() {
                crate::guest::log(&format!(
                    "[scroll] no row left of {:?}; shift {d:.1} by position {p} (epoch {epoch}): pos {before:.1} -> {:.1}, first {}, rows {}",
                    self.anchors.iter().map(|a| a.0).collect::<Vec<_>>(),
                    self.scroller.pos,
                    self.first,
                    self.laid.len()
                ));
            }
        }
        for r in std::mem::take(&mut self.requests) {
            match self.target(&r) {
                Some(t) => {
                    let t = t.clamp(0., max);
                    if r.animated {
                        self.scroller.animate_to(t, ANIMATED_MS, self.now);
                    } else {
                        self.scroller.jump(t);
                    }
                    // A scroll the guest asks for decides whether the list
                    // stands at its end.
                    self.stuck = self.stick && t >= max - AT_END;
                }
                None => crate::guest::log(&format!(
                    "[view] scrollTo position {} isn't laid out ({} rows from {})",
                    r.index,
                    self.laid.len(),
                    self.first
                )),
            }
        }
        self.scroller.pos = self.scroller.pos.clamp(0., max);
        let pos = self.scroller.pos;
        if self.stick {
            self.stuck = pos >= max - AT_END;
        }
        // The rows shown, the one to keep in place first: the last for a
        // list that grows at its end, as Android's `stackFromEnd` lays out
        // from the end, and the first otherwise.
        let mut shown: Vec<(NodeId, f32)> = self
            .laid
            .iter()
            .filter(|l| l.start < pos + box_len && l.start + l.len > pos)
            .map(|l| (l.node, l.start))
            .collect();
        if self.stick {
            shown.reverse();
        }
        self.anchors = shown;
        self.anchor_at = self
            .shown()
            .and_then(|(p, _)| Some((p, self.span(p)?.0, self.epoch)));
        -pos + self.scroller.stretch()
    }
}

/// The scroll node's own element: its box, which the content is laid out
/// and clipped in.
pub struct PortBox {
    port: PortRef,
    child: AnyElement,
}

impl PortBox {
    pub fn new(port: PortRef, child: impl IntoElement) -> PortBox {
        PortBox {
            port,
            child: child.into_any_element(),
        }
    }
}

impl IntoElement for PortBox {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for PortBox {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        self.port.borrow_mut().probes.clear();
        let id = self.child.request_layout(window, cx);
        self.port.borrow_mut().box_id = Some(id);
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        (): &mut (),
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

/// A row of a scroll: it keeps the row's layout id for [`PortContent`].
pub struct RowProbe {
    port: PortRef,
    index: usize,
    node: NodeId,
    child: AnyElement,
}

impl RowProbe {
    pub fn new(port: PortRef, index: usize, node: NodeId, child: AnyElement) -> RowProbe {
        RowProbe {
            port,
            index,
            node,
            child,
        }
    }
}

impl IntoElement for RowProbe {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for RowProbe {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let id = self.child.request_layout(window, cx);
        self.port
            .borrow_mut()
            .probes
            .push((self.index, self.node, id));
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        (): &mut (),
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

/// The content of a scroll: laid out where the box's flow puts it, and
/// drawn moved by the offset [`Port::settle`] decides once the frame is
/// laid out.
pub struct PortContent {
    port: PortRef,
    child: AnyElement,
}

impl PortContent {
    pub fn new(port: PortRef, child: impl IntoElement) -> PortContent {
        PortContent {
            port,
            child: child.into_any_element(),
        }
    }
}

impl IntoElement for PortContent {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for PortContent {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let (offset, horizontal) = {
            let mut port = self.port.borrow_mut();
            (port.settle(bounds, window), port.horizontal)
        };
        let shift = if horizontal {
            point(px(offset), px(0.))
        } else {
            point(px(0.), px(offset))
        };
        window.with_element_offset(shift, |window| self.child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        (): &mut (),
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(first: i32, heights: &[f32]) -> Port {
        let mut p = Port::new(false, true);
        p.first = first;
        p.count = Some(first + heights.len() as i32 + 5);
        p.est = 50.;
        let mut y = first as f32 * 50.;
        for (i, h) in heights.iter().enumerate() {
            p.laid.push(Laid {
                index: i,
                node: 100 + i as u32,
                start: y,
                len: *h,
            });
            y += h;
        }
        p.len = 300.;
        p
    }

    #[test]
    fn shown_counts_rows_and_placeholders() {
        let mut p = rows(10, &[100., 100., 100., 100.]);
        // Rows at 500..900: the offset 550 shows all four (positions 10
        // to 13), the first and last in part.
        p.scroller.pos = 550.;
        assert_eq!(p.shown(), Some((10, 14)));
        // At 350 the box shows placeholders 7, 8, 9 and rows 10, 11.
        p.scroller.pos = 350.;
        assert_eq!(p.shown(), Some((7, 12)));
        // Past the rows, the placeholders after them: 900.. is position
        // 14, and the box ends in position 18, the last.
        p.scroller.pos = 850.;
        assert_eq!(p.shown(), Some((13, 19)));
    }

    #[test]
    fn a_request_puts_a_row_at_the_start_less_the_offset() {
        let mut p = rows(0, &[100., 100., 100., 100.]);
        p.max = 100.;
        let r = Request {
            index: 2,
            align: Align::Start,
            offset: -38.,
            animated: false,
        };
        assert_eq!(p.target(&r), Some(162.));
        let v = Request {
            index: 3,
            align: Align::Visible {
                end_offset: 0.,
                top_only: false,
            },
            offset: 0.,
            animated: false,
        };
        // Row 3 ends at 400, 100 past the box: the least scroll is 100.
        assert_eq!(p.target(&v), Some(100.));
    }
}
