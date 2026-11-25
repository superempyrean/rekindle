//! The platform's UI side: the guest's turns applied to a retained tree, the
//! tree painted with GPUI, and the window's input sent back as UI events.
//!
//! Layout follows the Android platform's views: a `view` is a flex column
//! unless `direction` is `"row"`, a `stack` layers its children, a
//! `navigator` shows the child its `index` names. One dp is one GPUI pixel.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use futures::StreamExt;
use gpui::{
    AnyElement, App, Bounds, Context, DispatchPhase, Entity, FocusHandle, Focusable, FontWeight,
    Hsla, Image, ImageSource, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ObjectFit, Pixels, Rgba, ScrollDelta, ScrollEvent, SharedString, StyledImage,
    TextAlign, TouchPhase, Window, canvas, div, img, img as img_el, prelude::*, px, relative,
};
use rekindle_wire::event::{Event, Insets, UiEvent};
use rekindle_wire::{NodeId, PropId, Tree, Turn, Value};
use rekindle_wire_inject::{Engine, Verdict};

use crate::apk::Apk;
use crate::guest::{FromGuest, Guest};
use crate::input::{InputEvent, TextInput};
use crate::motion::{MotionHost, channel};
use crate::scrollport::{Align, Port, PortBox, PortContent, PortRef, Request, RowProbe};
use crate::theme::ThemeSource;

/// The `PropIds` the renderer reads, by name.
pub struct Ids {
    pub blur: PropId,
    pub shadow: PropId,
    pub press_ripple: PropId,
    pub scale_factor: PropId,
    pub first: PropId,
    pub count: PropId,
    pub epoch: PropId,
    pub stick_to_end: PropId,
    pub direction: PropId,
    pub width: PropId,
    pub height: PropId,
    pub align: PropId,
    pub justify: PropId,
    pub background: PropId,
    pub flex: PropId,
    pub padding: PropId,
    pub margin: PropId,
    pub gap: PropId,
    pub radius: PropId,
    pub border_color: PropId,
    pub border_width: PropId,
    pub opacity: PropId,
    pub clip: PropId,
    pub gravity: PropId,
    pub min_width: PropId,
    pub min_height: PropId,
    pub max_width: PropId,
    pub max_height: PropId,
    pub text: PropId,
    pub color: PropId,
    pub font_size: PropId,
    pub font_weight: PropId,
    pub font_family: PropId,
    pub line_height: PropId,
    pub text_transform: PropId,
    pub text_align: PropId,
    pub max_lines: PropId,
    pub uri: PropId,
    pub scale: PropId,
    pub index: PropId,
    pub on: PropId,
    pub value: PropId,
    pub placeholder: PropId,
    pub placeholder_color: PropId,
    pub secure: PropId,
    pub label: PropId,
    pub hidden: PropId,
    pub event_count: PropId,
    pub tint: PropId,
    pub absolute: PropId,
    pub flex_basis: PropId,
    pub radius_top_left: PropId,
    pub radius_top_right: PropId,
    pub radius_bottom_left: PropId,
    pub radius_bottom_right: PropId,
    pub clip_outline: PropId,
    pub border_sides: PropId,
    pub shrink: PropId,
    pub translate_x: PropId,
    pub translate_y: PropId,
    pub spans: PropId,
    pub background_gradient: PropId,
    pub background_gradient_axis: PropId,
}

pub fn ids() -> &'static Ids {
    static IDS: OnceLock<Ids> = OnceLock::new();
    IDS.get_or_init(|| {
        let p = |n: &str| PropId::from_name(n).unwrap_or_else(|| panic!("no PropId named {n}"));
        Ids {
            blur: p("blur"),
            shadow: p("shadow"),
            press_ripple: p("pressRipple"),
            scale_factor: p("scaleFactor"),
            first: p("first"),
            count: p("count"),
            epoch: p("epoch"),
            stick_to_end: p("stickToEnd"),
            direction: p("direction"),
            width: p("width"),
            height: p("height"),
            align: p("align"),
            justify: p("justify"),
            background: p("background"),
            flex: p("flex"),
            padding: p("padding"),
            margin: p("margin"),
            gap: p("gap"),
            radius: p("radius"),
            border_color: p("borderColor"),
            border_width: p("borderWidth"),
            opacity: p("opacity"),
            clip: p("clip"),
            gravity: p("gravity"),
            min_width: p("minWidth"),
            min_height: p("minHeight"),
            max_width: p("maxWidth"),
            max_height: p("maxHeight"),
            text: p("text"),
            color: p("color"),
            font_size: p("fontSize"),
            font_weight: p("fontWeight"),
            font_family: p("fontFamily"),
            line_height: p("lineHeight"),
            text_transform: p("textTransform"),
            text_align: p("textAlign"),
            max_lines: p("maxLines"),
            uri: p("uri"),
            scale: p("scale"),
            index: p("index"),
            on: p("on"),
            value: p("value"),
            placeholder: p("placeholder"),
            placeholder_color: p("placeholderColor"),
            secure: p("secure"),
            label: p("accessibilityLabel"),
            hidden: p("hidden"),
            event_count: p("eventCount"),
            tint: p("tint"),
            absolute: p("absolute"),
            flex_basis: p("flexBasis"),
            radius_top_left: p("radiusTopLeft"),
            radius_top_right: p("radiusTopRight"),
            radius_bottom_left: p("radiusBottomLeft"),
            radius_bottom_right: p("radiusBottomRight"),
            clip_outline: p("clipOutline"),
            border_sides: p("borderSides"),
            shrink: p("shrink"),
            translate_x: p("translateX"),
            translate_y: p("translateY"),
            spans: p("spans"),
            background_gradient: p("backgroundGradient"),
            background_gradient_axis: p("backgroundGradientAxis"),
        }
    })
}

/// The font families the APK's fonts registered as.
#[derive(Clone, Default)]
pub struct Fonts {
    pub sans: Option<SharedString>,
    /// gg mono, for `code` spans.
    pub mono: Option<SharedString>,
    pub ginto: Option<SharedString>,
}

/// How a node sits in its parent, which decides what `"fill"` means.
#[derive(Clone, Copy, PartialEq)]
enum Parent {
    Column,
    Row,
    /// A layer of a `stack`, a navigator's screen, or the root: fill is the
    /// whole box.
    Layer,
    /// The one child of a vertical `scroll`: a column item that is at least
    /// as tall as the scroll, as Android's `ScrollView` with
    /// `setFillViewport(true)` measures it.
    Viewport,
}

/// An image node's image and how it was found, or why it has none.
pub type Drawn = Result<(ImageSource, String), NoImage>;

/// Why an image node has no drawable.
pub struct NoImage {
    pub why: String,
    /// The APK set lacks the density split its base requires, which holds
    /// the density-specific drawables.
    pub apk_incomplete: bool,
}

pub type HitMap = Rc<RefCell<HashMap<NodeId, Bounds<Pixels>>>>;

/// What the self-test reads: the turns that arrived, and when the window
/// event went out.
#[derive(Default)]
pub struct Stats {
    pub turns: Vec<usize>,
    /// `turns.len()` when each window event was sent.
    pub window_events: Vec<usize>,
    /// The node events sent, as (node, event name).
    pub events: Vec<(NodeId, String)>,
    /// Cells the theme engine rewrote, counted over every turn.
    pub themed_turns: usize,
    /// Each frame's cost, filled in when its paint ends.
    pub frames: Rc<RefCell<Vec<FrameCost>>>,
}

/// What one frame took: when it began on the view's clock, the time in
/// `render` and from the start of `render` to the end of its paint, in
/// ms, and the nodes `render` built.
#[derive(Clone, Copy, Debug)]
pub struct FrameCost {
    pub at: f64,
    pub render: f64,
    pub total: f64,
    pub nodes: usize,
    /// A scroll moved in it: a drag, fling, eased scroll or stretch.
    pub scrolling: bool,
}

pub struct PhoenixView {
    /// The tree, shared with the frame being built so a node's cells are
    /// read where they are, not copied.
    pub tree: Rc<Tree>,
    pub guest: Option<Guest>,
    pub stopped: Option<String>,
    engine: Option<Engine>,
    theme: Option<ThemeSource>,
    apk: Option<Arc<Apk>>,
    fonts: Fonts,
    /// Each drawable as loaded, by uri, tint and box: the image and how it
    /// was found, or `None`.
    images: HashMap<String, Drawn>,
    /// What each image node last asked for: its `images` key.
    image_keys: HashMap<NodeId, String>,
    /// The `images` keys (uri and tint) of drawables with no size of their
    /// own, which are keyed by their box too.
    boxed: HashSet<String>,
    /// Each avatar `placeholder` drawable (`res://rn_default_avatar_*`, a
    /// coloured circle) as loaded, by uri and box: Android draws it behind
    /// the avatar while the image loads (`dd.K`).
    placeholders: HashMap<String, Option<ImageSource>>,
    /// `--live`: the remote image loader, the `images` keys it is still
    /// loading, and the size in pixels of each it loaded.
    loader: Option<crate::remote::Loader>,
    pending: HashSet<String>,
    loaded: HashMap<String, (u32, u32)>,
    /// The uri each image node was last sent `load` or `error` for.
    told: HashMap<NodeId, String>,
    /// Each row of a list that listens for `rowPress`: the list and the
    /// row's index.
    rows: HashMap<NodeId, (NodeId, i32)>,
    inputs: HashMap<NodeId, Entity<TextInput>>,
    /// Each field's `value` cell as the tree last held it.
    tree_values: HashMap<NodeId, String>,
    /// Each field's `changeText` count, its `eventCount`.
    edits: HashMap<NodeId, i64>,
    pub hits: HitMap,
    /// The radius a node's `clipOutline` gives each of its children,
    /// taken when the child is drawn.
    outlines: HashMap<NodeId, [Option<f32>; 4]>,
    /// Nodes their parent draws as its own border, drawn as nothing.
    absorbed: HashSet<NodeId>,
    /// The opaque backgrounds drawn so far this frame, in paint order,
    /// which a `clipOutline` hole shows through.
    painted: Vec<(NodeId, u32)>,
    /// The popovers met this frame, drawn after the whole tree as Android
    /// shows each in a window of its own, and whether one is being drawn.
    popovers: Vec<NodeId>,
    hoisting: bool,
    /// The kinds the log has named as not drawn.
    unhandled: HashSet<String>,
    /// Each scroll's offset, motion and last layout, the scrolls the last
    /// paint drew in paint order, the drag under way, and the clock they
    /// run on.
    ports: HashMap<NodeId, PortRef>,
    scroll_order: Rc<RefCell<Vec<NodeId>>>,
    drag: Option<Drag>,
    clock: std::time::Instant,
    sent_window: Option<(f32, f32, f32)>,
    /// The `frameInterval` last sent.
    sent_frame: Option<std::time::Duration>,
    /// The motion engine ticked for the frame being drawn, before `render`.
    motion_ticked: bool,
    /// `motion_clock` is running.
    motion_clock: bool,
    /// The nodes the last frame drew: not hidden, and not in a row laid
    /// out as space.
    drawn: HashSet<NodeId>,
    /// The guest's motion, run by the library's engine.
    pub motion: MotionHost,
    /// Renders left before the layout a turn made is painted and reported
    /// to the engine.
    settle: u8,
    /// Each node's translation as last drawn, which [`MotionHost`] takes
    /// out of the rects it reports, as Android's `getLeft` excludes it.
    drawn_translate: HashMap<NodeId, (f32, f32)>,
    /// Each navigator's screen as last drawn, and the transition running
    /// from it to the screen its `index` names now.
    nav_shown: HashMap<NodeId, NodeId>,
    nav_anims: HashMap<NodeId, crate::navanim::NavAnim>,
    /// Each list's last `range`, as the key the Android platform compares
    /// (`eq.b`), and whether its start was reached at the last check.
    ranges: HashMap<NodeId, String>,
    /// Each text node's laid-out text, which places its span images.
    text_layouts: HashMap<NodeId, gpui::TextLayout>,
    /// The size each node that listens for `layout` was last sent.
    layouts_sent: HashMap<NodeId, (f32, f32)>,
    /// The text field that has focus.
    focused: Option<NodeId>,
    text_links: HashMap<NodeId, Vec<(std::ops::Range<usize>, String)>>,
    start_reached: HashMap<NodeId, bool>,
    /// The nodes this frame's `render` has built.
    built: usize,
    /// Each node's parent, as of the last turn.
    parents: HashMap<NodeId, NodeId>,
    /// Nodes a turn, the motion engine or a loaded image changed since the
    /// last render, and the list rows built in full since anything in them
    /// last changed, which a frame may lay out as space of the same size
    /// while they are well off screen. `spaced` holds the rows laid out as
    /// space at the last frame.
    dirty: HashSet<NodeId>,
    clean_rows: HashSet<NodeId>,
    spaced: HashSet<NodeId>,
    /// `--capture`: each turn as the guest sent it.
    pub capture: Option<rekindle_wire::capture::Writer<std::fs::File>>,
    pub stats: Stats,
    focus: FocusHandle,
}

/// A node's element, with the bounds it is laid out at kept in the hit
/// map at prepaint. It adds no box to the layout.
struct Recorded {
    id: NodeId,
    hits: HitMap,
    inner: AnyElement,
}

impl IntoElement for Recorded {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for Recorded {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, ()) {
        (self.inner.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.hits.borrow_mut().insert(self.id, bounds);
        self.inner.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        (): &mut (),
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.paint(window, cx);
    }
}

/// `PHOENIX_VIEW_FRAME_LOG` set: each frame, and what asked for the next,
/// go to the guest log.
pub fn frame_logging() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PHOENIX_VIEW_FRAME_LOG").is_some())
}

fn argb(c: u32) -> Hsla {
    Rgba {
        r: ((c >> 16) & 0xff) as f32 / 255.0,
        g: ((c >> 8) & 0xff) as f32 / 255.0,
        b: (c & 0xff) as f32 / 255.0,
        a: (c >> 24) as f32 / 255.0,
    }
    .into()
}

fn num(v: Option<&Value>) -> Option<f32> {
    match v? {
        Value::F64(f) => Some(*f as f32),
        v => v.as_int().map(|i| i as f32),
    }
}

impl PhoenixView {
    pub fn new(
        guest: anyhow::Result<(Guest, futures::channel::mpsc::UnboundedReceiver<FromGuest>)>,
        apk: Option<Arc<Apk>>,
        fonts: Fonts,
        theme: Option<ThemeSource>,
        cx: &mut Context<Self>,
    ) -> Self {
        let loader = guest.as_ref().ok().and_then(|(g, _)| g.net()).map(|net| {
            let (tx, mut rx) = futures::channel::mpsc::unbounded();
            cx.spawn(async move |this, cx| {
                while let Some(done) = rx.next().await {
                    if this.update(cx, |view, cx| view.on_image(done, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
            crate::remote::Loader::new(&net, &tx)
        });
        let (guest, stopped) = match guest {
            Ok((guest, mut rx)) => {
                cx.spawn(async move |this, cx| {
                    while let Some(msg) = rx.next().await {
                        if this.update(cx, |view, cx| view.on_guest(msg, cx)).is_err() {
                            break;
                        }
                    }
                })
                .detach();
                (Some(guest), None)
            }
            Err(e) => (None, Some(format!("{e:#}"))),
        };
        PhoenixView {
            tree: Rc::new(Tree::new()),
            guest,
            stopped,
            engine: None,
            theme,
            apk,
            fonts,
            images: HashMap::new(),
            image_keys: HashMap::new(),
            boxed: HashSet::new(),
            placeholders: HashMap::new(),
            loader,
            pending: HashSet::new(),
            loaded: HashMap::new(),
            told: HashMap::new(),
            rows: HashMap::new(),
            inputs: HashMap::new(),
            tree_values: HashMap::new(),
            edits: HashMap::new(),
            hits: HitMap::default(),
            painted: Vec::new(),
            popovers: Vec::new(),
            hoisting: false,
            outlines: HashMap::new(),
            absorbed: HashSet::new(),
            unhandled: HashSet::new(),
            ports: HashMap::new(),
            scroll_order: Rc::default(),
            drag: None,
            clock: std::time::Instant::now(),
            sent_window: None,
            sent_frame: None,
            motion_ticked: false,
            motion_clock: false,
            drawn: HashSet::new(),
            motion: MotionHost::new(),
            settle: 0,
            drawn_translate: HashMap::new(),
            nav_shown: HashMap::new(),
            nav_anims: HashMap::new(),
            ranges: HashMap::new(),
            text_layouts: HashMap::new(),
            layouts_sent: HashMap::new(),
            focused: None,
            text_links: HashMap::new(),
            start_reached: HashMap::new(),
            built: 0,
            parents: HashMap::new(),
            dirty: HashSet::new(),
            clean_rows: HashSet::new(),
            spaced: HashSet::new(),
            capture: None,
            stats: Stats::default(),
            focus: cx.focus_handle(),
        }
    }

    fn on_guest(&mut self, msg: FromGuest, cx: &mut Context<Self>) {
        match msg {
            FromGuest::Turn(bytes) => {
                // As emitted: the leading u32 0 and the turn.
                self.stats.turns.push(bytes.len() + 4);
                if let Some(w) = &mut self.capture {
                    let n = self.stats.turns.len() as u64 - 1;
                    if let Err(e) = w.record(n, &bytes) {
                        crate::guest::log(&format!("[view] capture stopped: {e}"));
                        self.capture = None;
                    }
                }
                let bytes = match self.engine.as_mut().map(|e| e.filter(&bytes)) {
                    Some(Ok(Some(rewritten))) => {
                        self.stats.themed_turns += 1;
                        rewritten
                    }
                    Some(Err(e)) => {
                        crate::guest::log(&format!("[view] theme engine refused a turn: {e:?}"));
                        bytes
                    }
                    _ => bytes,
                };
                match Turn::from_bytes(&bytes) {
                    Ok(turn) => {
                        Rc::make_mut(&mut self.tree).apply(&turn);
                        self.motion.feed(&turn);
                        self.touched(&turn);
                        self.settle = 2;
                        let tree = &self.tree;
                        self.ports.retain(|id, _| tree.node(*id).is_some());
                        self.scroll_requests(&turn);
                    }
                    Err(e) => crate::guest::log(&format!(
                        "[view] a turn of {} bytes didn't decode: {e:?}",
                        bytes.len()
                    )),
                }
                cx.notify();
            }
            FromGuest::Theme(palette) => {
                if let Some(theme) = &self.theme {
                    match theme.work(&palette) {
                        Ok(work) => {
                            crate::guest::log(&format!("[view] theme: {}", work.to_json()));
                            self.engine = Engine::new(work).ok();
                        }
                        Err(e) => crate::guest::log(&format!("[view] theme: {e:#}")),
                    }
                }
            }
            FromGuest::Stopped(why) => {
                self.stopped = Some(why);
                cx.notify();
            }
        }
    }

    /// Send a UI event, through the theme engine when one runs.
    fn send(&mut self, ev: &UiEvent) {
        let Some(guest) = &self.guest else { return };
        let bytes = ev.to_bytes();
        match self.engine.as_mut().map(|e| e.event(&bytes)) {
            Some(Ok(Verdict::Consume(_))) => {}
            Some(Ok(Verdict::Replace(ev))) => guest.dispatch(ev.to_bytes()),
            Some(Ok(Verdict::Batch(b))) => guest.dispatch(b),
            _ => guest.dispatch(bytes),
        }
    }

    fn listens(&self, node: NodeId, name: &str) -> bool {
        matches!(self.tree.props().get(node, ids().on), Some(Value::Array(a)) if a.iter().any(|v| matches!(v, Value::Str(s) if s == name)))
    }

    /// An event on a node, sent only if its `on` lists the event, as a
    /// platform raises only the events a node lists.
    pub fn node_event(&mut self, node: NodeId, name: &str, value: Value) -> bool {
        if !self.listens(node, name) {
            return false;
        }
        let Some(prop) = PropId::from_name(name) else {
            return false;
        };
        self.stats.events.push((node, name.to_string()));
        self.send(&UiEvent::Node(Event {
            node,
            name: prop,
            value,
        }));
        true
    }

    /// `changeText`'s value as Android's `TextWatcher` sends it: an
    /// object of `text` and `eventCount`, the field's edit count, in
    /// `PropId` order. The guest ignores a bare string.
    fn change_text(&mut self, node: NodeId, text: String) -> Value {
        let n = self.edits.entry(node).or_insert(0);
        *n += 1;
        let mut fields = vec![
            (ids().text, Value::Str(text)),
            (ids().event_count, Value::I32(*n as i32)),
        ];
        fields.sort_by_key(|(p, _)| p.0);
        Value::Object(fields)
    }

    /// `rowPress` on a list, as the Android platform's `RecyclerView` touch
    /// listener sends it: the row's adapter position (`index`) and the
    /// list's `epoch`.
    pub fn row_press(&mut self, list: NodeId, index: i32) -> bool {
        let epoch_id = PropId::from_name("epoch").unwrap();
        let epoch = num(self.tree.props().get(list, epoch_id)).unwrap_or(0.) as i64;
        let mut fields = vec![
            (ids().index, Value::I32(index)),
            (epoch_id, Value::I64(epoch)),
        ];
        fields.sort_by_key(|(p, _)| p.0);
        self.node_event(list, "rowPress", Value::Object(fields))
    }

    /// A long press on `id`, as the platform raises one: `longPress` on the
    /// innermost node at or above `id` that lists it, else `rowLongPress`
    /// on the list whose row holds `id`.
    pub fn long_press(&mut self, id: NodeId) -> bool {
        let size = |n: NodeId| {
            let mut count = 0;
            let mut stack = vec![n];
            while let Some(x) = stack.pop() {
                count += 1;
                if let Some(node) = self.tree.node(x) {
                    stack.extend(&node.children);
                }
            }
            count
        };
        let holder = self
            .shown()
            .into_iter()
            .filter(|n| self.listens(*n, "longPress") && self.holds(*n, id))
            .min_by_key(|n| size(*n));
        if let Some(n) = holder {
            return self.node_event(n, "longPress", Value::Null);
        }
        let row = self
            .rows
            .iter()
            .filter(|(row, (list, _))| self.listens(*list, "rowLongPress") && self.holds(**row, id))
            .map(|(_, at)| *at)
            .next();
        let Some((list, index)) = row else {
            return false;
        };
        let epoch_id = PropId::from_name("epoch").unwrap();
        let epoch = num(self.tree.props().get(list, epoch_id)).unwrap_or(0.) as i64;
        let mut fields = vec![
            (ids().index, Value::I32(index)),
            (epoch_id, Value::I64(epoch)),
        ];
        fields.sort_by_key(|(p, _)| p.0);
        self.node_event(list, "rowLongPress", Value::Object(fields))
    }

    /// Whether `id` is `node` or under it.
    fn holds(&self, node: NodeId, id: NodeId) -> bool {
        let mut stack = vec![node];
        while let Some(x) = stack.pop() {
            if x == id {
                return true;
            }
            if let Some(n) = self.tree.node(x) {
                stack.extend(&n.children);
            }
        }
        false
    }

    pub fn back(&mut self) {
        self.send(&UiEvent::Back);
    }

    /// Send `window` (and, the first time, `insets` and `frameInterval`)
    /// when the viewport differs from what the guest last got.
    /// The views a press on `id` sets pressed, as Android's
    /// `dispatchSetPressed` passes the state down: `id`, and each
    /// descendant with a `viewMotion` cell that doesn't take presses
    /// itself.
    fn pressed_views(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = vec![id];
        let mut stack: Vec<NodeId> = self
            .tree
            .node(id)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        while let Some(n) = stack.pop() {
            if self.listens(n, "press") {
                continue;
            }
            if self.motion.tracks(n) {
                out.push(n);
            }
            if let Some(node) = self.tree.node(n) {
                stack.extend(&node.children);
            }
        }
        out
    }

    /// The motion engine's frame: the layout of the last paint reported,
    /// one `tick` if a frame is due, the op 8 pairs sent to the guest, and
    /// another frame asked of GPUI while the engine writes to drawn nodes.
    /// GPUI's frames are the clock, as Android's `Choreographer` is.
    fn motion_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.motion.has_layout_work() {
            let parents = &self.parents;
            let hits = self.hits.borrow();
            let moved = &self.drawn_translate;
            // The rect as Android's layout has it: the painted bounds less
            // the translation of the node and each ancestor.
            let rect = |n: NodeId| -> Option<[f32; 4]> {
                let b = hits.get(&n)?;
                let (mut x, mut y) = (f32::from(b.origin.x), f32::from(b.origin.y));
                let mut at = Some(n);
                while let Some(a) = at {
                    if let Some((tx, ty)) = moved.get(&a) {
                        x -= tx;
                        y -= ty;
                    }
                    at = parents.get(&a).copied();
                }
                Some([x, y, f32::from(b.size.width), f32::from(b.size.height)])
            };
            self.motion
                .report_layout(|n| Some((rect(n)?, parents.get(&n).and_then(|p| rect(*p)))));
        }
        for (id, port) in &self.ports {
            let port = port.borrow();
            self.motion
                .scrolled(*id, port.horizontal, port.scroller.pos);
        }
        let ticked = std::mem::take(&mut self.motion_ticked);
        if self.motion.due() && !ticked {
            self.motion.frame();
        }
        let shown = self.motion_writes();
        if shown || self.settle > 0 || !self.nav_anims.is_empty() {
            if frame_logging() {
                crate::guest::log(&format!(
                    "[frame] next: motion {}, settle {}, nav {}",
                    self.motion.due(),
                    self.settle,
                    self.nav_anims.len()
                ));
            }
            self.settle = self.settle.saturating_sub(1);
            window.request_animation_frame();
        } else if self.motion.due() {
            self.motion_clock(cx);
        }
    }

    /// The motion engine's last writes marked dirty and its op 8 pairs sent
    /// to the guest. Whether a write reached a node the last frame drew.
    fn motion_writes(&mut self) -> bool {
        let touched = std::mem::take(&mut self.motion.touched);
        let shown = touched.iter().any(|n| self.drawn.contains(n));
        self.dirty.extend(touched);
        for pairs in std::mem::take(&mut self.motion.outbox) {
            self.send(&UiEvent::Motion(pairs));
        }
        shown
    }

    /// Ticks of the motion engine, a display frame apart, while it wants
    /// frames but writes only to nodes that aren't drawn, such as the
    /// opacity pulse under a hidden `stack` on the signed-in chat. A
    /// hidden Android view's writes don't invalidate anything on screen,
    /// and these don't draw a frame. The first write to a drawn node does.
    fn motion_clock(&mut self, cx: &mut Context<Self>) {
        if self.motion_clock {
            return;
        }
        self.motion_clock = true;
        let frame = self.sent_frame.unwrap_or(crate::guest::FRAME);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(frame).await;
                let again = this.update(cx, |this, cx| {
                    if !this.motion.due() {
                        return false;
                    }
                    this.motion.frame();
                    if this.motion_writes() {
                        this.motion_ticked = true;
                        cx.notify();
                        return false;
                    }
                    this.motion.due()
                });
                if !matches!(again, Ok(true)) {
                    break;
                }
            }
            let _ = this.update(cx, |this, _| this.motion_clock = false);
        })
        .detach();
    }

    /// A text node's spans as GPUI text. The layout and the links' ranges
    /// are kept, for the span images and for a click on a link.
    fn span_text(&mut self, id: NodeId, laid: Laid) -> AnyElement {
        let styled = gpui::StyledText::new(laid.text).with_runs(laid.runs);
        self.text_layouts.insert(id, styled.layout().clone());
        if laid.links.is_empty() {
            self.text_links.remove(&id);
        } else {
            self.text_links.insert(id, laid.links);
        }
        styled.into_any_element()
    }

    /// The link under a point of a text node, from its last layout.
    fn link_at(&self, id: NodeId, at: gpui::Point<Pixels>) -> Option<String> {
        let layout = self.text_layouts.get(&id)?;
        let i = layout.index_for_position(at).ok()?;
        self.text_links
            .get(&id)?
            .iter()
            .find(|(r, _)| r.contains(&i))
            .map(|(_, l)| l.clone())
    }

    /// The images of a text node's image spans, each over the en spaces
    /// that hold its place, where the last layout of the text put them.
    fn span_images(&mut self, id: NodeId, laid: &Laid, scale: f32) -> Vec<AnyElement> {
        if laid.images.is_empty() {
            return Vec::new();
        }
        let Some(layout) = self.text_layouts.get(&id).cloned() else {
            return Vec::new();
        };
        let Some(origin) = self.hits.borrow().get(&id).map(|b| b.origin) else {
            return Vec::new();
        };
        let line = f32::from(layout.line_height());
        let mut out = Vec::new();
        for (range, img) in &laid.images {
            let (Some(a), Some(b)) = (
                layout.position_for_index(range.start),
                layout.position_for_index(range.end),
            ) else {
                continue;
            };
            // The spaces' width, or the image's if they wrapped.
            let w = if b.y == a.y {
                f32::from(b.x - a.x)
            } else {
                img.size
            };
            let x = f32::from(a.x - origin.x) + (w - img.size).max(0.) / 2.;
            let y = f32::from(a.y - origin.y) + (line - img.size) / 2.;
            let Some(source) =
                self.image(id, &img.uri, img.tint, Some((img.size, img.size)), scale)
            else {
                continue;
            };
            let mut i = img_el(source)
                .size(px(img.size))
                .object_fit(ObjectFit::Contain);
            if img.radius > 0. {
                i = i.rounded(px(img.radius));
            }
            out.push(
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .size(px(img.size))
                    .child(i)
                    .into_any_element(),
            );
        }
        out
    }

    /// A click on a link span: `linkPress` with `{url: <link id>}`, if the
    /// node lists it. The press is remembered, so a URL the guest asks to
    /// open right after it counts as the user's.
    pub fn link_press(&mut self, id: NodeId, link: &str) {
        crate::guest::log(&format!("[view] link {link:?} pressed on #{id}"));
        let fields = vec![(
            PropId::from_name("url").unwrap(),
            Value::Str(link.to_string()),
        )];
        if self.node_event(id, "linkPress", Value::Object(fields)) {
            crate::guest::log(&format!("[view] linkPress on #{id}: link {link:?}"));
            if let Some(g) = &self.guest {
                g.user_link_press();
            }
        }
    }

    /// Where a link span whose text contains `text` was drawn, for a click
    /// the snapshot run dispatches: the middle of its first glyph run.
    pub fn link_point(&self, text: &str) -> Option<gpui::Point<Pixels>> {
        for (id, layout) in &self.text_layouts {
            let Some(v) = self.tree.props().get(*id, ids().spans) else {
                continue;
            };
            let mut at = 0;
            for sp in spans(v) {
                let len = if sp.image.is_some() { 0 } else { sp.text.len() };
                if sp.link.is_some() && sp.text.contains(text) {
                    let a = layout.position_for_index(at)?;
                    let h = layout.line_height();
                    return Some(gpui::point(a.x + px(4.), a.y + h / 2.));
                }
                at += len;
            }
        }
        None
    }

    /// `range` and `startReached` for each list that listens for them, from
    /// the rows the last paint showed, as the Android platform's
    /// `RecyclerView` scroll listener sends them (`modules.eq`). Positions
    /// are the adapter's: the list's rows are positions `first` on, of
    /// `count`. `range` goes out when its fields change; `startReached`
    /// when the first row shown comes within the first three positions.
    /// `layout` to each node whose `on` lists it, with its `width` and
    /// `height` in dp from the last paint, when they change, as the
    /// Android platform sends it (`lh.a0`).
    fn layout_events(&mut self) {
        let on = ids().on;
        let listening: Vec<NodeId> = self
            .tree
            .nodes()
            .map(|n| n.id)
            .filter(|id| {
                matches!(self.tree.props().get(*id, on), Some(Value::Array(a))
                    if a.iter().any(|v| matches!(v, Value::Str(s) if s == "layout")))
            })
            .collect();
        for id in listening {
            let Some(b) = self.hits.borrow().get(&id).copied() else {
                continue;
            };
            let size = (f32::from(b.size.width), f32::from(b.size.height));
            if self.layouts_sent.get(&id) == Some(&size) {
                continue;
            }
            self.layouts_sent.insert(id, size);
            let p = |n: &str| PropId::from_name(n).unwrap();
            let mut fields = vec![
                (p("width"), Value::F64(f64::from(size.0))),
                (p("height"), Value::F64(f64::from(size.1))),
            ];
            fields.sort_by_key(|(p, _)| p.0);
            self.node_event(id, "layout", Value::Object(fields));
        }
        let tree = &self.tree;
        self.layouts_sent.retain(|id, _| tree.node(*id).is_some());
    }

    /// `blur` to the text field that had focus once it no longer has it.
    fn blur_events(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(id) = self.focused else { return };
        let still = self
            .inputs
            .get(&id)
            .is_some_and(|e| e.read(cx).focus_handle(cx).is_focused(window));
        if !still {
            self.focused = None;
            self.node_event(id, "blur", Value::Object(vec![]));
        }
    }

    fn list_events(&mut self) {
        let lists: Vec<NodeId> = self
            .tree
            .nodes()
            .filter(|n| n.kind.name() == "list")
            .map(|n| n.id)
            .filter(|id| self.listens(*id, "range") || self.listens(*id, "startReached"))
            .collect();
        for id in lists {
            // The positions, count and epoch of the frame last laid out:
            // a turn that changed the list since is not on screen yet, and
            // a range in its epoch with the old rows' positions sent the
            // guest's window to rows nobody was looking at.
            let Some((start, end, count, epoch)) = self.ports.get(&id).and_then(|p| {
                let p = p.borrow();
                let (start, end) = p.shown()?;
                Some((start, end, p.count.unwrap_or(i32::MAX), p.epoch))
            }) else {
                continue;
            };
            if self.listens(id, "range") {
                let at_end = end >= count;
                let key = format!("{start}:{end}:{epoch}:{at_end}");
                if self.ranges.get(&id) != Some(&key) {
                    self.ranges.insert(id, key);
                    let p = |n: &str| PropId::from_name(n).unwrap();
                    // Android's `scrolling` is true once the list is
                    // idle (`getScrollState() == 0`), and a GPUI scroll
                    // has no state but idle.
                    let mut fields = vec![
                        (p("start"), Value::I32(start)),
                        (p("end"), Value::I32(end)),
                        (p("epoch"), Value::I64(epoch)),
                        (p("scrolling"), Value::Bool(true)),
                        (p("atEnd"), Value::Bool(at_end)),
                        (p("screenStart"), Value::I32(start)),
                        (p("screenEnd"), Value::I32(end)),
                    ];
                    fields.sort_by_key(|(p, _)| p.0);
                    crate::guest::log(&format!(
                        "[view] list #{id}: range {start}..{end} of {count}, epoch {epoch}"
                    ));
                    self.node_event(id, "range", Value::Object(fields));
                }
            }
            if self.listens(id, "startReached") {
                let reached = (0..3).contains(&start);
                if reached && self.start_reached.get(&id) != Some(&true) {
                    crate::guest::log(&format!("[view] list #{id}: startReached at {start}"));
                    self.node_event(id, "startReached", Value::Object(vec![]));
                }
                self.start_reached.insert(id, reached);
            }
        }
    }

    fn sync_window(&mut self, window: &Window) {
        let size = window.viewport_size();
        let now = (
            f32::from(size.width),
            f32::from(size.height),
            window.scale_factor(),
        );
        let sane = |v: f32| v > 0.0 && v < 100_000.0;
        if self.sent_window == Some(now) || self.guest.is_none() || !sane(now.0) || !sane(now.1) {
            return;
        }
        let first = self.sent_window.is_none();
        self.sent_window = Some(now);
        if let Some(g) = &self.guest {
            g.set_window(now.0, now.1, now.2);
        }
        self.motion.set_density(now.2);
        self.stats.window_events.push(self.stats.turns.len());
        crate::guest::log(&format!(
            "[view] window {} x {} dp, density {}",
            now.0, now.1, now.2
        ));
        self.send(&UiEvent::Window {
            width: now.0,
            height: now.1,
            density: now.2,
        });
        if first {
            self.send(&UiEvent::Insets(Insets::default()));
        }
    }

    /// `frameInterval` (op 6) with the time between refreshes of the
    /// display the window is on, as the Android platform sends the panel's,
    /// and again when it changes. The guest's frames follow it.
    fn sync_frame(&mut self, window: &Window, cx: &App) {
        if self.sent_window.is_none() {
            return;
        }
        let frame = window
            .refresh_interval(cx)
            .or_else(|| cx.primary_display()?.refresh_interval())
            .unwrap_or(crate::guest::FRAME);
        if self.sent_frame == Some(frame) {
            return;
        }
        self.sent_frame = Some(frame);
        let ms = frame.as_secs_f64() * 1000.0;
        if let Some(g) = &self.guest {
            g.set_frame_interval(frame);
        }
        self.motion.frame_ms = ms as f32;
        crate::guest::log(&format!(
            "[view] frameInterval {ms:.3} ms ({:.1} Hz)",
            1000.0 / ms
        ));
        self.send(&UiEvent::FrameInterval(ms));
    }

    pub fn text_of(&self, node: NodeId) -> Option<&str> {
        match self.tree.props().get(node, ids().text) {
            Some(Value::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// The nodes shown on screen now: the root and what is under it,
    /// following only a navigator's current screen.
    pub fn shown(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack: Vec<NodeId> = self.tree.root().into_iter().collect();
        while let Some(id) = stack.pop() {
            let Some(n) = self.tree.node(id) else {
                continue;
            };
            if self.tree.props().get(id, ids().hidden) == Some(&Value::Bool(true)) {
                continue;
            }
            out.push(id);
            if n.kind.name() == "navigator" {
                stack.extend(self.screens(id).iter().rev());
            } else {
                stack.extend(n.children.iter().rev());
            }
        }
        out
    }

    /// The shown tree, one node a line, indented by depth: kind, id, the
    /// bounds it was painted at and its cells.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        let hits = self.hits.borrow();
        let mut stack: Vec<(NodeId, usize)> =
            self.tree.root().map(|r| (r, 0)).into_iter().collect();
        while let Some((id, depth)) = stack.pop() {
            let Some(n) = self.tree.node(id) else {
                continue;
            };
            let at = hits.get(&id).map_or_else(
                || "unpainted".to_string(),
                |b| {
                    format!(
                        "{:.1},{:.1} {:.1}x{:.1}",
                        f32::from(b.origin.x),
                        f32::from(b.origin.y),
                        f32::from(b.size.width),
                        f32::from(b.size.height)
                    )
                },
            );
            let mut cells: Vec<String> = self
                .tree
                .props()
                .node_props(id)
                .map(|(k, v)| format!("{}={v:?}", k.name().unwrap_or("?")))
                .collect();
            let mut written = self.motion.written(id);
            written.sort_unstable();
            for (ch, bits) in written {
                cells.push(format!("motion[{ch}]={}", crate::motion::show(ch, bits)));
            }
            let _ = writeln!(
                out,
                "{:indent$}{} #{id} [{at}] {}",
                "",
                n.kind.name(),
                cells.join(" "),
                indent = depth * 2
            );
            if self.tree.props().get(id, ids().hidden) == Some(&Value::Bool(true)) {
                continue;
            }
            if n.kind.name() == "navigator" {
                let shown = self.screens(id);
                let screens: Vec<String> = n
                    .children
                    .iter()
                    .map(|c| {
                        let kind = self.tree.node(*c).map_or("?", |n| n.kind.name());
                        let mark = if shown.contains(c) { " (shown)" } else { "" };
                        format!("{kind} #{c}{mark}")
                    })
                    .collect();
                let _ = writeln!(
                    out,
                    "{:indent$}screens: {}",
                    "",
                    screens.join(", "),
                    indent = depth * 2 + 2
                );
                stack.extend(shown.iter().rev().map(|c| (*c, depth + 1)));
            } else {
                stack.extend(n.children.iter().rev().map(|c| (*c, depth + 1)));
            }
        }
        out
    }

    /// The images on screen: node, uri, and its drawable and how it was
    /// found, or why there is none (`None` before it is first drawn).
    pub fn shown_images(&self) -> Vec<(NodeId, String, Option<&Drawn>)> {
        self.shown()
            .into_iter()
            .filter(|n| self.tree.node(*n).is_some_and(|n| n.kind.name() == "image"))
            .map(|n| {
                let uri = match self.tree.props().get(n, ids().uri) {
                    Some(Value::Str(s)) => s.clone(),
                    _ => String::new(),
                };
                let found = self.image_keys.get(&n).and_then(|k| self.images.get(k));
                (n, uri, found)
            })
            .collect()
    }

    /// A shown node whose text is `text`.
    pub fn find_text(&self, text: &str) -> Option<NodeId> {
        self.shown()
            .into_iter()
            .find(|n| self.text_of(*n) == Some(text))
    }

    /// A shown node whose `accessibilityLabel` is `label` and that listens
    /// for `press`.
    pub fn find_button(&self, label: &str) -> Option<NodeId> {
        self.shown().into_iter().find(|n| {
            matches!(self.tree.props().get(*n, ids().label), Some(Value::Str(s)) if s == label)
                && self.listens(*n, "press")
        })
    }

    /// A shown node whose `accessibilityLabel` or `accessibilityId` is
    /// `label`, or starts with it when `label` ends in `*`.
    pub fn find_labelled(&self, label: &str) -> Option<NodeId> {
        let id = PropId::from_name("accessibilityId").unwrap();
        let matches = |s: &str| match label.strip_suffix('*') {
            Some(prefix) => s.starts_with(prefix),
            None => s == label,
        };
        self.shown().into_iter().find(|n| {
            [ids().label, id]
                .iter()
                .any(|k| matches!(self.tree.props().get(*n, *k), Some(Value::Str(s)) if matches(s)))
        })
    }

    pub fn input(&self, node: NodeId) -> Option<Entity<TextInput>> {
        self.inputs.get(&node).cloned()
    }

    pub fn shown_inputs(&self) -> Vec<NodeId> {
        self.shown()
            .into_iter()
            .filter(|n| self.tree.node(*n).is_some_and(|n| n.kind.name() == "input"))
            .collect()
    }

    fn navigator_child(&self, id: NodeId) -> Option<NodeId> {
        let n = self.tree.node(id)?;
        let i = num(self.tree.props().get(id, ids().index)).unwrap_or(0.0) as usize;
        n.children.get(i).or(n.children.last()).copied()
    }

    /// The screens a navigator draws this frame and where. A change of the
    /// screen its `index` names starts a transition from the screen drawn
    /// before, as `NavigatorLayout.h` starts one (`modules.ih`), and both
    /// are drawn until it ends.
    fn nav_places(&mut self, id: NodeId, children: &[NodeId]) -> Vec<crate::navanim::Place> {
        use crate::navanim::{NavAnim, Place};
        let still = |node| Place {
            node,
            tx: 0.,
            ty: 0.,
            alpha: 1.,
        };
        let screens = self.screens(id);
        let props = self.tree.props();
        let layers = matches!(
            props.get(id, PropId::from_name("transition").unwrap()),
            Some(Value::Str(s)) if s == "layers"
        );
        let [current] = screens[..] else {
            self.nav_anims.remove(&id);
            return screens.into_iter().map(still).collect();
        };
        if layers || !self.motion.running() {
            self.nav_shown.insert(id, current);
            return vec![still(current)];
        }
        let pos = |n: NodeId| children.iter().position(|c| *c == n);
        if let Some(prev) = self.nav_shown.insert(id, current)
            && prev != current
            && let (Some(a), Some(b)) = (pos(prev), pos(current))
        {
            let push = b > a;
            let top = if push { current } else { prev };
            let text = |n: NodeId, name: &str| match props.get(n, PropId::from_name(name).unwrap())
            {
                Some(Value::Str(s)) => Some(s.clone()),
                _ => None,
            };
            let presentation = text(top, "presentation");
            let kind = match presentation.as_deref() {
                Some("modal" | "transparentModal") => "modal".to_string(),
                _ if props.get(top, PropId::from_name("panelBack").unwrap())
                    == Some(&Value::Bool(true)) =>
                {
                    "panels".to_string()
                }
                Some("card") => "card".to_string(),
                _ => text(id, "transition").unwrap_or_else(|| "slide".to_string()),
            };
            crate::guest::log(&format!(
                "[view] navigator #{id}: {kind} {} from #{prev} to #{current}",
                if push { "push" } else { "pop" }
            ));
            self.nav_anims
                .insert(id, NavAnim::new(prev, current, push, kind));
        }
        let Some(anim) = self.nav_anims.get(&id) else {
            return vec![still(current)];
        };
        if anim.done() || anim.to != current || pos(anim.from).is_none() {
            self.nav_anims.remove(&id);
            return vec![still(current)];
        }
        let (w, h) = self.hits.borrow().get(&id).map_or((412., 860.), |b| {
            (f32::from(b.size.width), f32::from(b.size.height))
        });
        anim.places(pos(anim.from) < pos(anim.to), w, h)
    }

    /// The screens a navigator shows, bottom first. With `transition:
    /// "layers"` that is every child, stacked, as the Android platform's
    /// `NavigatorLayout` (`mh.settleLayers`) leaves each one visible: the
    /// panels a wide window shows side by side. Otherwise only the child
    /// `index` names.
    ///
    /// At phone width the panels' worklets slide the one not in use off
    /// screen, and phoenix-view runs no worklets. The guest also marks
    /// that panel `accessibilityHidden` with `pointerEvents: "none"`, so
    /// a screen whose panel is marked so is left out.
    fn screens(&self, id: NodeId) -> Vec<NodeId> {
        let props = self.tree.props();
        let layers = matches!(
            props.get(id, PropId::from_name("transition").unwrap()),
            Some(Value::Str(s)) if s == "layers"
        );
        let put_away = |c: NodeId| {
            let hidden = PropId::from_name("accessibilityHidden").unwrap();
            let pointer = PropId::from_name("pointerEvents").unwrap();
            let marked = |n: NodeId| {
                props.get(n, hidden) == Some(&Value::Bool(true))
                    && matches!(props.get(n, pointer), Some(Value::Str(s)) if s == "none")
            };
            marked(c)
                || self
                    .tree
                    .node(c)
                    .is_some_and(|n| n.children.len() == 1 && marked(n.children[0]))
        };
        match self.tree.node(id) {
            // With the motion engine running, every layer stays, as
            // `NavigatorLayout` leaves them all visible and the panels'
            // worklets move them.
            Some(n) if layers && self.motion.running() => n.children.clone(),
            Some(n) if layers => {
                let shown: Vec<NodeId> = n
                    .children
                    .iter()
                    .copied()
                    .filter(|c| !put_away(*c))
                    .collect();
                if shown.is_empty() {
                    self.navigator_child(id).into_iter().collect()
                } else {
                    shown
                }
            }
            _ => self.navigator_child(id).into_iter().collect(),
        }
    }

    /// The drawable `res://<name>` names, as the Java platform's
    /// `getDrawable` finds it (see [`crate::drawable`]), tinted with `tint`;
    /// `size` is the box in dp, for a drawable with no size of its own.
    fn image(
        &mut self,
        node: NodeId,
        uri: &str,
        tint: Option<u32>,
        size: Option<(f32, f32)>,
        scale: f32,
    ) -> Option<ImageSource> {
        if uri.starts_with("https:") || uri.starts_with("http:") {
            return self.remote_image(node, uri, tint);
        }
        // A drawable with a size of its own is loaded once; one drawn into
        // its box, again for each box.
        let plain = format!("{uri}|{tint:?}");
        let boxed = format!("{plain}|{size:?}");
        let key = if self.boxed.contains(&plain) {
            boxed.clone()
        } else {
            plain.clone()
        };
        if let Some(i) = self.images.get(&key) {
            self.image_keys.insert(node, key);
            return i.as_ref().ok().map(|(i, _)| i.clone());
        }
        let found = self.load_image(uri, tint, size, scale);
        let shown = short(uri);
        let key = match &found {
            Ok((_, how, true)) => {
                crate::guest::log(&format!("[view] {shown}: {how}, in a {size:?} box"));
                self.boxed.insert(plain);
                boxed
            }
            Ok((_, how, false)) => {
                crate::guest::log(&format!("[view] {shown}: {how}"));
                plain
            }
            Err(miss) => {
                crate::guest::log(&format!("[view] {shown}: not drawn: {}", miss.why));
                plain
            }
        };
        let found = found.map(|(i, how, _)| (ImageSource::Image(i), how));
        let image = found.as_ref().ok().map(|(i, _)| i.clone());
        self.image_keys.insert(node, key.clone());
        self.images.insert(key, found);
        image
    }

    /// A remote image: drawn once the loader has it. The first time a node
    /// asks, the loader fetches it; when it is done, each node that shows
    /// it is sent `load` or `error`.
    fn remote_image(&mut self, node: NodeId, uri: &str, tint: Option<u32>) -> Option<ImageSource> {
        let key = format!("{uri}|{tint:?}");
        self.image_keys.insert(node, key.clone());
        if let Some(found) = self.images.get(&key) {
            let image = found.as_ref().ok().map(|(i, _)| i.clone());
            self.tell(node, uri, &key);
            return image;
        }
        if self.pending.contains(&key) {
            return None;
        }
        if let Some(loader) = &self.loader {
            self.pending.insert(key.clone());
            loader.load(key, uri.to_string(), tint);
        } else {
            let why = "a remote image, and the network is off without --live".to_string();
            crate::guest::log(&format!("[view] {}: not drawn: {why}", short(uri)));
            self.images.insert(
                key.clone(),
                Err(NoImage {
                    why,
                    apk_incomplete: false,
                }),
            );
            self.tell(node, uri, &key);
        }
        None
    }

    fn on_image(&mut self, done: crate::remote::Done, cx: &mut Context<Self>) {
        let key = done.key;
        self.pending.remove(&key);
        let uri = key
            .rsplit_once('|')
            .map_or(key.as_str(), |(u, _)| u)
            .to_string();
        let found = match done.result {
            Ok(d) => {
                crate::guest::log(&format!("[view] {}: {}", short(&uri), d.how));
                self.loaded.insert(key.clone(), (d.width, d.height));
                Ok((ImageSource::Render(d.image), d.how))
            }
            Err(why) => {
                crate::guest::log(&format!("[view] {}: not drawn: {why}", short(&uri)));
                Err(NoImage {
                    why,
                    apk_incomplete: false,
                })
            }
        };
        self.images.insert(key.clone(), found);
        let nodes: Vec<NodeId> = self
            .image_keys
            .iter()
            .filter(|(n, k)| **k == key && self.tree.node(**n).is_some())
            .map(|(n, _)| *n)
            .collect();
        for n in nodes {
            self.dirty.insert(n);
            self.tell(n, &uri, &key);
        }
        cx.notify();
    }

    /// Send `load` (`width`, `height` and `uri`) or `error` (`uri`) to an
    /// image node once per uri, as the Android platform does when the
    /// node's `on` lists them.
    fn tell(&mut self, node: NodeId, uri: &str, key: &str) {
        if self.told.get(&node).is_some_and(|u| u == uri) {
            return;
        }
        let Some(found) = self.images.get(key) else {
            return;
        };
        self.told.insert(node, uri.to_string());
        let p = ids();
        let mut fields = vec![(p.uri, Value::Str(uri.to_string()))];
        let name = if found.is_ok() {
            if let Some(&(w, h)) = self.loaded.get(key) {
                fields.push((p.width, Value::I32(w as i32)));
                fields.push((p.height, Value::I32(h as i32)));
            }
            "load"
        } else {
            "error"
        };
        fields.sort_by_key(|(p, _)| p.0);
        self.node_event(node, name, Value::Object(fields));
    }

    /// An avatar `placeholder` drawable (`res://rn_default_avatar_*`), the
    /// coloured circle Android draws behind the avatar while the image
    /// loads. Loaded once per uri and box, with no tint, and kept apart
    /// from the node's own image so its `load`/`error` bookkeeping is
    /// untouched.
    fn placeholder_image(
        &mut self,
        uri: &str,
        size: Option<(f32, f32)>,
        scale: f32,
    ) -> Option<ImageSource> {
        let key = format!("{uri}|{size:?}");
        if let Some(found) = self.placeholders.get(&key) {
            return found.clone();
        }
        let found = self
            .load_image(uri, None, size, scale)
            .ok()
            .map(|(i, _, _)| ImageSource::Image(i));
        self.placeholders.insert(key, found.clone());
        found
    }

    fn load_image(
        &self,
        uri: &str,
        tint: Option<u32>,
        size: Option<(f32, f32)>,
        scale: f32,
    ) -> Result<(Arc<Image>, String, bool), NoImage> {
        use crate::drawable::{Missing, explain_missing, load};
        let miss = |why: String| NoImage {
            why,
            apk_incomplete: false,
        };
        if let Some(data) = uri.strip_prefix("data:") {
            return data_image(data)
                .map(|(format, bytes)| {
                    (
                        Arc::new(Image::from_bytes(format, bytes)),
                        format!("a data: uri, {format:?}"),
                        false,
                    )
                })
                .ok_or_else(|| miss("a data: uri that isn't a base64 image".into()));
        }
        // `emoji://<codepoints>` is the APK's `assets/emoji-<codepoints>.png`,
        // or the same without `-fe0f`, as the Android platform finds it
        // (`dd.m`).
        if let Some(code) = uri.strip_prefix("emoji://") {
            let Some(apk) = &self.apk else {
                return Err(miss("no APK".into()));
            };
            return [code.to_string(), code.replace("-fe0f", "")]
                .iter()
                .find_map(|c| {
                    let name = format!("assets/emoji-{c}.png");
                    apk.read(&name).map(|b| (b, name))
                })
                .map(|(bytes, name)| {
                    (
                        Arc::new(Image::from_bytes(gpui::ImageFormat::Png, bytes)),
                        name,
                        false,
                    )
                })
                .ok_or_else(|| miss(format!("the APK has no assets/emoji-{code}.png")));
        }
        let Some(name) = uri.strip_prefix("res://") else {
            let kind = uri.split_once(':').map_or("a schemeless", |(s, _)| s);
            return Err(miss(format!(
                "a {kind}: uri, which phoenix-view doesn't load"
            )));
        };
        let Some(apk) = &self.apk else {
            return Err(miss("no APK".into()));
        };
        let want = crate::res::Want {
            dpi: (scale * 160.).round() as u16,
            night: true,
        };
        let absent = |what: &str| {
            let (why, apk_incomplete) = explain_missing(apk, what);
            NoImage {
                why,
                apk_incomplete,
            }
        };
        match load(apk, name, want, tint, size) {
            Ok(l) => Ok((
                Arc::new(Image::from_bytes(l.format, l.bytes)),
                l.how,
                l.boxed,
            )),
            Err(Missing::NotInTable) => Err(absent(name)),
            Err(Missing::Absent(what)) => Err(absent(&what)),
            Err(Missing::NoConfig) => {
                Err(miss(format!("{name} has no configuration for {want:?}")))
            }
            Err(Missing::Unsupported(why)) => Err(miss(why)),
        }
    }

    fn render_node(
        &mut self,
        id: NodeId,
        parent: Parent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(node) = self.tree.node(id) else {
            return div().into_any_element();
        };
        self.built += 1;
        let kind = node.kind.name().to_string();
        let children = node.children.clone();
        let p = ids();
        // In `PropId` order, as the tree keeps them.
        let tree = self.tree.clone();
        let cells: Vec<(PropId, &Value)> = tree.props().node_props(id).collect();
        let get = |k: PropId| {
            cells
                .binary_search_by_key(&k.0, |(p, _)| p.0)
                .ok()
                .map(|i| cells[i].1)
        };
        // What the motion engine last wrote for a channel of this node,
        // which stands over the cell until the next write, as on Android.
        let written = self.motion.written(id);
        let mv = |ch: u32| written.iter().find(|(c, _)| *c == ch).map(|(_, v)| *v);
        let mf = |ch: u32| mv(ch).map(f32::from_bits).filter(|f| f.is_finite());
        if get(p.hidden) == Some(&Value::Bool(true)) || self.absorbed.remove(&id) {
            return div().into_any_element();
        }
        self.drawn.insert(id);
        // A popover takes no room in its parent: it is drawn over
        // everything once the tree is, by `render_popover`.
        if kind == "popover" && !std::mem::take(&mut self.hoisting) {
            self.popovers.push(id);
            return div().absolute().size_0().into_any_element();
        }
        let text = |k: PropId| match get(k) {
            Some(Value::Str(s)) => Some(s.clone()),
            _ => None,
        };
        // Android's flex lays out as Yoga does: an item without `flex` keeps
        // its size, and an item's minimum is its `minWidth`/`minHeight`, 0
        // by default, never its content's size (CSS's `min-height: auto`).
        // Without that, an image's aspect ratio or a scroll's content can
        // push its column past the window.
        let mut el = div()
            .id(id as usize)
            .relative()
            .when(matches!(kind.as_str(), "modal" | "systemOverlay"), |el| {
                el.size_full()
            })
            .flex_shrink_0()
            .min_w(px(0.))
            .min_h(px(0.));

        // The box: size, flex, spacing, background, border, radius.
        let fill_w = text(p.width).as_deref() == Some("fill");
        let fill_h = text(p.height).as_deref() == Some("fill");
        // A layer's box is a flex column, so `"fill"` there is a stretch
        // and a grow, which keep the node's margins inside the box, as a
        // `FrameLayout` child with `MATCH_PARENT` and margins is laid out.
        // 100% and a margin would run past the box's edge.
        match (parent, fill_w) {
            (Parent::Row, true) => el = el.flex_grow(1.0).flex_shrink(1.0).flex_basis(px(0.)),
            (Parent::Column | Parent::Viewport | Parent::Layer, true) => el = el.self_stretch(),
            _ => {}
        }
        match (parent, fill_h) {
            (Parent::Column, true) => el = el.flex_grow(1.0).flex_shrink(1.0).flex_basis(px(0.)),
            (Parent::Viewport, _) => el = el.flex_grow(1.0),
            (Parent::Row, true) => el = el.self_stretch(),
            (Parent::Layer, true) => el = el.flex_grow(1.0).flex_shrink(1.0),
            _ => {}
        }
        // A size in dp keeps the shrink set above: a `"fill"` height
        // with a fixed width (the chat panel's column at desktop width)
        // must still shrink to its box.
        if let Some(w) = mf(channel::WIDTH).or(num(get(p.width))) {
            el = el.w(px(w));
        }
        if let Some(h) = mf(channel::HEIGHT).or(num(get(p.height))) {
            el = el.h(px(h));
        }
        // A percentage of the parent's box, such as the you-bar card's
        // corner pieces: a 50% clip holding a 200% card.
        if let Some(f) = text(p.width).as_deref().and_then(percent) {
            el = el.w(relative(f));
        }
        if let Some(f) = text(p.height).as_deref().and_then(percent) {
            el = el.h(relative(f));
        }
        if let Some(f) = num(get(p.flex)) {
            el = el.flex_grow(f).flex_shrink(1.0).flex_basis(px(0.));
        }
        // `flexBasis`: "auto" starts the item at its content's size, a
        // number at that many dp.
        match get(p.flex_basis) {
            Some(Value::Str(s)) if s == "auto" => el = el.flex_basis(gpui::Length::Auto),
            v => {
                if let Some(b) = num(v) {
                    el = el.flex_basis(px(b));
                }
            }
        }
        if let Some(v) = num(get(p.min_width)) {
            el = el.min_w(px(v));
        }
        if let Some(v) = num(get(p.min_height)) {
            el = el.min_h(px(v));
        }
        if let Some(v) = num(get(p.max_width)) {
            el = el.max_w(px(v));
        }
        if let Some(v) = num(get(p.max_height)) {
            el = el.max_h(px(v));
        }
        if let Some(Value::Quad([l, t, r, b])) = get(p.padding) {
            el = el.pl(px(*l)).pt(px(*t)).pr(px(*r)).pb(px(*b));
        }
        if let Some(b) = mf(channel::PADDING_BOTTOM) {
            el = el.pb(px(b));
        }
        if let Some(Value::Quad([l, t, r, b])) = get(p.margin) {
            el = el.ml(px(*l)).mt(px(*t)).mr(px(*r)).mb(px(*b));
        }
        if let Some(g) = num(get(p.gap)) {
            el = el.gap(px(g));
        }
        if let Some(c) = get(p.background).and_then(Value::as_color) {
            el = el.bg(argb(c));
        }
        // `blur` has no backdrop blur here: Android puts its `fallback`
        // colour under the view (`lh.k1`), the dim under an alert.
        if let Some(Value::Object(b)) = get(p.blur)
            && get(p.background).is_none()
            && let Some(c) = b
                .iter()
                .find(|(k, _)| k.name() == Some("fallback"))
                .and_then(|(_, v)| v.as_color())
        {
            el = el.bg(argb(c));
        }
        let axis = text(p.background_gradient_axis);
        match get(p.background_gradient).map(|g| (gradient_layers(g, axis.as_deref()), g)) {
            Some((Some(layers), _)) => el = el.children(layers),
            Some((None, g)) => {
                if let Some(g) = gradient(g, axis.as_deref()) {
                    el = el.bg(g);
                }
            }
            None => {}
        }
        if let Some(sh) = get(p.shadow).and_then(shadow) {
            el = el.shadow(vec![sh]);
        }
        // `pressRipple`: a ripple when pressed, here a light wash over the
        // view while the pointer is down on it.
        if get(p.press_ripple).is_some() {
            el = el.active(|s| s.bg(gpui::hsla(0., 0., 1., 0.12)));
        }
        if let Some(c) = mv(channel::BACKGROUND) {
            el = el.bg(argb(c));
        }
        // The corners a clipping parent hands down, for a child that sets
        // no radius of its own.
        let inherited = self.outlines.remove(&id);
        let own = [
            p.radius_top_left,
            p.radius_top_right,
            p.radius_bottom_right,
            p.radius_bottom_left,
        ]
        .map(|k| num(get(k)));
        let radius = mf(channel::RADIUS).or(num(get(p.radius)));
        let outline = match (radius, inherited) {
            (Some(r), _) => Some(r),
            (None, Some(c)) if own.iter().all(Option::is_none) => {
                set_corners(&mut el, c);
                c.iter().flatten().copied().reduce(f32::max)
            }
            _ => None,
        };
        if let Some(r) = radius {
            el = el.rounded(px(r));
        }
        set_corners(&mut el, own);
        // `clipOutline` clips the node to a rounded rectangle (`radius`)
        // or a circle (`circle`), and `clip` with a `radius` to its rounded
        // box. GPUI clips children to a rectangle only, so the children
        // that touch a corner take its radius: all of them for
        // `clipOutline`, whose children are layers over the whole box, and
        // the first and last along the axis for `clip` (an embed's accent
        // bar on the left of its rounded card).
        let punches = match get(p.clip_outline) {
            Some(Value::Object(f)) => self.punches(id, f),
            _ => Vec::new(),
        };
        if let Some(c) = get(p.background).and_then(Value::as_color)
            && c >> 24 == 0xff
        {
            self.painted.push((id, c));
        }
        let clip_radius = match get(p.clip_outline) {
            Some(Value::Object(f)) => {
                let field = |n: &str| f.iter().find(|(k, _)| k.name() == Some(n)).map(|(_, v)| v);
                if field("circle") == Some(&Value::Bool(true)) {
                    Some(9999.)
                } else {
                    num(field("radius")).filter(|r| *r > 0.)
                }
            }
            _ => None,
        };
        if let Some(r) = clip_radius {
            el = el.rounded(px(r)).overflow_hidden();
            for c in &children {
                self.outlines.insert(*c, [Some(r); 4]);
            }
        } else if let (Some(r), Some(Value::Bool(true))) = (num(get(p.radius)), get(p.clip)) {
            let row = text(p.direction).as_deref() == Some("row");
            let (start, end) = if row {
                (
                    [Some(r), None, None, Some(r)],
                    [None, Some(r), Some(r), None],
                )
            } else {
                (
                    [Some(r), Some(r), None, None],
                    [None, None, Some(r), Some(r)],
                )
            };
            let mut flow: Vec<NodeId> = children
                .iter()
                .copied()
                .filter(|c| !self.overlays(*c))
                .collect();
            // A row's first child that is only a narrow bar of colour (an
            // embed's accent) is drawn as the card's left border, which
            // GPUI curves along the rounded corners as Android's clip
            // curves the bar.
            if let Some((c, w, colour)) =
                row.then(|| flow.first().copied()).flatten().and_then(|c| {
                    let props = self.tree.props();
                    let w = num(props.get(c, p.width)).filter(|w| *w <= 8.)?;
                    let colour = props.get(c, p.background).and_then(Value::as_color)?;
                    self.tree.node(c).filter(|n| n.children.is_empty())?;
                    Some((c, w, colour))
                })
            {
                el.style().border_widths.left = Some(px(w).into());
                el = el.border_color(argb(colour));
                self.absorbed.insert(c);
                flow.remove(0);
            }
            if let (Some(first), Some(last)) = (flow.first(), flow.last()) {
                let mut both = start;
                for (b, e) in both.iter_mut().zip(end) {
                    *b = b.or(e);
                }
                if first == last {
                    self.outlines.insert(*first, both);
                } else {
                    self.outlines.insert(*first, start);
                    self.outlines.insert(*last, end);
                }
            }
        }
        if let Some(w) = mf(channel::BORDER_WIDTH).or(num(get(p.border_width))) {
            // `borderSides` turns sides on and off: top, right, bottom,
            // left, as the channel list's panel draws only its top and
            // left edges around its rounded top-left corner.
            let sides = match get(p.border_sides) {
                Some(Value::Array(a)) if a.len() == 4 => {
                    let on = |i: usize| num(a.get(i)).is_none_or(|v| v > 0.);
                    [on(0), on(1), on(2), on(3)]
                }
                _ => [true; 4],
            };
            let s = el.style();
            for (side, on) in [
                &mut s.border_widths.top,
                &mut s.border_widths.right,
                &mut s.border_widths.bottom,
                &mut s.border_widths.left,
            ]
            .into_iter()
            .zip(sides)
            {
                if on {
                    *side = Some(px(w).into());
                }
            }
            if let Some(c) =
                mv(channel::BORDER_COLOR).or(get(p.border_color).and_then(Value::as_color))
            {
                el = el.border_color(argb(c));
            }
        }
        if let Some(o) = mf(channel::OPACITY).or(num(get(p.opacity))) {
            el = el.opacity(o.clamp(0., 1.));
        }
        if get(p.clip) == Some(&Value::Bool(true)) {
            el = el.overflow_hidden();
        }
        // `shrink` lets the node give up room on its parent's main axis,
        // as Yoga's `flexShrink: 1`.
        if get(p.shrink) == Some(&Value::Bool(true)) {
            el = el.flex_shrink(1.0);
        }
        // `translateX` and `translateY` move the node after layout, as
        // Android's `setTranslationX`: the rail's pills sit at -12 dp,
        // outside the rail, until a server has unread messages.
        // The engine's translation stands over the cell's, as a worklet
        // slides the panels and the rail's selected pill.
        let tx = mf(channel::TRANSLATE_X)
            .or(num(get(p.translate_x)))
            .unwrap_or(0.);
        let ty = mf(channel::TRANSLATE_Y)
            .or(num(get(p.translate_y)))
            .unwrap_or(0.);
        if tx != 0. {
            el = el.left(px(tx));
        }
        if ty != 0. {
            el = el.top(px(ty));
        }
        if tx != 0. || ty != 0. {
            self.drawn_translate.insert(id, (tx, ty));
        } else {
            self.drawn_translate.remove(&id);
        }

        // Children are laid out by the container's kind.
        let row = text(p.direction).as_deref() == Some("row");
        let align = text(p.align);
        let justify = text(p.justify);
        let flexbox = |mut el: gpui::Stateful<gpui::Div>| {
            el = el.flex();
            el = if row { el.flex_row() } else { el.flex_col() };
            el = match align.as_deref() {
                Some("center") => el.items_center(),
                Some("end") => el.items_end(),
                Some("stretch") => el.items_stretch(),
                _ => el.items_start(),
            };
            match justify.as_deref() {
                Some("center") => el.justify_center(),
                Some("end") => el.justify_end(),
                Some("between") => el.justify_between(),
                Some("around") => el.justify_around(),
                Some("evenly") => el.justify_evenly(),
                _ => el.justify_start(),
            }
        };
        let axis = if row { Parent::Row } else { Parent::Column };

        // The innermost node that takes the click has it, as the Android
        // view that handles a touch consumes it. Pressing it reports the
        // pressed input to the motion engine, which runs the button's
        // `press_look`.
        if self.listens(id, "press") || self.rows.contains_key(&id) {
            el = el.on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    let nodes = this.pressed_views(id);
                    this.motion.press(nodes);
                    cx.notify();
                }),
            );
        }
        if self.listens(id, "press") {
            el = el
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.node_event(id, "press", Value::Null);
                    cx.stop_propagation();
                }));
        } else if let Some(&(list, index)) = self.rows.get(&id) {
            el = el
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.row_press(list, index);
                    cx.stop_propagation();
                }));
        }
        let hits = self.hits.clone();
        let drawn = match kind.as_str() {
            "text" | "markup" => {
                let upper = text(p.text_transform).as_deref() == Some("uppercase");
                // `spans` replaces `text`, as the Android platform's
                // `TextView` takes the `SpannableStringBuilder` built from
                // them: message bodies and DM previews carry only spans.
                let text_color = mv(channel::TEXT_COLOR).or(get(p.color).and_then(Value::as_color));
                let family_name = if text(p.font_family).as_deref() == Some("ginto") {
                    self.fonts.ginto.clone()
                } else {
                    self.fonts.sans.clone()
                };
                let mut overlays = Vec::new();
                let content = if let Some(runs) = get(p.spans).map(spans) {
                    let base = RunBase {
                        family: family_name.clone().unwrap_or_else(|| "gg sans".into()),
                        mono: self.fonts.mono.clone(),
                        weight: num(get(p.font_weight)).unwrap_or(400.),
                        color: text_color.unwrap_or(0xffff_ffff),
                        font_size: num(get(p.font_size)).unwrap_or(16.),
                    };
                    let laid = lay_out(runs, upper, &base);
                    overlays = self.span_images(id, &laid, window.scale_factor());
                    self.span_text(id, laid)
                } else {
                    let mut s = text(p.text).unwrap_or_default();
                    if upper {
                        s = s.to_uppercase();
                    }
                    SharedString::from(s).into_any_element()
                };
                // Text is measured at most as wide as its parent, as Yoga
                // measures a text node with the parent's width as its limit.
                let mut t = el.flex().flex_col();
                if get(p.max_width).is_none() {
                    t = t.max_w(relative(1.));
                }
                if let Some(c) = text_color {
                    t = t.text_color(argb(c));
                }
                if let Some(fs) = num(get(p.font_size)) {
                    t = t.text_size(px(fs));
                }
                if let Some(w) = num(get(p.font_weight)) {
                    t = t.font_weight(FontWeight(w));
                }
                if let Some(lh) = num(get(p.line_height)) {
                    t = t.line_height(px(lh));
                }
                if let Some(f) = family_name {
                    t = t.font_family(f);
                }
                t = match text(p.text_align).as_deref() {
                    Some("center") => t.text_align(TextAlign::Center),
                    Some("right" | "end") => t.text_align(TextAlign::Right),
                    _ => t,
                };
                // `maxLines` ends the last line with an ellipsis, as the
                // Android platform's `TextView` does with `ellipsize: end`.
                match num(get(p.max_lines)) {
                    Some(n) if n <= 1. => t = t.truncate(),
                    Some(n) => t = t.line_clamp(n as usize).text_ellipsis(),
                    None => {}
                }
                // A click on a link sends `linkPress` with the link's id as
                // `url`, as the Android platform's `LinkSpan` does
                // (`lh.b1`, `bh.e`), and the link takes the click.
                if self.text_links.contains_key(&id) {
                    t = t.on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
                        if let Some(link) = this.link_at(id, ev.position()) {
                            this.link_press(id, &link);
                            cx.stop_propagation();
                        }
                    }));
                }
                t.child(content).children(overlays).into_any_element()
            }
            "image" => {
                let uri = text(p.uri).unwrap_or_default();
                let tint = mv(channel::TINT).or(get(p.tint).and_then(Value::as_color));
                // A drawable with no size of its own fills the node: its
                // size in the cells, or as last painted.
                let size = match (num(get(p.width)), num(get(p.height))) {
                    (Some(w), Some(h)) => Some((w, h)),
                    _ => self.hits.borrow().get(&id).map(|b| {
                        (
                            f32::from(b.size.width).round(),
                            f32::from(b.size.height).round(),
                        )
                    }),
                };
                let fit = match text(p.scale).as_deref() {
                    Some("fit") => ObjectFit::Contain,
                    Some("stretch") => ObjectFit::Fill,
                    _ => ObjectFit::Cover,
                };
                // The bitmap is clipped to the node's corners, no rounder
                // than a circle.
                let radius = num(get(p.radius)).or(outline).map(|r| match size {
                    Some((w, h)) => r.min(w.min(h) / 2.),
                    None => r,
                });
                // The placeholder drawable, a coloured default-avatar
                // circle Android shows until the image loads (`dd.K`).
                let placeholder = text(p.placeholder)
                    .filter(|ph| ph.starts_with("res://"))
                    .and_then(|ph| self.placeholder_image(&ph, size, window.scale_factor()));
                let el = el.flex();
                match self.image(id, &uri, tint, size, window.scale_factor()) {
                    Some(image) => {
                        // `scaleFactor`, and the engine's `Scale`, scale the
                        // view about its centre; GPUI scales no box, so the
                        // bitmap is drawn that much smaller in the middle.
                        let factor = mf(channel::SCALE)
                            .or(num(get(p.scale_factor)))
                            .unwrap_or(1.)
                            .clamp(0., 4.);
                        let mut i = img(image).object_fit(fit);
                        i = if (factor - 1.).abs() > 1e-3 {
                            i.w(relative(factor)).h(relative(factor))
                        } else {
                            i.size_full()
                        };
                        if let Some(r) = radius {
                            i = i.rounded(px(r * factor));
                        }
                        el.items_center()
                            .justify_center()
                            .child(i)
                            .into_any_element()
                    }
                    // Still loading or absent: the placeholder circle, as
                    // Android shows it until the image arrives.
                    None => match placeholder {
                        Some(ph) => {
                            let mut i = img(ph).object_fit(ObjectFit::Cover).size_full();
                            if let Some(r) = radius {
                                i = i.rounded(px(r));
                            }
                            el.items_center()
                                .justify_center()
                                .child(i)
                                .into_any_element()
                        }
                        None => el.into_any_element(),
                    },
                }
            }
            "input" => {
                let entity = self.input_entity(id, window, cx);
                let mut i = el.flex().flex_row().items_center();
                if let Some(c) = get(p.color).and_then(Value::as_color) {
                    i = i.text_color(argb(c));
                }
                i = i.text_size(px(num(get(p.font_size)).unwrap_or(16.0)));
                if let Some(f) = &self.fonts.sans {
                    i = i.font_family(f.clone());
                }
                i.child(entity).into_any_element()
            }
            "stack" => {
                // The first child that isn't `absolute` sizes the stack;
                // the rest are layers over it. An `absolute` child can come
                // first, such as a channel row's unread dot, and taking it
                // as the sizing child once collapsed the row's stack and
                // drew the row over the next one.
                let flow = children.iter().copied().find(|c| !self.overlays(*c));
                let mut layers = Vec::new();
                for c in &children {
                    let child = self.render_node(*c, Parent::Layer, window, cx);
                    if Some(*c) == flow {
                        layers.push(child);
                    } else {
                        layers.push(layer(child, self.gravity(*c).as_deref()));
                    }
                }
                el.flex()
                    .flex_col()
                    .children(layers)
                    .children(punches)
                    .into_any_element()
            }
            "navigator" => {
                let places = self.nav_places(id, &children);
                let screens: Vec<AnyElement> = places
                    .into_iter()
                    .enumerate()
                    .map(|(n, place)| {
                        let mut screen = div()
                            .size_full()
                            .flex()
                            .flex_col()
                            .child(self.render_node(place.node, Parent::Layer, window, cx));
                        if n > 0 {
                            screen = screen.absolute().top_0().left_0();
                        }
                        if place.tx != 0. || place.ty != 0. {
                            screen = screen.relative().left(px(place.tx)).top(px(place.ty));
                            if n > 0 {
                                screen = screen.absolute();
                            }
                        }
                        if place.alpha < 1. {
                            screen = screen.opacity(place.alpha.max(0.));
                        }
                        screen.into_any_element()
                    })
                    .collect();
                el.flex().flex_col().children(screens).into_any_element()
            }
            "scroll" | "list" | "hscroll" => {
                // A list that listens for `rowPress` takes a click on any of
                // its rows. Its children are the rows from `first` on.
                let first = num(get(p.first)).unwrap_or(0.) as i32;
                if kind == "list" && self.listens(id, "rowPress") {
                    for (n, c) in children.iter().enumerate() {
                        self.rows.insert(*c, (id, first + n as i32));
                    }
                }
                let horizontal = kind == "hscroll";
                let stick = !horizontal && get(p.stick_to_end) == Some(&Value::Bool(true));
                let port = self
                    .ports
                    .entry(id)
                    .or_insert_with(|| Rc::new(RefCell::new(Port::new(horizontal, stick))))
                    .clone();
                // The adapter positions outside the rows the guest sent are
                // placeholders, as tall as the rows average, so the list is
                // as long as its `count` and a fling runs on through them.
                let (before, after, est) = {
                    let mut p = port.borrow_mut();
                    p.horizontal = horizontal;
                    p.stick = stick;
                    p.now = self.clock.elapsed().as_secs_f64() * 1000.;
                    p.pad = match get(ids().padding) {
                        Some(Value::Quad([l, _, r, _])) if horizontal => (*l, *r),
                        Some(Value::Quad([_, t, _, b])) => (*t, *b),
                        _ => (0., 0.),
                    };
                    if kind == "list" {
                        p.first = first;
                        p.count = num(get(ids().count)).map(|c| c as i32);
                        p.epoch = num(get(ids().epoch)).map_or(0, |e| e as i64);
                    } else {
                        p.first = 0;
                        p.count = None;
                    }
                    p.est = p.placeholder();
                    let rows = children.iter().filter(|c| !self.overlays(**c)).count();
                    let (b, a) = p.placeholders(rows);
                    (b, a, p.est)
                };
                let inner = match (horizontal, children.len()) {
                    (true, _) => Parent::Row,
                    (false, 1) => Parent::Viewport,
                    (false, _) => Parent::Column,
                };
                let mut kids = Vec::with_capacity(children.len() + 2);
                let spacer = |n: i32| {
                    let d = div().flex_shrink_0();
                    let len = px(n as f32 * est);
                    if horizontal { d.w(len) } else { d.h(len) }.into_any_element()
                };
                if before > 0 {
                    kids.push(spacer(before));
                }
                for (i, &c) in children.iter().enumerate() {
                    if self.overlays(c) {
                        let child = self.render_node(c, Parent::Layer, window, cx);
                        kids.push(layer(child, self.gravity(c).as_deref()));
                        continue;
                    }
                    // A row well off screen that hasn't changed since it
                    // was last built is laid out as space as tall as it
                    // was: a scroll moves the rows, it doesn't change
                    // them.
                    let space = (kind == "list")
                        .then(|| self.spacer_len(&port.borrow(), c))
                        .flatten();
                    let child = if let Some(len) = space {
                        if self.spaced.insert(c) {
                            self.forget_drawn(c);
                        }
                        let mut d = div().flex_shrink_0().h(px(len));
                        if let Some(Value::Quad([_, t, _, b])) = self.tree.props().get(c, p.margin)
                        {
                            d = d.mt(px(*t)).mb(px(*b));
                        }
                        d.into_any_element()
                    } else {
                        self.spaced.remove(&c);
                        self.clean_rows.insert(c);
                        self.render_node(c, inner, window, cx)
                    };
                    kids.push(RowProbe::new(port.clone(), i, c, child).into_any_element());
                }
                if after > 0 {
                    kids.push(spacer(after));
                }
                // The content keeps its own size: shrunk to the scroll box,
                // as a flex item shrinks by default, it left nothing to
                // scroll, and a long chat could never reach its end.
                let mut content = div().flex().flex_shrink_0();
                content = if horizontal {
                    content.flex_row().min_w(relative(1.))
                } else {
                    content.flex_col().min_h(relative(1.))
                };
                let order = self.scroll_order.clone();
                let mark = canvas(move |_, _, _| order.borrow_mut().push(id), |_, (), _, _| {})
                    .absolute()
                    .size_0();
                // The box clips; the content is drawn moved by the offset
                // the port settles once the frame is laid out.
                let el = el
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(mark)
                    .child(PortContent::new(port.clone(), content.children(kids)));
                PortBox::new(port, el).into_any_element()
            }
            "view" | "flow" | "sheet" | "modal" | "banner" | "popover" | "systemOverlay"
            | "zoom" => {
                let kids = self.render_children(&children, axis, window, cx);
                flexbox(el)
                    .children(kids)
                    .children(punches)
                    .into_any_element()
            }
            other => {
                // A kind this platform doesn't draw (a Rive animation, a
                // video): its box and its children, laid out as a `view`'s,
                // and the kind named once in the log.
                if self.unhandled.insert(other.to_string()) {
                    crate::guest::log(&format!("[view] not drawn: kind {other:?} (#{id})"));
                }
                let kids = self.render_children(&children, axis, window, cx);
                flexbox(el).children(kids).into_any_element()
            }
        };
        Recorded {
            id,
            hits,
            inner: drawn,
        }
        .into_any_element()
    }

    /// A row laid out as space draws none of its nodes: the bounds they
    /// were last painted at are dropped, so nothing reads them as drawn.
    fn forget_drawn(&mut self, row: NodeId) {
        let mut hits = self.hits.borrow_mut();
        let mut stack = vec![row];
        while let Some(n) = stack.pop() {
            hits.remove(&n);
            if let Some(node) = self.tree.node(n) {
                stack.extend(&node.children);
            }
        }
    }

    /// Whether a node sits over its parent instead of in its flow: one
    /// with `absolute`, or a `modal` or `systemOverlay`, which Android shows
    /// in a window of its own over the screen.
    fn overlays(&self, id: NodeId) -> bool {
        self.tree
            .node(id)
            .is_some_and(|n| matches!(n.kind.name(), "modal" | "systemOverlay" | "popover"))
            || self.tree.props().get(id, ids().absolute) == Some(&Value::Bool(true))
    }

    /// A disk over each hole of a node's `clipOutline` (`holes`: x, y and
    /// radius in the node's box), in the colour drawn beneath the hole's
    /// centre. Android clips the node's drawing out of the hole, as the
    /// avatar is cut around its status badge; GPUI clips to a rectangle
    /// only, so this paints what the hole shows over the node's children.
    /// A node with nothing opaque in it, a ring's border, gets none: its
    /// disk would cover what was drawn over the hole before it. The
    /// colour comes from the backgrounds drawn before this node, at their
    /// bounds of the last frame, so the first frame draws no disk.
    fn punches(&self, id: NodeId, outline: &[(PropId, Value)]) -> Vec<AnyElement> {
        let Some(Value::Array(holes)) = outline
            .iter()
            .find(|(k, _)| k.name() == Some("holes"))
            .map(|(_, v)| v)
        else {
            return Vec::new();
        };
        if !self.fills(id, 8) {
            return Vec::new();
        }
        let hits = self.hits.borrow();
        let Some(own) = hits.get(&id).copied() else {
            return Vec::new();
        };
        holes
            .iter()
            .filter_map(|h| {
                let Value::Array(h) = h else { return None };
                let (x, y, r) = (num(h.first())?, num(h.get(1))?, num(h.get(2))?);
                let centre = gpui::point(own.origin.x + px(x), own.origin.y + px(y));
                let colour = self
                    .painted
                    .iter()
                    .rev()
                    .find(|(n, _)| hits.get(n).is_some_and(|b| b.contains(&centre)))
                    .map(|(_, c)| *c)?;
                Some(
                    div()
                        .absolute()
                        .left(px(x - r))
                        .top(px(y - r))
                        .size(px(2. * r))
                        .rounded_full()
                        .bg(argb(colour))
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// Whether a node or one of its descendants, `depth` levels down at
    /// most, draws an image or a background.
    fn fills(&self, id: NodeId, depth: u8) -> bool {
        let props = self.tree.props();
        let Some(node) = self.tree.node(id) else {
            return false;
        };
        node.kind.name() == "image"
            || props.get(id, ids().background).is_some()
            || depth > 0 && node.children.iter().any(|c| self.fills(*c, depth - 1))
    }

    /// A popover, as Android's `PopupWindow` for it shows it: over the
    /// whole window, on a scrim of its `scrim` colour that takes the
    /// clicks outside it (and sends `dismiss` unless `dismissible` is
    /// false), unless it is `passthrough`. Its anchor is its parent. It
    /// sits `offset` below the anchor (`placement` `"bottom"`, the
    /// default) or above it (`"top"`), lined up with the anchor's
    /// `align` edge, or centred on it (the default) and kept `edgeInset`
    /// (12 dp) inside the window. Its place needs its size, so the frame
    /// before its first layout draws it transparent, as Android shows it
    /// once it is placed.
    fn render_popover(
        &mut self,
        id: NodeId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let props = self.tree.props();
        let field = |name: &str| props.get(id, PropId::from_name(name).unwrap()).cloned();
        let s = |name: &str| match field(name) {
            Some(Value::Str(s)) => Some(s),
            _ => None,
        };
        let placement = s("placement").unwrap_or_else(|| "bottom".into());
        let align = s("align").unwrap_or_else(|| "center".into());
        let offset = num(field("offset").as_ref()).unwrap_or(0.);
        let inset = num(field("edgeInset").as_ref()).unwrap_or(12.);
        let inset_left = num(field("edgeInsetLeft").as_ref()).unwrap_or(inset);
        let scrim = field("scrim").and_then(|v| v.as_color());
        let passthrough = field("passthrough") == Some(Value::Bool(true));
        let dismissible = field("dismissible") != Some(Value::Bool(false));
        let anchor = self
            .tree
            .nodes()
            .find(|n| n.children.contains(&id))
            .map(|n| n.id);
        let viewport = window.viewport_size();
        let (ww, wh) = (f32::from(viewport.width), f32::from(viewport.height));
        let (place, size) = {
            let hits = self.hits.borrow();
            (
                anchor.and_then(|a| hits.get(&a).copied()),
                hits.get(&id).map(|b| b.size),
            )
        };
        self.hoisting = true;
        let content = self.render_node(id, Parent::Layer, window, cx);
        let mut boxed = div()
            .absolute()
            .max_w(px((ww - inset - inset_left).max(0.)))
            .child(content);
        match (place, size) {
            (Some(a), Some(size)) => {
                let (left, top) = (f32::from(a.origin.x), f32::from(a.origin.y));
                let (x, y) = popover_origin(
                    [
                        left,
                        top,
                        left + f32::from(a.size.width),
                        top + f32::from(a.size.height),
                    ],
                    (f32::from(size.width), f32::from(size.height)),
                    (ww, wh),
                    (&placement, &align),
                    offset,
                    (inset_left, inset),
                );
                boxed = boxed.left(px(x)).top(px(y));
            }
            _ => boxed = boxed.opacity(0.),
        }
        let mut layer = div().absolute().top_0().left_0().size_full();
        if !passthrough {
            layer = layer.occlude();
            if let Some(c) = scrim {
                layer = layer.bg(argb(c));
            }
            if dismissible {
                layer = layer.on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                        let inside = this
                            .hits
                            .borrow()
                            .get(&id)
                            .is_some_and(|b| b.contains(&ev.position));
                        if !inside {
                            this.node_event(id, "dismiss", Value::Object(vec![]));
                            cx.notify();
                        }
                    }),
                );
            }
        }
        layer.child(boxed).into_any_element()
    }

    fn gravity(&self, id: NodeId) -> Option<String> {
        match self.tree.props().get(id, ids().gravity) {
            Some(Value::Str(s)) => Some(s.clone()),
            _ => None,
        }
    }

    /// A container's children: in its flow on `axis`, or, for one that
    /// [`overlays`](Self::overlays), in a layer over it placed by its
    /// `gravity`.
    fn render_children(
        &mut self,
        children: &[NodeId],
        axis: Parent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        children
            .iter()
            .map(|&c| {
                if self.overlays(c) {
                    let child = self.render_node(c, Parent::Layer, window, cx);
                    layer(child, self.gravity(c).as_deref())
                } else {
                    self.render_node(c, axis, window, cx)
                }
            })
            .collect()
    }

    fn input_entity(
        &mut self,
        id: NodeId,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        let p = ids();
        let props = self.tree.props();
        let value = match props.get(id, p.value) {
            Some(Value::Str(s)) => s.clone(),
            _ => String::new(),
        };
        let placeholder = match props.get(id, p.placeholder) {
            Some(Value::Str(s)) => s.clone(),
            _ => String::new(),
        };
        let secure = props.get(id, p.secure) == Some(&Value::Bool(true));
        let placeholder_color = props
            .get(id, p.placeholder_color)
            .and_then(Value::as_color)
            .map(argb);
        let color = props.get(id, p.color).and_then(Value::as_color).map(argb);
        let entity = if let Some(e) = self.inputs.get(&id) {
            e.clone()
        } else {
            let e = cx.new(|cx| TextInput::new(placeholder.clone(), cx));
            cx.subscribe(&e, move |this, input, ev: &InputEvent, cx| match ev {
                InputEvent::Changed => {
                    let text = input.read(cx).text().to_string();
                    let value = this.change_text(id, text);
                    this.node_event(id, "changeText", value);
                }
                InputEvent::Submit => {
                    this.node_event(id, "submit", Value::Null);
                }
                // `focus` and `blur` carry an empty object, as `lh.l0`
                // sends them.
                InputEvent::Focus => {
                    this.focused = Some(id);
                    this.node_event(id, "focus", Value::Object(vec![]));
                }
            })
            .detach();
            self.inputs.insert(id, e.clone());
            e
        };
        // The field follows the guest's `value` only when the guest changes
        // it, so its own edits stand until the guest says otherwise.
        let changed = self.tree_values.get(&id) != Some(&value);
        if changed {
            self.tree_values.insert(id, value.clone());
        }
        entity.update(cx, |input, cx| {
            input.secure = secure;
            input.set_placeholder(placeholder);
            if let Some(c) = placeholder_color {
                input.placeholder_color = c;
            }
            if let Some(c) = color {
                input.cursor_color = c;
            }
            if changed && input.text() != value {
                input.set_text(value, cx);
            }
        });
        entity
    }
}

/// A piece of a text node's `spans`: its text and the style the Android
/// platform gives it (`props.b.b`, decompiled from the 356207 APK).
#[derive(Debug, Default, PartialEq)]
#[allow(clippy::struct_excessive_bools, reason = "one per span flag")]
pub struct Span {
    pub text: String,
    pub color: Option<u32>,
    pub bg: Option<u32>,
    pub weight: Option<f32>,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// `code`: gg mono, as `props.b.b` sets it on Android 9 and later.
    pub code: bool,
    /// `link`: the link's id, which `linkPress` carries as `url`.
    pub link: Option<String>,
    /// `image`: the span's text is replaced by an image (a custom emoji).
    pub image: Option<SpanImage>,
}

/// A span's `image`: `uri`, `size` in dp (20 by default), `tint` and
/// `radius`.
#[derive(Clone, Debug, PartialEq)]
pub struct SpanImage {
    pub uri: String,
    pub size: f32,
    pub tint: Option<u32>,
    pub radius: f32,
}

/// A `spans` cell: an array of records, each with `text` and optional
/// `color`, `bg`, `bold`, `weight`, `italic`, `underline`, `strike`,
/// `code`, `link` and `image`. `bold` is weight 700. A `link` is underlined
/// only when its span says `underline`.
pub fn spans(v: &Value) -> Vec<Span> {
    let Value::Array(items) = v else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let Value::Object(f) = item else { return None };
            let field = |n: &str| f.iter().find(|(k, _)| k.name() == Some(n)).map(|(_, v)| v);
            let on = |n: &str| field(n) == Some(&Value::Bool(true));
            let text = match field("text") {
                Some(Value::Str(s)) => s.clone(),
                _ => String::new(),
            };
            let weight = if on("bold") {
                Some(700.)
            } else {
                num(field("weight")).filter(|w| *w > 0.)
            };
            let link = match field("link") {
                Some(Value::Str(s)) if !s.is_empty() => Some(s.clone()),
                _ => None,
            };
            let image = match field("image") {
                Some(Value::Object(g)) => {
                    let gf = |n: &str| g.iter().find(|(k, _)| k.name() == Some(n)).map(|(_, v)| v);
                    match gf("uri") {
                        Some(Value::Str(uri)) if !uri.is_empty() => Some(SpanImage {
                            uri: uri.clone(),
                            size: num(gf("size")).unwrap_or(20.),
                            tint: gf("tint").and_then(Value::as_color),
                            radius: num(gf("radius")).unwrap_or(0.),
                        }),
                        _ => None,
                    }
                }
                _ => None,
            };
            Some(Span {
                text,
                color: field("color").and_then(Value::as_color),
                bg: field("bg").and_then(Value::as_color),
                weight,
                italic: on("italic"),
                underline: on("underline"),
                strike: on("strike"),
                code: on("code"),
                link,
                image,
            })
        })
        .collect()
}

/// The base style of a text node's runs: what a span leaves unset.
pub struct RunBase {
    pub family: SharedString,
    pub mono: Option<SharedString>,
    pub weight: f32,
    pub color: u32,
    pub font_size: f32,
}

/// Spans laid out as GPUI text: the string and its runs, each link's byte
/// range and id, and where each image goes. An image span's text is
/// replaced by en spaces as wide as the image, which is drawn over them.
pub struct Laid {
    pub text: SharedString,
    pub runs: Vec<gpui::TextRun>,
    pub links: Vec<(std::ops::Range<usize>, String)>,
    pub images: Vec<(std::ops::Range<usize>, SpanImage)>,
}

pub fn lay_out(spans: Vec<Span>, upper: bool, base: &RunBase) -> Laid {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut links = Vec::new();
    let mut images = Vec::new();
    for s in spans {
        let start = text.len();
        if let Some(img) = &s.image {
            // An en space is half the font size wide.
            let n = (img.size / (base.font_size * 0.5)).ceil().max(1.) as usize;
            text.extend(std::iter::repeat_n('\u{2002}', n));
        } else if upper {
            text.push_str(&s.text.to_uppercase());
        } else {
            text.push_str(&s.text);
        }
        if text.len() == start {
            continue;
        }
        let family = match (&base.mono, s.code) {
            (Some(m), true) => m.clone(),
            _ => base.family.clone(),
        };
        runs.push(gpui::TextRun {
            len: text.len() - start,
            font: gpui::Font {
                family,
                weight: FontWeight(s.weight.unwrap_or(base.weight)),
                style: if s.italic {
                    gpui::FontStyle::Italic
                } else {
                    gpui::FontStyle::Normal
                },
                ..gpui::font("")
            },
            color: argb(s.color.unwrap_or(base.color)),
            background_color: s.bg.map(argb),
            underline: s.underline.then(|| gpui::UnderlineStyle {
                thickness: px(1.),
                ..Default::default()
            }),
            strikethrough: s.strike.then(|| gpui::StrikethroughStyle {
                thickness: px(1.),
                ..Default::default()
            }),
        });
        if let Some(l) = s.link {
            links.push((start..text.len(), l));
        }
        if let Some(img) = s.image {
            images.push((start..text.len(), img));
        }
    }
    Laid {
        text: text.into(),
        runs,
        links,
        images,
    }
}

/// A `backgroundGradient`: a record of `angle` (degrees, 180 from top to
/// bottom), `colors` and `positions`, or a bare array of colours along
/// `backgroundGradientAxis` (`"vertical"`, top to bottom, or across). The
/// angle and each stop as a colour and a position from 0 to 1.
fn gradient_stops(v: &Value, axis: Option<&str>) -> Option<(f32, Vec<(u32, f32)>)> {
    let (angle, colors, positions) = match v {
        Value::Object(f) => {
            let field = |n: &str| f.iter().find(|(k, _)| k.name() == Some(n)).map(|(_, v)| v);
            let list = |n: &str| match field(n) {
                Some(Value::Array(a)) => a.clone(),
                _ => Vec::new(),
            };
            (
                num(field("angle")).unwrap_or(180.),
                list("colors"),
                list("positions"),
            )
        }
        Value::Array(a) => {
            let angle = if axis == Some("vertical") { 180. } else { 90. };
            (angle, a.clone(), Vec::new())
        }
        _ => return None,
    };
    let colors: Vec<u32> = colors.iter().filter_map(Value::as_color).collect();
    let n = colors.len();
    let stops: Vec<(u32, f32)> = colors
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let even = if n > 1 { i as f32 / (n - 1) as f32 } else { 0. };
            (c, num(positions.get(i)).unwrap_or(even))
        })
        .collect();
    (!stops.is_empty()).then_some((angle, stops))
}

/// A gradient as one GPUI background, which has two stops: from the first
/// colour to the next one that differs, at their positions.
fn gradient(v: &Value, axis: Option<&str>) -> Option<gpui::Background> {
    let (angle, stops) = gradient_stops(v, axis)?;
    let first = stops[0];
    let next = stops
        .iter()
        .position(|s| s.0 != first.0)
        .unwrap_or(stops.len() - 1);
    Some(gpui::linear_gradient(
        angle,
        gpui::linear_color_stop(argb(first.0), first.1),
        gpui::linear_color_stop(argb(stops[next].0), stops[next].1),
    ))
}

/// A gradient with more than two stops along a box's side (an angle of 0,
/// 90, 180 or 270), as one layer a pair of stops: GPUI's gradient has two.
/// Each layer covers its stops' share of the box. `None` for any other.
fn gradient_layers(v: &Value, axis: Option<&str>) -> Option<Vec<AnyElement>> {
    let (angle, mut stops) = gradient_stops(v, axis)?;
    if stops.len() <= 2 {
        return None;
    }
    let a = angle.rem_euclid(360.);
    let (vertical, reversed) = match a as i32 {
        180 if a.fract() == 0. => (true, false),
        0 if a.fract() == 0. => (true, true),
        90 if a.fract() == 0. => (false, false),
        270 if a.fract() == 0. => (false, true),
        _ => return None,
    };
    if reversed {
        stops = stops
            .into_iter()
            .rev()
            .map(|(c, at)| (c, 1. - at))
            .collect();
    }
    let mut out = Vec::new();
    for w in stops.windows(2) {
        let ((c0, a0), (c1, a1)) = (w[0], w[1]);
        if a1 <= a0 {
            continue;
        }
        let dir = if vertical { 180. } else { 90. };
        let bg = gpui::linear_gradient(
            dir,
            gpui::linear_color_stop(argb(c0), 0.),
            gpui::linear_color_stop(argb(c1), 1.),
        );
        let mut d = div().absolute().bg(bg);
        d = if vertical {
            d.left_0().right_0().top(relative(a0)).h(relative(a1 - a0))
        } else {
            d.top_0().bottom_0().left(relative(a0)).w(relative(a1 - a0))
        };
        out.push(d.into_any_element());
    }
    Some(out)
}

/// A `shadow` record (`color`, `opacity`, `radius`, `offsetX`,
/// `offsetY`) as GPUI's box shadow.
fn shadow(v: &Value) -> Option<gpui::BoxShadow> {
    let Value::Object(f) = v else { return None };
    let field = |n: &str| f.iter().find(|(k, _)| k.name() == Some(n)).map(|(_, v)| v);
    let mut color = argb(
        field("color")
            .and_then(Value::as_color)
            .unwrap_or(0xff00_0000),
    );
    color.a *= num(field("opacity")).unwrap_or(1.).clamp(0., 1.);
    Some(gpui::BoxShadow {
        color,
        offset: gpui::point(
            px(num(field("offsetX")).unwrap_or(0.)),
            px(num(field("offsetY")).unwrap_or(0.)),
        ),
        blur_radius: px(num(field("radius")).unwrap_or(0.)),
        spread_radius: px(0.),
        inset: false,
    })
}

/// Corner radii, top left, top right, bottom right, bottom left; `None`
/// leaves a corner as it is.
fn set_corners(el: &mut gpui::Stateful<gpui::Div>, corners: [Option<f32>; 4]) {
    let s = &mut el.style().corner_radii;
    for (slot, r) in [
        &mut s.top_left,
        &mut s.top_right,
        &mut s.bottom_right,
        &mut s.bottom_left,
    ]
    .into_iter()
    .zip(corners)
    {
        if let Some(r) = r {
            *slot = Some(px(r).into());
        }
    }
}

/// `"50%"` as 0.5.
fn percent(s: &str) -> Option<f32> {
    s.strip_suffix('%')?
        .trim()
        .parse::<f32>()
        .ok()
        .map(|v| v / 100.)
}

/// A uri as the log shows it: long ones cut short.
fn short(uri: &str) -> String {
    if uri.len() > 80 {
        format!("{}...", uri.chars().take(60).collect::<String>())
    } else {
        uri.to_string()
    }
}

/// A `data:<mime>;base64,<bytes>` image.
fn data_image(data: &str) -> Option<(gpui::ImageFormat, Vec<u8>)> {
    use base64::Engine as _;
    let (head, body) = data.split_once(',')?;
    let mime = head.strip_suffix(";base64")?;
    let format = match mime {
        "image/png" => gpui::ImageFormat::Png,
        "image/jpeg" | "image/jpg" => gpui::ImageFormat::Jpeg,
        "image/webp" => gpui::ImageFormat::Webp,
        "image/gif" => gpui::ImageFormat::Gif,
        "image/bmp" => gpui::ImageFormat::Bmp,
        "image/svg+xml" => gpui::ImageFormat::Svg,
        _ => return None,
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    Some((format, bytes))
}

/// Where a popover of `size` goes in a window of `window`, for an anchor
/// at `anchor` (left, top, right, bottom): see
/// [`PhoenixView::render_popover`]. `inset` is the left and right edge
/// insets.
fn popover_origin(
    anchor: [f32; 4],
    size: (f32, f32),
    window: (f32, f32),
    (placement, align): (&str, &str),
    offset: f32,
    (inset_left, inset): (f32, f32),
) -> (f32, f32) {
    let [left, top, right, bottom] = anchor;
    let (w, h) = size;
    let below = bottom + offset;
    let above = top - offset - h;
    let y = match placement {
        "top" => above,
        "auto" if below + h > window.1 - inset && above >= inset => above,
        "left" | "right" => (top + bottom - h) / 2.,
        _ => below,
    };
    let x = match (placement, align) {
        ("left", _) => left - offset - w,
        ("right", _) => right + offset,
        (_, "start") => left,
        (_, "end") => right - w,
        _ => ((left + right - w) / 2.).clamp(inset_left, (window.0 - inset - w).max(inset_left)),
    };
    (x, y)
}

/// A `stack` layer above the first: over the whole stack, placed by its
/// `gravity` (`top|start` when it has none).
fn layer(child: AnyElement, gravity: Option<&str>) -> AnyElement {
    let g = gravity.unwrap_or("top|start");
    let parts: HashSet<&str> = g.split('|').collect();
    let mut w = div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .flex_col();
    w = if parts.contains("bottom") {
        w.justify_end()
    } else if parts.contains("center") || parts.contains("center_vertical") {
        w.justify_center()
    } else {
        w.justify_start()
    };
    w = if parts.contains("end") || parts.contains("right") {
        w.items_end()
    } else if parts.contains("center") || parts.contains("center_horizontal") {
        w.items_center()
    } else {
        w.items_start()
    };
    w.child(child).into_any_element()
}

/// A press over a scroll that may become a drag: where it went down and
/// was last seen, the vertical and horizontal scroll under it, and the
/// axis once it moved past the touch slop.
struct Drag {
    from: gpui::Point<Pixels>,
    last: gpui::Point<Pixels>,
    vertical: Option<NodeId>,
    horizontal: Option<NodeId>,
    axis: Option<bool>,
}

impl PhoenixView {
    pub fn now(&self) -> f64 {
        self.clock.elapsed().as_secs_f64() * 1000.
    }

    /// What a turn changed, for the rows built in full on the next frame,
    /// and each node's parent after it.
    fn touched(&mut self, turn: &Turn) {
        use rekindle_wire::turn::NodeOp;
        for op in &turn.ops {
            match op {
                NodeOp::Create { node, .. }
                | NodeOp::Update { node, .. }
                | NodeOp::Children { node, .. }
                | NodeOp::ChildEdits { node, .. } => {
                    self.dirty.insert(*node);
                }
                NodeOp::Delete { nodes } => self.dirty.extend(nodes),
                _ => {}
            }
        }
        let tree = &self.tree;
        self.parents.clear();
        for n in tree.nodes() {
            for c in &n.children {
                self.parents.insert(*c, n.id);
            }
        }
        self.clean_rows.retain(|r| tree.node(*r).is_some());
        self.spaced.retain(|r| tree.node(*r).is_some());
    }

    /// The rows the changes since the last frame reached: each changed
    /// node's ancestor whose parent is a list. They are built in full.
    fn dirty_rows(&mut self) {
        let dirty = std::mem::take(&mut self.dirty);
        for mut n in dirty {
            while let Some(&p) = self.parents.get(&n) {
                if self.tree.node(p).is_some_and(|p| p.kind.name() == "list") {
                    self.clean_rows.remove(&n);
                    break;
                }
                n = p;
            }
        }
    }

    /// A row of `list` that this frame may lay out as space: built in full
    /// since it last changed, laid out at the last frame, and more than a
    /// quarter of a screen from the rows shown. The space it takes, with
    /// its margins. The frame's offset is already stepped, and a shift the
    /// layout makes moves the rows with it, so the margin only has to
    /// cover a `scrollTo`.
    fn spacer_len(&self, port: &Port, row: NodeId) -> Option<f32> {
        if !self.clean_rows.contains(&row) || port.horizontal {
            return None;
        }
        let at = port.laid.iter().position(|l| l.node == row)?;
        let l = port.laid[at];
        let lo = port.scroller.pos - 0.25 * port.len;
        let hi = port.scroller.pos + 1.25 * port.len;
        if l.len <= 0. || port.len <= 0. || (l.start < hi && l.start + l.len > lo) {
            return None;
        }
        Some(l.len)
    }

    /// The guest's `ScrollTo` ops (op 7) on lists, kept for the next
    /// layout, which places the row they name. Android's `gq` reads
    /// `index`, `align`, `offset`, `endOffset`, `topOnly` and `animated`.
    fn scroll_requests(&mut self, turn: &Turn) {
        for op in &turn.ops {
            let rekindle_wire::turn::NodeOp::ScrollTo { node, value } = op else {
                continue;
            };
            let Value::Object(fields) = value else {
                continue;
            };
            let field = |n: &str| {
                fields
                    .iter()
                    .find(|(k, _)| k.name() == Some(n))
                    .map(|(_, v)| v)
            };
            let text = |n: &str| match field(n) {
                Some(Value::Str(s)) => Some(s.as_str()),
                _ => None,
            };
            let index = num(field("index")).unwrap_or(0.) as i32;
            let offset = num(field("offset")).unwrap_or(0.);
            let align = match text("align") {
                Some("center") => Align::Center,
                Some("end") => Align::End,
                Some("visible") => Align::Visible {
                    end_offset: num(field("endOffset")).unwrap_or(0.),
                    top_only: field("topOnly") == Some(&Value::Bool(true)),
                },
                _ => Align::Start,
            };
            let animated = field("animated") == Some(&Value::Bool(true)) && self.motion.enabled;
            let horizontal = self
                .tree
                .node(*node)
                .is_some_and(|n| n.kind.name() == "hscroll");
            let stick = self
                .tree
                .props()
                .get(*node, PropId::from_name("stickToEnd").unwrap())
                == Some(&Value::Bool(true));
            crate::guest::log(&format!(
                "[view] scrollTo #{node}: position {index}, {align:?}, offset {offset}{}",
                if animated { ", animated" } else { "" }
            ));
            self.ports
                .entry(*node)
                .or_insert_with(|| Rc::new(RefCell::new(Port::new(horizontal, stick))))
                .borrow_mut()
                .requests
                .push(Request {
                    index,
                    align,
                    offset,
                    animated,
                });
        }
    }

    /// Each fling, eased scroll and stretch one frame on, and another frame
    /// asked for while any still moves. The prepaint applies the offsets.
    fn scroll_frame(&mut self, window: &mut Window) {
        let now = self.now();
        let mut moving = false;
        for (id, port) in &self.ports {
            let mut port = port.borrow_mut();
            // A `scrollTo` waits for the port's next layout. A port the
            // last frame didn't draw, on a hidden screen, has none coming
            // until a turn shows it.
            if !port.requests.is_empty() && self.drawn.contains(id) {
                moving = true;
            }
            if !port.scroller.active() {
                continue;
            }
            let (max, len) = (port.max, port.len);
            let pos = port.scroller.step(max, len, now);
            moving |= port.scroller.active();
            if crate::scrollport::logging() {
                crate::guest::log(&format!(
                    "[scroll] frame at {now:.1} ms: offset {pos:.1}, stretch {:.1}",
                    port.scroller.stretch()
                ));
            }
        }
        if moving {
            if frame_logging() {
                crate::guest::log("[frame] next: scroll");
            }
            window.request_animation_frame();
        }
        // `scrollSettle` when a scroll comes to rest, as the Android
        // platform's scroll listener sends it when the `RecyclerView`'s
        // scroll state goes idle (`eq.a`). The guest holds a page of older
        // messages until then.
        let dragged = self.drag.as_ref().and_then(|d| match d.axis {
            Some(true) => d.horizontal,
            Some(false) => d.vertical,
            None => None,
        });
        let mut settled = Vec::new();
        for (id, port) in &self.ports {
            let mut port = port.borrow_mut();
            let now_moving = port.scroller.active() || dragged == Some(*id);
            if port.was_moving && !now_moving {
                settled.push(*id);
            }
            port.was_moving = now_moving;
        }
        for id in settled {
            if self.node_event(id, "scrollSettle", Value::Object(vec![])) {
                crate::guest::log(&format!("[view] list #{id}: scrollSettle"));
            }
        }
    }

    /// The innermost vertical and horizontal scroll under `at` that has
    /// something to scroll, from the scrolls the last paint drew.
    fn scrolls_at(&self, at: gpui::Point<Pixels>) -> (Option<NodeId>, Option<NodeId>) {
        let (mut v, mut h) = (None, None);
        for id in self.scroll_order.borrow().iter().rev() {
            let Some(port) = self.ports.get(id) else {
                continue;
            };
            let port = port.borrow();
            if !port.bounds.contains(&at) || port.max <= 0. {
                continue;
            }
            if port.horizontal {
                h.get_or_insert(*id);
            } else {
                v.get_or_insert(*id);
            }
        }
        (v, h)
    }

    /// The window's wheel and touchpad scrolls, taken before GPUI's own
    /// scroll handling sees them. A wheel's notches fling the scroll under
    /// the pointer, a precise scroll moves it directly, since the platform
    /// adds its own momentum, and either stretches an end it runs into.
    fn on_wheel(&mut self, ev: &ScrollEvent, window: &mut Window) -> bool {
        let (v, h) = self.scrolls_at(ev.position);
        let (dx, dy, precise) = match ev.delta {
            ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y), true),
            // A notch is three lines on Windows' default settings.
            ScrollDelta::Lines(l) => (
                l.x * crate::scroller::WHEEL_NOTCH / 3.,
                l.y * crate::scroller::WHEEL_NOTCH / 3.,
                false,
            ),
        };
        // Toward the content's end is a negative delta, as GPUI's offset
        // goes. A vertical turn over a horizontal scroll with nothing
        // vertical around it scrolls it sideways.
        let (target, delta) = if dx.abs() > dy.abs() {
            (h, -dx)
        } else {
            (v.or(h), -dy)
        };
        let Some(port) = target.and_then(|id| self.ports.get(&id)) else {
            return false;
        };
        let now = self.now();
        let mut port = port.borrow_mut();
        let (max, len) = (port.max, port.len);
        if precise {
            if ev.touch_phase == TouchPhase::Ended {
                port.scroller.precise_end(now);
            } else {
                port.scroller.precise(delta, max, len, now);
            }
        } else {
            port.scroller.wheel(delta, max, len, now);
        }
        window.refresh();
        true
    }

    /// A left press over a scroll, outside a text field, may start a drag.
    /// It stops a fling under it, as a touch does on Android.
    fn on_press(&mut self, ev: &MouseDownEvent) {
        self.drag = None;
        if ev.button != MouseButton::Left {
            return;
        }
        let on_field = {
            let hits = self.hits.borrow();
            self.inputs
                .keys()
                .any(|n| hits.get(n).is_some_and(|b| b.contains(&ev.position)))
        };
        let (vertical, horizontal) = self.scrolls_at(ev.position);
        if on_field || (vertical.is_none() && horizontal.is_none()) {
            return;
        }
        let now = self.now();
        for id in [vertical, horizontal].into_iter().flatten() {
            if let Some(port) = self.ports.get(&id) {
                let mut port = port.borrow_mut();
                let pos = port.scroller.pos;
                port.scroller.press(pos, now);
            }
        }
        self.drag = Some(Drag {
            from: ev.position,
            last: ev.position,
            vertical,
            horizontal,
            axis: None,
        });
    }

    /// The pointer moved with the button held: once past the touch slop
    /// along an axis a scroll takes, the content follows the pointer, and
    /// the press it began as is let go.
    fn on_pointer_move(&mut self, ev: &MouseMoveEvent, window: &mut Window) {
        let Some(drag) = &mut self.drag else { return };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        if drag.axis.is_none() {
            let d = ev.position - drag.from;
            let slop = px(crate::scroller::TOUCH_SLOP);
            if drag.vertical.is_some() && d.y.abs() > slop && d.y.abs() >= d.x.abs() {
                drag.axis = Some(false);
            } else if drag.horizontal.is_some() && d.x.abs() > slop {
                drag.axis = Some(true);
            } else if drag.vertical.is_some() && d.y.abs() > slop {
                drag.axis = Some(false);
            } else {
                return;
            }
            drag.last = ev.position;
            self.motion.press(Vec::new());
            return;
        }
        let horizontal = drag.axis == Some(true);
        let d = ev.position - drag.last;
        drag.last = ev.position;
        let delta = f32::from(if horizontal { d.x } else { d.y });
        let Some(id) = (if horizontal {
            drag.horizontal
        } else {
            drag.vertical
        }) else {
            return;
        };
        let now = self.now();
        let Some(port) = self.ports.get(&id) else {
            return;
        };
        let mut port = port.borrow_mut();
        let (max, len) = (port.max, port.len);
        port.scroller.drag(delta, max, len, now);
        window.refresh();
    }

    /// The button came up: a drag flings at the pointer's velocity, and
    /// isn't a click.
    fn on_release(&mut self, window: &mut Window) -> bool {
        let Some(drag) = self.drag.take() else {
            return false;
        };
        let Some(horizontal) = drag.axis else {
            return false;
        };
        let id = if horizontal {
            drag.horizontal
        } else {
            drag.vertical
        };
        let now = self.now();
        let density = window.scale_factor();
        if let Some(port) = id.and_then(|id| self.ports.get(&id)) {
            port.borrow_mut().scroller.lift(density, now);
        }
        window.refresh();
        true
    }

    /// An element that registers the scroll and drag listeners, in the
    /// capture phase, ahead of every node's. A wheel turn or a drag a
    /// scroll took stops there, so GPUI's own scroll and the click a drag
    /// would end in never see it.
    fn pointer_listeners(cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        canvas(
            |_, _, _| {},
            move |_, (), window, _| {
                let v = view.clone();
                window.on_mouse_event(move |ev: &ScrollEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let used = v
                        .update(cx, |this, cx| {
                            let used = this.on_wheel(ev, window);
                            if used {
                                cx.notify();
                            }
                            used
                        })
                        .unwrap_or(false);
                    if used {
                        cx.stop_propagation();
                    }
                });
                let v = view.clone();
                window.on_mouse_event(move |ev: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = v.update(cx, |this, _| this.on_press(ev));
                    }
                });
                let v = view.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = v.update(cx, |this, cx| {
                            if this.drag.is_some() {
                                this.on_pointer_move(ev, window);
                                cx.notify();
                            }
                        });
                    }
                });
                let v = view.clone();
                window.on_mouse_event(move |_: &MouseUpEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let dragged = v
                        .update(cx, |this, cx| {
                            let dragged = this.on_release(window);
                            if dragged {
                                cx.notify();
                            }
                            dragged
                        })
                        .unwrap_or(false);
                    if dragged {
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }
}

impl Focusable for PhoenixView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PhoenixView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let began = std::time::Instant::now();
        self.built = 0;
        self.sync_window(window);
        self.sync_frame(window, cx);
        if frame_logging() {
            crate::guest::log(&format!(
                "[frame] at {:.1} ms, {} turns",
                self.now(),
                self.stats.turns.len()
            ));
        }
        self.scroll_order.borrow_mut().clear();
        self.scroll_frame(window);
        self.motion_frame(window, cx);
        self.list_events();
        self.layout_events();
        self.blur_events(window, cx);
        self.dirty_rows();
        self.rows.clear();
        let mut root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::rgb(0x0012_1214))
            .text_color(gpui::white())
            .track_focus(&self.focus)
            // A release anywhere ends the press, before the click it may
            // make stops the event.
            .capture_any_mouse_up(cx.listener(|this, _, _, cx| {
                this.motion.press(Vec::new());
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, _| {
                if ev.keystroke.key == "escape" {
                    this.back();
                }
            }));
        if let Some(f) = &self.fonts.sans {
            root = root.font_family(f.clone());
        }
        root = root.child(Self::pointer_listeners(cx));
        self.drawn.clear();
        if let Some(r) = self.tree.root() {
            self.painted.clear();
            self.popovers.clear();
            root = root.child(self.render_node(r, Parent::Layer, window, cx));
            // A popover in a popover is met while the first is drawn.
            while !self.popovers.is_empty() {
                let pop = self.popovers.remove(0);
                root = root.child(self.render_popover(pop, window, cx));
            }
        } else {
            let msg = match &self.stopped {
                Some(why) => format!("The guest stopped: {why}"),
                None => "Waiting for the guest's first turn".to_string(),
            };
            root = root
                .items_center()
                .justify_center()
                .child(SharedString::from(msg));
        }
        // The frame's cost, taken when the last thing in it is painted.
        let cost = FrameCost {
            at: self.now(),
            render: began.elapsed().as_secs_f64() * 1000.,
            total: 0.,
            nodes: self.built,
            scrolling: self.ports.values().any(|p| p.borrow().scroller.active())
                || self.drag.is_some(),
        };
        let frames = self.stats.frames.clone();
        root = root.child(
            canvas(
                |_, _, _| {},
                move |_, (), _, _| {
                    let mut f = frames.borrow_mut();
                    if f.len() >= 4096 {
                        f.drain(..2048);
                    }
                    f.push(FrameCost {
                        total: began.elapsed().as_secs_f64() * 1000.,
                        ..cost
                    });
                },
            )
            .absolute()
            .size_0(),
        );
        if let (Some(why), Some(_)) = (&self.stopped, self.tree.root()) {
            root = root.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .p_2()
                    .bg(gpui::hsla(0., 0.7, 0.3, 0.9))
                    .child(SharedString::from(format!("The guest stopped: {why}"))),
            );
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(fields: &[(&str, Value)]) -> Value {
        Value::Object(
            fields
                .iter()
                .filter_map(|(n, v)| Some((PropId::from_name(n)?, v.clone())))
                .collect(),
        )
    }

    /// A DM preview's `spans`, as the guest sent it, and a message body's
    /// pieces with their styles.
    #[test]
    fn spans_keep_text_and_style() {
        let v = Value::Array(vec![
            record(&[("text", Value::Str("JohnWowHaha: ".into()))]),
            record(&[
                ("text", Value::Str("https://example.com".into())),
                ("color", Value::I64(0xff00_a8fc)),
                ("underline", Value::Bool(true)),
            ]),
        ]);
        let s = spans(&v);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].text, "JohnWowHaha: ");
        assert_eq!(s[0].color, None);
        assert_eq!(s[1].color, Some(0xff00_a8fc));
        let joined: String = s.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "JohnWowHaha: https://example.com");
        assert!(spans(&Value::Null).is_empty());
    }

    #[test]
    fn spans_lay_out_code_links_and_images() {
        let img = Value::Object(vec![
            (
                PropId::from_name("uri").unwrap(),
                Value::Str("https://cdn.discordapp.com/emojis/1.webp".into()),
            ),
            (PropId::from_name("size").unwrap(), Value::F64(17.5)),
        ]);
        let v = Value::Array(vec![
            record(&[
                ("text", Value::Str("a ".into())),
                ("code", Value::Bool(true)),
            ]),
            record(&[("text", Value::Str(":spotify:".into())), ("image", img)]),
            record(&[
                ("text", Value::Str("site".into())),
                ("link", Value::Str("0".into())),
            ]),
        ]);
        let base = RunBase {
            family: "gg sans".into(),
            mono: Some("gg mono".into()),
            weight: 400.,
            color: 0xffff_ffff,
            font_size: 16.,
        };
        let laid = lay_out(spans(&v), false, &base);
        // 17.5 dp at 8 dp an en space: three of them, three bytes each.
        assert_eq!(laid.text.as_ref(), "a \u{2002}\u{2002}\u{2002}site");
        assert_eq!(laid.runs[0].font.family.as_ref(), "gg mono");
        assert_eq!(laid.runs[1].font.family.as_ref(), "gg sans");
        assert_eq!(laid.images[0].0, 2..11);
        assert_eq!(laid.links, vec![(11..15, "0".to_string())]);
    }

    #[test]
    fn popovers_sit_by_their_anchor() {
        let anchor = [100., 50., 200., 80.];
        let win = (412., 860.);
        let at = |pa: (&str, &str), offset: f32| {
            popover_origin(anchor, (300., 120.), win, pa, offset, (12., 12.))
        };
        // Centred under the anchor, it would start at 0: the left inset
        // holds it at 12.
        assert_eq!(at(("bottom", "center"), 8.), (12., 88.));
        assert_eq!(at(("top", "start"), 4.), (100., -74.));
        assert_eq!(at(("bottom", "end"), 0.), (-100., 80.));
        // Centred where it fits.
        let small = popover_origin(
            anchor,
            (60., 20.),
            win,
            ("bottom", "center"),
            0.,
            (12., 12.),
        );
        assert_eq!(small, (120., 80.));
    }

    #[test]
    fn gradients_past_two_stops_draw_as_layers() {
        let g = |angle: f64| {
            record(&[
                ("angle", Value::F64(angle)),
                (
                    "colors",
                    Value::Array(vec![
                        Value::I32(0x0024_2429),
                        Value::I32(0x1a24_2429),
                        Value::I64(0xcc24_2429),
                        Value::I64(0xff24_2429),
                    ]),
                ),
                (
                    "positions",
                    Value::Array(vec![
                        Value::F64(0.),
                        Value::F64(0.33),
                        Value::F64(0.76),
                        Value::F64(1.),
                    ]),
                ),
            ])
        };
        assert_eq!(gradient_layers(&g(180.), None).map(|l| l.len()), Some(3));
        assert_eq!(gradient_layers(&g(270.), None).map(|l| l.len()), Some(3));
        assert!(gradient_layers(&g(45.), None).is_none());
        let (_, stops) = gradient_stops(&g(180.), None).unwrap();
        assert_eq!(stops[2], (0xcc24_2429, 0.76));
    }
}
