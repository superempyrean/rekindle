//! Drive mode (test harness): rekindle exercises its own UI in a live client
//! the way a user would, with no OS input. Events go into the client's own
//! `Window::dispatch_event` (its compiled function, at its anchor) on its UI
//! thread, from a next-frame callback so never mid-draw; results are read off
//! the frames the client's gpui actually drew. The host starts it ([`start`])
//! and reports [`result`].
//!
//! The script (every surface here is real elements, `rekindle-native`):
//! - **toast:** raise one; find its card and the card's close button in the
//!   finished scene; move the pointer onto the button -> gpui repaints it in
//!   its hover colour; click -> only that toast is dismissed (gpui routed the
//!   mouse-down to rekindle's listener, which asked gpui's
//!   `Hitbox::is_hovered`).
//! - **chat-bar button:** registered in the client's chat-bar registry, it is
//!   the client's own button, last in the client's row (rekindle's order sorts
//!   after the client's): hover -> the client paints its hover background and
//!   shows its tooltip; click -> the client calls the button's action
//!   (rekindle's closure), the plugin opens its modal, and the client renders
//!   rekindle's modal body. (Framework layer: rekindle's own button, left of
//!   the client's, with rekindle's hover colour and group-hover tooltip.)
//! - **modal dismiss:** the client's `chat_client::Dismiss` action, by name
//!   (what its Escape binding dispatches) -> the client asks rekindle to
//!   dismiss it, and the plugin closes it.
//! - **context menu:** right-click the first member row -> the client's menu
//!   (its own `build`, rekindle's group last); click rekindle's row -> the
//!   client calls rekindle's `invoke`, the plugin copies the user's URL (to
//!   the drive, never the OS clipboard) and toasts.
//! - **profile:** click the member row -> the client opens the profile and
//!   tells rekindle (`opened`); the plugin's section is rendered in it.
//!   `chat_client::Dismiss` closes it.
//! - **rekindle's window** (framework layer only): click rekindle's own
//!   chat-bar button -> rekindle's window opens with its plugin grid; set
//!   the body's scroll position -> gpui moves the grid by it (a wheel event
//!   would set it in gpui's own listener); click the scrim -> it closes;
//!   Ctrl+Shift+, -> it opens; Escape -> it closes.
//! - **settings:** the client's `chat_client::OpenUserSettings` action, by
//!   name, opens settings; click the
//!   last sidebar row (rekindle's Plugins page, spliced into the client's
//!   sidebar) -> the client renders rekindle's page, a grid of plugin
//!   cards; click a card -> its page, with the plugin's own panel under it;
//!   back and on to the next card until a panel has a text field; click
//!   the field -> gpui focuses it; type -> the field (and the plugin) get
//!   the text; Enter -> focus leaves.
//!
//! The store script ([`start_store`]) runs instead, in a session that uses a
//! home and the release channel: open settings and rekindle's page, press
//! the Community tab, press the first card's install button, and press
//! "Install" on its review screen. The host checks what was installed.
//! If the installed plugin's panel has a colour picker on screen, it then
//! drags across the picker's square and presses its hue bar, and checks the
//! plugin showed each colour back.

use std::sync::Mutex;

use rekindle_native::client::{self, FrameRecord};

/// The client's live `Window`, on its UI thread: what every read of it here
/// needs. Only [`observe`] makes one, and its caller guarantees that.
#[derive(Clone, Copy)]
struct Live(*const u8);

impl Live {
    fn frame(self) -> Option<FrameRecord> {
        // SAFETY: the type's invariant.
        unsafe { client::frame(self.0) }
    }

    fn viewport(self) -> Option<(f32, f32)> {
        // SAFETY: the type's invariant.
        unsafe { client::viewport(self.0) }
    }

    fn scale_factor(self) -> Option<f32> {
        // SAFETY: the type's invariant.
        unsafe { client::scale_factor(self.0) }
    }

    fn root_view_type(self) -> Option<String> {
        // SAFETY: the type's invariant.
        unsafe { client::root_view_type(self.0) }
    }

    fn focused_field(self) -> Option<String> {
        // SAFETY: the type's invariant.
        unsafe { crate::panel::focused_field(self.0) }
    }
}

/// A filled quad of the frame: logical bounds (x, y, w, h) and its HSLA.
pub(crate) type FrameQuad = ((f32, f32, f32, f32), [f32; 4]);
type Rect = (f32, f32, f32, f32);

const TOAST_TEXT: &str = "rekindle drive: toast";
/// Colours as rekindle sets them.
use crate::toast::{CARD, CLOSE, CLOSE_HOVER};
const BUTTON_HOVER: u32 = 0xffffff1a;
const TOOLTIP: u32 = 0x111214f0;
const FIELD_BG: u32 = 0xffffff14;
/// A panel button's fill.
const PANEL_BUTTON: u32 = 0x5865f6ff;
/// rekindle's window's fill, and how far the script scrolls its body.
const WINDOW_BG: u32 = 0x313338ff;
const WINDOW_SCROLL: f32 = 120.0;
/// The glyphs of text-replace's "Add rule" button (its non-space characters).
const ADD_RULE_GLYPHS: usize = 7;
/// The glyphs of rekindle's panel buttons the scripts press or tell views
/// apart by: a plugin page's "Back", and the review screen's "Install" and
/// "Cancel".
const BACK_GLYPHS: usize = 4;
const INSTALL_GLYPHS: usize = 7;
const CANCEL_GLYPHS: usize = 6;
/// The grid: an unselected tab's fill and a card's, as rekindle sets them,
/// and the Community tab's place in the row.
use crate::panel::{CARD_BG as PLUGIN_CARD, CARD_HOVER as PLUGIN_CARD_HOVER, TAB_BG};
const COMMUNITY_TAB: usize = 2;
/// Frames the store script waits for the channel or a download.
const STORE_TIMEOUT: u32 = 900;
/// Times it reopens the store while the first update check is still running.
const STORE_TRIES: u32 = 10;
const TYPED: &str = "xy";
/// What the drive searches the kaomoji picker for.
const SEARCH: &str = "bear";
const SETTLE_FRAMES: u32 = 20;
const STEP_TIMEOUT: u32 = 90;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    Settle,
    AwaitToast,
    CheckToastHover,
    CheckToastClick,
    AwaitButton,
    CheckButtonHover,
    CheckButtonClick,
    CheckModalDismissed,
    AwaitMemberRow,
    AwaitMenu,
    CheckMenuClick,
    CheckProfile,
    CloseProfile,
    AwaitSettings,
    AwaitPluginsPage,
    CheckFieldFocused,
    CheckTyped,
    CheckBlurred,
    /// Back from settings to the chat (the client focuses its composer).
    LeaveSettings,
    /// It clicked the chat-bar button again: the kaomoji picker opens.
    AwaitPicker,
    /// It clicked the picker's search field: type into it.
    TypeSearch,
    /// The search narrowed the faces: click one.
    AwaitFaces,
    /// The face went into the composer, through its input handler.
    CheckInserted,
    /// The inspector script: the pointer is over an element.
    CheckInspector,
    /// It clicked the element in inspect mode.
    CheckSelected,
    /// The store script: rekindle's page is up, press the Community tab.
    StoreBrowse,
    /// The index is listed: press the first card's install button.
    StoreList,
    /// The download finished and its review screen is up: press "Install".
    StoreReview,
    /// Back on the main view.
    StoreInstalled,
    /// The plugin's panel is up: drag in its colour picker's square.
    StorePicker,
    /// It dragged: the plugin shows the colour under the drag's end.
    StoreDragged,
    /// It pressed the hue bar: the plugin shows the new hue.
    StoreHue,
    /// Framework layer: it clicked rekindle's own chat-bar button.
    CheckWindowOpen,
    /// It scrolled the window's body.
    CheckWindowScroll,
    /// It clicked the scrim.
    CheckWindowClosed,
    /// It pressed the window's shortcut.
    CheckShortcutOpened,
    /// It pressed Escape.
    CheckEscapeClosed,
    Done,
}

#[derive(Clone, Debug)]
enum Act {
    Move(f32, f32),
    Click(f32, f32),
    /// Press at the first point, move to the second through the middle,
    /// and release there.
    Drag((f32, f32), (f32, f32)),
    RightClick(f32, f32),
    /// Key name, text typed, control held, shift held.
    Key(&'static str, &'static str, bool, bool),
    Type(&'static str),
    /// A client action, by its registered name (string-stable: no key
    /// binding or coordinates involved).
    Action(&'static str),
}

struct Drive {
    step: Step,
    frames: u32,
    /// Frames spent in the current step.
    since: u32,
    target: Option<Rect>,
    pending: Option<Act>,
    /// Toasts other than the script's, when it clicked.
    others: usize,
    clicks_before: usize,
    renders_before: usize,
    field: Option<String>,
    field_before: String,
    /// Filled rects of the frame the button was found in: what hovering it
    /// adds is the client's hover background and tooltip.
    before: Vec<Rect>,
    /// The glyphs inside the button, and their colours, when it was found.
    glyphs_before: Vec<(Rect, [f32; 4])>,
    /// The member row the menu and profile steps use.
    row: Option<Rect>,
    /// It pressed text-replace's "Add rule" for a rule's text fields.
    added_rule: bool,
    /// rekindle's chat-bar button (the last plugin's: the kaomoji picker).
    button: Option<Rect>,
    /// Dismisses sent to get from settings back to the chat.
    dismisses: u32,
    /// The client surfaces (modal, settings) are installed: script them too.
    full: bool,
    /// Run the inspector script instead.
    inspect: bool,
    /// Run the store script instead.
    store: bool,
    /// Times the store script found the store empty and went back.
    store_tries: u32,
    /// The card of the grid the script opens next.
    card: usize,
    /// A frame had a sprite's image (or painted quads) where the sprite was.
    sprite_imaged: bool,
    sprite_painted: bool,
    results: Vec<(&'static str, String)>,
}

static DRIVE: Mutex<Option<Drive>> = Mutex::new(None);
/// What a plugin copied while the drive ran (instead of the OS clipboard).
static COPIED: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn copied(text: String) {
    *COPIED.lock().unwrap() = Some(text);
}

/// Start the script; it runs from the next frames on.
pub fn start() {
    start_with(true);
}

/// Start the script for a client whose client-function surfaces are off
/// (framework layer only): toasts and chat-bar buttons, no modal or settings.
pub fn start_framework_only() {
    start_with(false);
}

/// Start the inspector script: hover a control and check the inspector
/// picked it and drew its border and panel.
pub fn start_inspect() {
    start_with(false);
    if let Some(d) = DRIVE.lock().unwrap().as_mut() {
        d.inspect = true;
    }
}

/// Start the store script: install the first plugin of the community index
/// through rekindle's panel.
pub fn start_store() {
    start_with(true);
    if let Some(d) = DRIVE.lock().unwrap().as_mut() {
        d.store = true;
    }
}

fn start_with(full: bool) {
    *DRIVE.lock().unwrap() = Some(Drive {
        step: Step::Settle,
        frames: 0,
        since: 0,
        target: None,
        pending: None,
        others: 0,
        clicks_before: 0,
        renders_before: 0,
        field: None,
        field_before: String::new(),
        before: Vec::new(),
        glyphs_before: Vec::new(),
        row: None,
        added_rule: false,
        button: None,
        dismisses: 0,
        full,
        inspect: false,
        store: false,
        store_tries: 0,
        card: 0,
        sprite_imaged: false,
        sprite_painted: false,
        results: Vec::new(),
    });
}

/// Where the script is (diagnostics): step, frames, frames in the step.
pub fn status() -> String {
    match DRIVE.try_lock().as_deref() {
        Ok(Some(d)) => format!("{:?}/{}/{}", d.step, d.frames, d.since),
        Ok(None) => "off".into(),
        Err(_) => "locked".into(),
    }
}

/// Whether a script was started in this client, finished or not.
pub(crate) fn started() -> bool {
    DRIVE.lock().unwrap().is_some()
}

pub(crate) fn active() -> bool {
    DRIVE
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|d| d.step != Step::Done)
}

/// `key=value` results once the script has finished, else `None`.
pub fn result() -> Option<String> {
    let g = DRIVE.lock().unwrap();
    let d = g.as_ref()?;
    let mut out: Vec<String> = d.results.iter().map(|(k, v)| format!("{k}={v}")).collect();
    // Registries rekindle stayed out of because the client's own entries
    // didn't match the layout.
    {
        let refused = crate::registry::refused();
        if !refused.is_empty() {
            out.push(format!(
                "registries_refused={}",
                refused
                    .iter()
                    .map(|(r, why)| format!("{r}({})", why.replace(' ', "_")))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
    }
    // What the plugins' renderers put in the client's message rows.
    let (badges, highlights) = crate::render::written();
    if !badges.is_empty() {
        out.push(format!("row_badges={}", badges.join(",")));
    }
    out.push(format!("row_highlights={highlights}"));
    (d.step == Step::Done).then(|| out.join(" "))
}

fn hsla(rgba: u32) -> [f32; 4] {
    rekindle_native::Rgba::hex(rgba).to_hsla()
}

fn same_colour(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 2e-3)
}

fn same_rect(a: Rect, b: Rect) -> bool {
    (a.0 - b.0).abs() < 1.0
        && (a.1 - b.1).abs() < 1.0
        && (a.2 - b.2).abs() < 1.0
        && (a.3 - b.3).abs() < 1.0
}

/// The tooltip of rekindle's last chat-bar button: the one last in the
/// client's row.
fn offered_tooltip() -> Option<String> {
    crate::registry::buttons().last().map(|b| b.tooltip.clone())
}

/// Glyphs rekindle's tooltip text needs: its non-space characters.
fn tooltip_want() -> usize {
    offered_tooltip().map_or(0, |t| t.chars().filter(|c| !c.is_whitespace()).count())
}

/// Each new card-sized filled surface off `target` (not in `before`) and the
/// glyphs painted over it: inside it and later in gpui's draw order.
fn tooltip_glyphs(window: Live, target: Rect, before: &[Rect]) -> Vec<(Rect, usize)> {
    let Some(f) = window.frame() else {
        return Vec::new();
    };
    f.quads
        .iter()
        .zip(&f.quad_orders)
        .filter(|((r, c), _)| {
            c.is_some_and(|c| c[3] != 0.0)
                && !contains(target, *r)
                && r.2 > 20.0
                && r.3 > 14.0
                && r.3 < 60.0
                && !before.iter().any(|b| same_rect(*b, *r))
        })
        .map(|((r, _), order)| {
            let over = f
                .glyphs
                .iter()
                .filter(|(g, o)| {
                    o > order && contains(*r, (g.0 + g.2 / 2.0, g.1 + g.3 / 2.0, 0.0, 0.0))
                })
                .count();
            (*r, over)
        })
        .collect()
}

/// The quads painted this frame with a border of `border` (HSLA).
fn quads_bordered(window: Live, border: [f32; 4]) -> Vec<Rect> {
    let Some(f) = window.frame() else {
        return Vec::new();
    };
    f.quads
        .iter()
        .zip(&f.quad_borders)
        .filter(|(_, b)| same_colour(**b, border))
        .map(|((r, _), _)| *r)
        .collect()
}

/// The glyphs painted inside `r` this frame, with their colours.
fn glyphs_in(window: Live, r: Rect) -> Vec<(Rect, [f32; 4])> {
    let Some(f) = window.frame() else {
        return Vec::new();
    };
    f.glyphs
        .iter()
        .zip(&f.glyph_colors)
        .filter(|((g, _), _)| contains(r, *g))
        .map(|((g, _), c)| (*g, *c))
        .collect()
}

/// Whether `outer` contains `inner` (to half a pixel).
fn contains(outer: Rect, inner: Rect) -> bool {
    inner.0 >= outer.0 - 0.5
        && inner.1 >= outer.1 - 0.5
        && inner.0 + inner.2 <= outer.0 + outer.2 + 0.5
        && inner.1 + inner.3 <= outer.1 + outer.3 + 0.5
}

fn centre(r: Rect) -> (f32, f32) {
    (r.0 + r.2 / 2.0, r.1 + r.3 / 2.0)
}

fn filled(quads: &[FrameQuad], colour: u32) -> impl Iterator<Item = Rect> + '_ {
    quads
        .iter()
        .filter(move |(_, c)| same_colour(*c, hsla(colour)))
        .map(|(r, _)| *r)
}

/// The grid's tabs, left to right, and which is selected: the unselected
/// ones by their fill, and the selected one, a button-coloured pill level
/// with them.
fn tabs(quads: &[FrameQuad]) -> Vec<(Rect, bool)> {
    let off: Vec<Rect> = filled(quads, TAB_BG).collect();
    let Some(first) = off.first().copied() else {
        return Vec::new();
    };
    let on = filled(quads, PANEL_BUTTON)
        .filter(|r| (r.1 - first.1).abs() < 1.0 && (r.3 - first.3).abs() < 1.0);
    let mut all: Vec<(Rect, bool)> = off
        .iter()
        .map(|r| (*r, false))
        .chain(on.map(|r| (r, true)))
        .collect();
    all.sort_by(|a, b| a.0.0.total_cmp(&b.0.0));
    all
}

/// The grid's cards in reading order.
fn cards(quads: &[FrameQuad]) -> Vec<Rect> {
    // The card under the pointer has its hover fill.
    let mut all: Vec<Rect> = filled(quads, PLUGIN_CARD)
        .chain(filled(quads, PLUGIN_CARD_HOVER))
        .filter(|r| r.2 > 80.0 && r.3 > 40.0)
        .collect();
    all.sort_by(|a, b| {
        (a.1, a.0)
            .partial_cmp(&(b.1, b.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    all
}

/// Whether the plugins' buttons are rekindle's own, in the framework layer
/// (else they are the client's, from its chat-bar registry).
fn framework_buttons() -> bool {
    crate::chat_bar::framework_buttons_on()
}

/// rekindle's button among the client's own chat-bar buttons, found by the
/// row's shape: the last, as rekindle's order sorts after the client's.
fn client_button(window: Live, filled: &[Rect]) -> Option<Rect> {
    use crate::discovery::{Rect as R, find_chat_bar};
    let frame = window.frame()?;
    let viewport = window.viewport()?;
    let r = |r: &Rect| R {
        x: r.0,
        y: r.1,
        w: r.2,
        h: r.3,
    };
    let hitboxes: Vec<R> = frame.hitboxes.iter().map(r).collect();
    let filled: Vec<R> = filled.iter().map(r).collect();
    let bar = find_chat_bar(&hitboxes, &filled, viewport)?;
    bar.buttons
        .iter()
        .max_by(|a, b| a.x.total_cmp(&b.x))
        .map(|b| (b.x, b.y, b.w, b.h))
}

/// The composer: the widest hitbox level with the chat-bar button `b` and
/// ending left of it.
fn composer_of(window: Live, b: Rect) -> Option<Rect> {
    let f = window.frame()?;
    let cy = b.1 + b.3 / 2.0;
    f.hitboxes
        .iter()
        .copied()
        .filter(|r| r.1 <= cy && r.1 + r.3 >= cy && r.0 + r.2 <= b.0 + 1.0 && r.2 > 200.0)
        .max_by(|a, c| a.2.total_cmp(&c.2))
}

/// Whether the frame has a quad in a sprite's colour where that sprite's
/// pixels are: its position plus the run's place in it (to a pixel, as the
/// layer snaps to device pixels).
fn sprite_painted(quads: &[FrameQuad]) -> bool {
    if rekindle_native::painted() == 0 {
        return false;
    }
    let items = crate::overlay::OVERLAY_ITEMS.lock().unwrap().clone();
    items.iter().any(|item| {
        item.rects.iter().any(|r| {
            let want = (item.x + r.x, item.y + r.y, r.w, r.h);
            quads.iter().any(|(q, c)| {
                same_colour(*c, hsla(r.rgba))
                    && (q.0 - want.0).abs() <= 1.0
                    && (q.1 - want.1).abs() <= 1.0
                    && (q.2 - want.2).abs() <= 1.0
                    && (q.3 - want.3).abs() <= 1.0
            })
        })
    })
}

/// Whether the frame has an image (gpui's `paint_image`) where a sprite is:
/// its position and its picture's size (to a pixel), and, where the layout
/// says where the atlas tile is, a tile of the size rekindle built the image
/// at (its device size): gpui read the image's width and height where
/// rekindle wrote them.
fn sprite_imaged(window: Live) -> bool {
    if rekindle_native::painted_images() == 0 {
        return false;
    }
    let Some(f) = window.frame() else {
        return false;
    };
    let scale = window.scale_factor().unwrap_or(1.0);
    let items = crate::overlay::OVERLAY_ITEMS.lock().unwrap().clone();
    items.iter().any(|item| {
        item.picture.as_deref().is_some_and(|p| {
            let device = (
                (p.w * scale).round().max(1.0) as i32,
                (p.h * scale).round().max(1.0) as i32,
            );
            f.images.iter().enumerate().any(|(i, r)| {
                let placed = (r.0 - item.x).abs() <= 1.0
                    && (r.1 - item.y).abs() <= 1.0
                    && (r.2 - p.w).abs() <= 1.0
                    && (r.3 - p.h).abs() <= 1.0;
                placed && f.image_sizes.get(i).is_none_or(|s| *s == device)
            })
        })
    })
}

fn toast_up() -> bool {
    crate::toast_texts().iter().any(|t| t == TOAST_TEXT)
}

fn next(d: &mut Drive, s: Step) {
    d.step = s;
    d.since = 0;
}

fn load(a: &std::sync::atomic::AtomicUsize) -> usize {
    a.load(std::sync::atomic::Ordering::SeqCst)
}

/// After the client drew a frame (before rekindle recolours it): check what
/// the last action did and choose the next. `true` = an action is pending and
/// should run before the next frame ([`act`]).
///
/// # Safety
///
/// `window` is the client's live `Window`, on its UI thread, after a draw.
pub(crate) unsafe fn observe(window: *const u8, quads: &[FrameQuad]) -> bool {
    let window = Live(window);
    let mut g = DRIVE.lock().unwrap();
    let Some(d) = g.as_mut() else { return false };
    if d.step == Step::Done {
        return false;
    }
    d.frames += 1;
    d.since += 1;
    // The sprites move: compare each frame with the sprites it was drawn
    // from, and keep the first match, rather than one frame at the end
    // against sprites that have moved on since.
    if !d.sprite_imaged && rekindle_native::can_draw_images() && sprite_imaged(window) {
        d.sprite_imaged = true;
    }
    if !d.sprite_painted
        && rekindle_native::can_paint()
        && !rekindle_native::can_draw_images()
        && sprite_painted(quads)
    {
        d.sprite_painted = true;
    }
    // rekindle's client layer has turned itself off (a client-memory check
    // failed, or the framework layer did): nothing below can work, so say
    // why and stop rather than time out.
    if let Some(why) = rekindle_native::failure() {
        d.results.push(("native_failed", why.replace(' ', "_")));
        next(d, Step::Done);
        return false;
    }
    let timed_out = d.since > STEP_TIMEOUT;
    let scripts = [
        inspect,
        store,
        toast,
        button,
        window_script,
        menus,
        settings,
        kaomoji,
    ];
    for script in scripts {
        match script(d, window, quads, timed_out) {
            Flow::Next => {}
            Flow::Handled => break,
            Flow::Stop => return false,
        }
    }
    if d.step == Step::Done {
        d.results
            .push(("validated", rekindle_native::validate::passed().to_string()));
        let root = window.root_view_type().unwrap_or_default();
        d.results.push(("root_view", root));
        d.results.push((
            "sprites_built",
            (load(&crate::overlay::SPRITES_BUILT) > 0).to_string(),
        ));
        // Painted by rekindle as one element each, where the build lets it.
        // Drawn as images, where the client draws images; else painted as quads.
        if rekindle_native::can_draw_images() {
            d.results
                .push(("sprites_imaged", d.sprite_imaged.to_string()));
            let sized = window.frame().is_some_and(|f| !f.image_sizes.is_empty());
            d.results
                .push(("sprites_image_size_checked", sized.to_string()));
        } else {
            d.results.push((
                "sprites_painted",
                if rekindle_native::can_paint() {
                    d.sprite_painted.to_string()
                } else {
                    "n/a".into()
                },
            ));
        }
        d.results.push(("drive_frames", d.frames.to_string()));
    }
    d.pending.is_some()
}

/// What a script did with the step.
enum Flow {
    /// The step isn't one of its own.
    Next,
    /// It took the step.
    Handled,
    /// It ended the drive; nothing more is checked this frame.
    Stop,
}

/// Record that `key` failed and end the drive.
fn fail(d: &mut Drive, key: &'static str) {
    d.results.push((key, "false".into()));
    next(d, Step::Done);
}

/// The inspector script: hover a control, then select it.
fn inspect(d: &mut Drive, window: Live, quads: &[FrameQuad], timed_out: bool) -> Flow {
    let with = |colour: u32| filled(quads, colour);
    match d.step {
        // A control: a filled quad that is also a hitbox, of a button's size.
        Step::Settle if d.inspect && d.frames >= SETTLE_FRAMES => {
            let f = window.frame();
            let control = f.as_ref().and_then(|f| {
                f.hitboxes.iter().copied().find(|h| {
                    (16.0..=64.0).contains(&h.2)
                        && (16.0..=64.0).contains(&h.3)
                        && f.quads
                            .iter()
                            .any(|(q, c)| c.is_some_and(|c| c[3] > 0.0) && same_rect(*q, *h))
                })
            });
            match control {
                Some(r) => {
                    d.target = Some(r);
                    let (x, y) = centre(r);
                    d.pending = Some(Act::Move(x, y));
                    next(d, Step::CheckInspector);
                }
                None => fail(d, "inspect_control_found"),
            }
        }
        Step::CheckInspector if d.since >= 3 => {
            let target = d.target.expect("target");
            let pick = crate::inspector::last_pick();
            let picked = pick.as_ref().is_some_and(|p| {
                contains(target, p.target.rect) && p.stack.iter().any(|i| same_rect(i.rect, target))
            });
            let border = pick.as_ref().is_some_and(|p| {
                quads_bordered(window, crate::inspector::hex_border())
                    .iter()
                    .any(|r| same_rect(*r, p.target.rect))
            });
            let panel = with(crate::inspector::PANEL_RGBA).next();
            let panel_text = panel.is_some_and(|r| glyphs_in(window, r).len() > 10);
            if (picked && border && panel_text) || timed_out {
                d.results.push(("inspect_picked", picked.to_string()));
                d.results.push(("inspect_border", border.to_string()));
                d.results.push(("inspect_panel", panel_text.to_string()));
                let text = pick
                    .map(|p| crate::inspector::lines(&p).join("|"))
                    .unwrap_or_default();
                d.results.push(("inspect_text", text.replace(' ', "~")));
                d.clicks_before = crate::inspector::swallowed_count();
                let (x, y) = centre(target);
                d.pending = Some(Act::Click(x, y));
                next(d, Step::CheckSelected);
            }
        }
        // The press and release went to the inspector, not the app, and
        // the next frame selected what the pointer was over.
        Step::CheckSelected if d.since >= 3 => {
            let hovered = crate::inspector::last_pick().map(|p| p.target.rect);
            let selected = crate::inspector::selected()
                .is_some_and(|s| hovered.is_some_and(|h| same_rect(s.target.rect, h)));
            let kept = crate::inspector::swallowed_count() >= d.clicks_before + 2;
            if (selected && kept) || timed_out {
                d.results.push(("inspect_selected", selected.to_string()));
                d.results.push(("inspect_click_kept", kept.to_string()));
                next(d, Step::Done);
            }
        }
        _ => return Flow::Next,
    }
    Flow::Handled
}

/// The store script: install a plugin from the community index, and use its colour picker.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per step of the script, in order"
)]
fn store(d: &mut Drive, window: Live, quads: &[FrameQuad], timed_out: bool) -> Flow {
    let with = |colour: u32| filled(quads, colour);
    match d.step {
        Step::Settle if d.store && d.frames >= SETTLE_FRAMES => {
            d.renders_before = load(&crate::panel::SETTINGS_RENDERS);
            d.pending = Some(Act::Action("chat_client::OpenUserSettings"));
            next(d, Step::AwaitSettings);
        }
        // The Community tab. While the first update check hasn't cached the
        // channel the tab has no cards: press it again after a while.
        Step::StoreBrowse if d.since > if d.store_tries == 0 { 3 } else { 45 } => {
            let page = load(&crate::panel::SETTINGS_RENDERS) > d.renders_before;
            let tabs = tabs(quads);
            if let (true, Some((tab, _))) = (page, tabs.get(COMMUNITY_TAB)) {
                let (x, y) = centre(*tab);
                d.pending = Some(Act::Click(x, y));
                next(d, Step::StoreList);
            } else if timed_out {
                d.results.push(("plugins_page_rendered", page.to_string()));
                fail(d, "store_opened");
            }
        }
        // Its first card's button is "Install…".
        Step::StoreList if d.since > 3 => {
            let on_tab = tabs(quads)
                .get(COMMUNITY_TAB)
                .is_some_and(|(_, selected)| *selected);
            let install = cards(quads)
                .first()
                .and_then(|card| with(PANEL_BUTTON).find(|b| contains(*card, *b)));
            match (on_tab, install) {
                (true, Some(b)) => {
                    d.results.push(("store_opened", "true".into()));
                    d.results.push(("store_listed", "true".into()));
                    let (x, y) = centre(b);
                    d.pending = Some(Act::Click(x, y));
                    next(d, Step::StoreReview);
                }
                (true, None) if d.store_tries < STORE_TRIES => {
                    d.store_tries += 1;
                    next(d, Step::StoreBrowse);
                }
                (true, None) => {
                    d.results.push(("store_opened", "true".into()));
                    fail(d, "store_listed");
                }
                (false, _) if timed_out => fail(d, "store_opened"),
                (false, _) => {}
            }
        }
        Step::StoreReview if d.since > 3 => {
            let buttons: Vec<(Rect, usize)> = with(PANEL_BUTTON)
                .map(|r| (r, glyphs_in(window, r).len()))
                .collect();
            let install = buttons
                .iter()
                .find(|(_, n)| *n == INSTALL_GLYPHS)
                .map(|(r, _)| *r);
            let cancel = buttons.iter().any(|(_, n)| *n == CANCEL_GLYPHS);
            if let (Some(b), true) = (install, cancel) {
                d.results.push(("review_shown", "true".into()));
                let (x, y) = centre(b);
                d.pending = Some(Act::Click(x, y));
                next(d, Step::StoreInstalled);
            } else if d.since > STORE_TIMEOUT {
                // What the panel showed instead: its buttons, by glyph count.
                let counts: Vec<String> = buttons.iter().map(|(_, n)| n.to_string()).collect();
                d.results.push(("panel_buttons", counts.join(",")));
                fail(d, "review_shown");
            }
        }
        // Installed, the plugin's page is open: it has "Back".
        Step::StoreInstalled if d.since > 3 => {
            let main = with(PANEL_BUTTON).any(|r| glyphs_in(window, r).len() == BACK_GLYPHS);
            if main || timed_out {
                d.results.push(("install_confirmed", main.to_string()));
                next(d, if main { Step::StorePicker } else { Step::Done });
            }
        }
        // The picker's square and hue bar are hitboxes of their own size.
        Step::StorePicker if d.since > 3 => {
            use crate::picker::{HUE_SIZE, SV_SIZE};
            let hitboxes = window.frame().map(|f| f.hitboxes).unwrap_or_default();
            let sized = |size: (f32, f32)| {
                hitboxes
                    .iter()
                    .copied()
                    .find(|r| (r.2 - size.0).abs() < 1.0 && (r.3 - size.1).abs() < 1.0)
            };
            match (sized(SV_SIZE), sized(HUE_SIZE), crate::panel::picked()) {
                (Some(square), Some(bar), Some(_)) => {
                    d.results.push(("picker_shown", "true".into()));
                    d.target = Some(square);
                    d.row = Some(bar);
                    // Ends three quarters across and three quarters down.
                    let from = (square.0 + square.2 * 0.1, square.1 + square.3 * 0.1);
                    let to = (square.0 + square.2 * 0.75, square.1 + square.3 * 0.75);
                    d.pending = Some(Act::Drag(from, to));
                    next(d, Step::StoreDragged);
                }
                // The panel has no picker, or it is below the window's edge.
                _ if timed_out => {
                    d.results.push(("picker_shown", "false".into()));
                    next(d, Step::Done);
                }
                _ => {}
            }
        }
        Step::StoreDragged if d.since > 3 => {
            let (hex, settled) = crate::panel::picked().unwrap_or_default();
            let (rgb, _) = crate::picker::parse_hex(&hex).unwrap_or_default();
            let [_, s, v] = crate::picker::rgb_to_hsv(rgb);
            let at_end = (s - 0.75).abs() < 0.02 && (v - 0.25).abs() < 0.02;
            if (at_end && settled) || timed_out {
                d.results
                    .push(("picker_dragged", (at_end && settled).to_string()));
                if !(at_end && settled) {
                    d.results
                        .push(("picker_colour", format!("{hex}/{settled}")));
                    next(d, Step::Done);
                    return Flow::Stop;
                }
                let bar = d.row.expect("bar");
                d.pending = Some(Act::Click(bar.0 + bar.2 * 0.5, bar.1 + bar.3 / 2.0));
                next(d, Step::StoreHue);
            }
        }
        Step::StoreHue if d.since > 3 => {
            let (hex, settled) = crate::panel::picked().unwrap_or_default();
            let (rgb, _) = crate::picker::parse_hex(&hex).unwrap_or_default();
            let [h, s, v] = crate::picker::rgb_to_hsv(rgb);
            // The middle of the bar is cyan; saturation and value are kept.
            let turned =
                (h - 180.0).abs() < 4.0 && (s - 0.75).abs() < 0.03 && (v - 0.25).abs() < 0.03;
            if (turned && settled) || timed_out {
                d.results
                    .push(("picker_hue", (turned && settled).to_string()));
                if !(turned && settled) {
                    d.results
                        .push(("picker_colour", format!("{hex}/{settled}")));
                }
                next(d, Step::Done);
            }
        }
        _ => return Flow::Next,
    }
    Flow::Handled
}

/// Toasts: raise one, hover its close button, and click it.
fn toast(d: &mut Drive, _window: Live, quads: &[FrameQuad], timed_out: bool) -> Flow {
    let with = |colour: u32| filled(quads, colour);
    match d.step {
        Step::Settle if d.frames >= SETTLE_FRAMES => {
            crate::push_toast(TOAST_TEXT, crate::ToastStyle::Info);
            next(d, Step::AwaitToast);
        }
        // Toasts stack newest at the bottom: the script's is the lowest card.
        // Only its close button takes the pointer.
        Step::AwaitToast => {
            let lowest = with(CARD).max_by(|a, b| a.1.total_cmp(&b.1));
            let close = lowest.and_then(|card| with(CLOSE).find(|r| contains(card, *r)));
            if let (true, Some(r)) = (toast_up(), close) {
                d.results.push(("toast_painted", "true".into()));
                d.target = Some(r);
                let (x, y) = centre(r);
                d.pending = Some(Act::Move(x, y));
                next(d, Step::CheckToastHover);
            } else if timed_out {
                d.results.push(("toast_painted", "false".into()));
                next(d, Step::AwaitButton);
            }
        }
        Step::CheckToastHover => {
            let target = d.target.expect("target");
            let hovered = with(CLOSE_HOVER).any(|r| same_rect(r, target));
            if hovered || timed_out {
                d.results.push(("toast_hover", hovered.to_string()));
                d.others = crate::toast_texts()
                    .iter()
                    .filter(|t| *t != TOAST_TEXT)
                    .count();
                let (x, y) = centre(target);
                d.pending = Some(Act::Click(x, y));
                next(d, Step::CheckToastClick);
            }
        }
        Step::CheckToastClick => {
            // Only the clicked toast goes; any other stays.
            let gone = !toast_up() && crate::toast_texts().len() == d.others;
            if gone || timed_out {
                d.results.push(("toast_click_dismissed", gone.to_string()));
                next(d, Step::AwaitButton);
            }
        }
        _ => return Flow::Next,
    }
    Flow::Handled
}

/// The chat-bar button: find it, hover it, and click it.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per step of the script, in order"
)]
fn button(d: &mut Drive, window: Live, quads: &[FrameQuad], timed_out: bool) -> Flow {
    let with = |colour: u32| filled(quads, colour);
    match d.step {
        Step::AwaitButton if !framework_buttons() => {
            let filled: Vec<Rect> = quads
                .iter()
                .filter(|(_, c)| c[3] != 0.0)
                .map(|(r, _)| *r)
                .collect();
            if let Some(r) = client_button(window, &filled) {
                d.results.push(("button_placed", "true".into()));
                d.target = Some(r);
                d.button = Some(r);
                d.before = filled;
                d.glyphs_before = glyphs_in(window, r);
                let (x, y) = centre(r);
                d.pending = Some(Act::Move(x, y));
                next(d, Step::CheckButtonHover);
            } else if timed_out {
                fail(d, "button_placed");
            }
        }
        Step::AwaitButton => {
            let own = crate::chat_bar::own_buttons().first().copied();
            if let Some(b) = own {
                let r = (b.x, b.y, b.w, b.h);
                d.results.push(("button_placed", "true".into()));
                d.target = Some(r);
                let (x, y) = centre(r);
                d.pending = Some(Act::Move(x, y));
                next(d, Step::CheckButtonHover);
            } else if timed_out {
                fail(d, "button_placed");
            }
        }
        Step::CheckButtonHover => {
            let target = d.target.expect("target");
            let (hovered, tooltip) = if framework_buttons() {
                let hovered = with(BUTTON_HOVER).any(|r| same_rect(r, target));
                // The tooltip sits above the button, centred on it.
                let (cx, _) = centre(target);
                (
                    hovered,
                    with(TOOLTIP)
                        .any(|r| r.1 + r.3 <= target.1 + 0.5 && (r.0 + r.2 / 2.0 - cx).abs() < 1.5),
                )
            } else {
                // The client's button: unfilled at rest, and hovering it
                // either fills a background or recolours its glyphs (the
                // composer's icons brighten). Its tooltip is a new
                // card-sized surface off it (gpui places it by the pointer,
                // so it may overlap the button) carrying rekindle's text: one
                // glyph per non-space character, painted over the card.
                let new = |r: &Rect| !d.before.iter().any(|b| same_rect(*b, *r));
                let filled = quads
                    .iter()
                    .any(|(r, c)| c[3] != 0.0 && new(r) && same_rect(*r, target));
                let recoloured = glyphs_in(window, target).iter().any(|(g, c)| {
                    d.glyphs_before.iter().any(|(b, was)| {
                        same_rect(*b, *g) && was.map(f32::to_bits) != c.map(f32::to_bits)
                    })
                });
                let hovered = filled || recoloured;
                (
                    hovered,
                    tooltip_want() > 0
                        && tooltip_glyphs(window, target, &d.before)
                            .iter()
                            .any(|(_, n)| *n == tooltip_want()),
                )
            };
            if (hovered && tooltip) || timed_out {
                if !tooltip && !framework_buttons() {
                    // What was there instead: glyphs wanted / found per card.
                    let found: Vec<String> = tooltip_glyphs(window, target, &d.before)
                        .iter()
                        .map(|(r, n)| format!("{:.0}x{:.0}:{n}", r.2, r.3))
                        .collect();
                    d.results.push((
                        "button_tooltip_glyphs",
                        format!("{}/{}", tooltip_want(), found.join(";")),
                    ));
                }
                d.results.push(("button_hover", hovered.to_string()));
                d.results.push(("button_tooltip", tooltip.to_string()));
                d.clicks_before = crate::button_clicks();
                d.renders_before = load(&crate::registry::MODAL_RENDERS);
                let (x, y) = centre(target);
                d.pending = Some(Act::Click(x, y));
                next(d, Step::CheckButtonClick);
            }
        }
        Step::CheckButtonClick if !d.full => {
            let routed = crate::button_clicks() > d.clicks_before;
            if routed || timed_out {
                d.results.push(("button_click_routed", routed.to_string()));
                // rekindle's own button is the leftmost of its buttons.
                let own = crate::chat_bar::own_buttons().last().copied();
                if let Some(b) = own.filter(|_| crate::window::has_button()) {
                    let (x, y) = centre((b.x, b.y, b.w, b.h));
                    d.pending = Some(Act::Click(x, y));
                    next(d, Step::CheckWindowOpen);
                } else {
                    d.results.push(("window_open", "n/a".into()));
                    next(d, Step::Done);
                }
            }
        }
        _ => return Flow::Next,
    }
    Flow::Handled
}

/// rekindle's window: open, scroll, close by the scrim, the shortcut and Escape.
fn window_script(d: &mut Drive, window: Live, quads: &[FrameQuad], timed_out: bool) -> Flow {
    let with = |colour: u32| filled(quads, colour);
    match d.step {
        Step::CheckWindowOpen => {
            let viewport = window.viewport().unwrap_or((0.0, 0.0));
            // The window, centred 48 px down, in its surface colour.
            let painted = with(WINDOW_BG).any(|r| {
                r.2 >= 320.0
                    && (r.1 - 48.0).abs() < 1.0
                    && (r.0 + r.2 / 2.0 - viewport.0 / 2.0).abs() < 1.0
            });
            let grid = cards(quads).first().copied();
            if (crate::window::is_open() && painted && grid.is_some()) || timed_out {
                d.results.push((
                    "window_open",
                    (crate::window::is_open() && painted).to_string(),
                ));
                d.results.push(("window_grid", grid.is_some().to_string()));
                d.target = grid;
                let scrolled = grid.is_some() && rekindle_native::can_scroll() && {
                    // SAFETY: the drive observes from the client's UI thread.
                    unsafe {
                        rekindle_native::set_scroll_y_position(
                            crate::window::SCROLL_KEY,
                            -WINDOW_SCROLL,
                        )
                    }
                };
                if scrolled {
                    next(d, Step::CheckWindowScroll);
                } else {
                    d.results.push(("window_scrolled", "n/a".into()));
                    d.pending = Some(Act::Click(4.0, 4.0));
                    next(d, Step::CheckWindowClosed);
                }
            }
        }
        Step::CheckWindowScroll if d.since >= 2 => {
            // gpui clamps the position to what overflows, so the grid moved
            // by the position it kept.
            // SAFETY: the drive observes from the client's UI thread.
            let at = unsafe { rekindle_native::scroll_y_position(crate::window::SCROLL_KEY) }
                .unwrap_or(0.0);
            let was = d.target.expect("target");
            let moved = cards(quads)
                .first()
                .is_some_and(|r| at < 0.0 && (r.1 - (was.1 + at)).abs() < 1.0);
            // A grid that fits has nothing to scroll: gpui clamps to 0.
            let fits = at == 0.0 && cards(quads).first().is_some_and(|r| same_rect(*r, was));
            if moved || fits || timed_out {
                d.results.push((
                    "window_scrolled",
                    if fits {
                        "fits".into()
                    } else {
                        moved.to_string()
                    },
                ));
                d.pending = Some(Act::Click(4.0, 4.0));
                next(d, Step::CheckWindowClosed);
            }
        }
        Step::CheckWindowClosed => {
            let closed = !crate::window::is_open();
            if closed || timed_out {
                d.results.push(("window_closed", closed.to_string()));
                d.pending = Some(Act::Key(",", "<", true, true));
                next(d, Step::CheckShortcutOpened);
            }
        }
        Step::CheckShortcutOpened => {
            let open = crate::window::is_open();
            if open || timed_out {
                d.results.push(("window_shortcut", open.to_string()));
                d.pending = Some(Act::Key("escape", "", false, false));
                next(d, Step::CheckEscapeClosed);
            }
        }
        Step::CheckEscapeClosed => {
            let closed = !crate::window::is_open();
            if closed || timed_out {
                d.results.push(("window_escape", closed.to_string()));
                next(d, Step::Done);
            }
        }
        _ => return Flow::Next,
    }
    Flow::Handled
}

/// The modal the button opened, the context menu and the profile.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per step of the script, in order"
)]
fn menus(d: &mut Drive, window: Live, quads: &[FrameQuad], timed_out: bool) -> Flow {
    match d.step {
        Step::CheckButtonClick => {
            let routed = crate::button_clicks() > d.clicks_before;
            // The plugin answers the click (on the runtime's thread) by
            // opening its modal; the client then renders rekindle's body.
            let modal = crate::registry::modal_title().is_some();
            let body = load(&crate::registry::MODAL_RENDERS) > d.renders_before;
            if (routed && modal && body) || timed_out {
                d.results.push(("button_click_routed", routed.to_string()));
                d.results.push(("button_opened_modal", modal.to_string()));
                d.results.push(("modal_body_rendered", body.to_string()));
                d.pending = Some(Act::Action("chat_client::Dismiss"));
                next(d, Step::CheckModalDismissed);
            }
        }
        Step::CheckModalDismissed => {
            let gone = crate::registry::modal_title().is_none();
            if gone || timed_out {
                d.results.push(("modal_dismissed", gone.to_string()));
                next(d, Step::AwaitMemberRow);
            }
        }
        // The member list: the 240px column at the right edge, rows a
        // 32px avatar high; the first one.
        Step::AwaitMemberRow => {
            let hitboxes = window.frame().map(|f| f.hitboxes).unwrap_or_default();
            let width = window.viewport().map_or(0.0, |v| v.0);
            let row = hitboxes
                .into_iter()
                .filter(|r| r.0 >= width - 241.0 && r.2 > 150.0 && r.3 >= 32.0 && r.3 < 64.0)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some(r) = row {
                d.row = Some(r);
                d.before = quads
                    .iter()
                    .filter(|(_, c)| c[3] != 0.0)
                    .map(|(r, _)| *r)
                    .collect();
                d.clicks_before = load(&crate::registry::MENU_INVOKES);
                *COPIED.lock().unwrap() = None;
                let (x, y) = centre(r);
                d.pending = Some(Act::RightClick(x, y));
                next(d, Step::AwaitMenu);
            } else if timed_out {
                fail(d, "member_row_found");
            }
        }
        // The client's menu: a new 220px-wide surface; its rows are the
        // hitboxes in it, rekindle's group last.
        Step::AwaitMenu => {
            let menu = quads
                .iter()
                .filter(|(r, c)| {
                    c[3] != 0.0
                        && (r.2 - 220.0).abs() < 1.5
                        && !d.before.iter().any(|b| same_rect(*b, *r))
                })
                .map(|(r, _)| *r)
                .next();
            let hitboxes = window.frame().map(|f| f.hitboxes).unwrap_or_default();
            let rows: Vec<Rect> = menu
                .map(|m| {
                    hitboxes
                        .into_iter()
                        .filter(|h| contains(m, *h) && h.3 > 16.0 && h.2 < m.2 - 4.0)
                        .collect()
                })
                .unwrap_or_default();
            if rows.len() >= 2 {
                d.results.push(("menu_opened", "true".into()));
                let last = *rows
                    .iter()
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .expect("rows");
                let (x, y) = centre(last);
                d.pending = Some(Act::Click(x, y));
                next(d, Step::CheckMenuClick);
            } else if timed_out {
                fail(d, "menu_opened");
            }
        }
        Step::CheckMenuClick => {
            let routed = load(&crate::registry::MENU_INVOKES) > d.clicks_before;
            let copied = COPIED
                .lock()
                .unwrap()
                .as_deref()
                .is_some_and(|t| t.starts_with("https://discord.com/users/"));
            let toast = crate::toast_texts().iter().any(|t| t.starts_with("Copied"));
            if (routed && copied && toast) || timed_out {
                d.results.push(("menu_row_routed", routed.to_string()));
                d.results.push(("menu_copied", copied.to_string()));
                d.results.push(("menu_toast", toast.to_string()));
                d.clicks_before = load(&crate::registry::PROFILE_OPENS);
                d.renders_before = load(&crate::registry::PROFILE_RENDERS);
                let (x, y) = centre(d.row.expect("row"));
                d.pending = Some(Act::Click(x, y));
                next(d, Step::CheckProfile);
            }
        }
        Step::CheckProfile => {
            let opened = load(&crate::registry::PROFILE_OPENS) > d.clicks_before;
            let rendered = load(&crate::registry::PROFILE_RENDERS) > d.renders_before;
            if (opened && rendered) || timed_out {
                d.results.push(("profile_opened", opened.to_string()));
                d.results
                    .push(("profile_section_rendered", rendered.to_string()));
                d.pending = Some(Act::Action("chat_client::Dismiss"));
                next(d, Step::CloseProfile);
            }
        }
        Step::CloseProfile if d.since > 2 => {
            d.renders_before = load(&crate::panel::SETTINGS_RENDERS);
            d.pending = Some(Act::Action("chat_client::OpenUserSettings"));
            next(d, Step::AwaitSettings);
        }
        _ => return Flow::Next,
    }
    Flow::Handled
}

/// rekindle's settings page: its grid, a plugin's panel and a text field.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per step of the script, in order"
)]
fn settings(d: &mut Drive, window: Live, quads: &[FrameQuad], timed_out: bool) -> Flow {
    let with = |colour: u32| filled(quads, colour);
    match d.step {
        // The settings sidebar: the left third, rows a line high. The last
        // row is rekindle's page (its group is appended to the client's).
        Step::AwaitSettings => {
            let hitboxes = window.frame().map(|f| f.hitboxes).unwrap_or_default();
            let width = window.viewport().map_or(0.0, |v| v.0);
            let rows: Vec<Rect> = hitboxes
                .into_iter()
                .filter(|r| {
                    r.0 + r.2 <= width * 0.34 + 1.0 && r.3 > 16.0 && r.3 < 48.0 && r.2 > 60.0
                })
                .collect();
            if rows.len() >= 2 && d.since > 2 {
                let last = *rows
                    .iter()
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .expect("rows");
                d.results.push(("settings_opened", "true".into()));
                let (x, y) = centre(last);
                d.pending = Some(Act::Click(x, y));
                next(
                    d,
                    if d.store {
                        Step::StoreBrowse
                    } else {
                        Step::AwaitPluginsPage
                    },
                );
            } else if timed_out {
                fail(d, "settings_opened");
            }
        }
        // The grid: open each card's page in turn until one's panel has a
        // text field.
        Step::AwaitPluginsPage => {
            let page = load(&crate::panel::SETTINGS_RENDERS) > d.renders_before;
            let back = with(PANEL_BUTTON).find(|r| glyphs_in(window, *r).len() == BACK_GLYPHS);
            // A text field: a field-coloured box wider and taller than a toggle.
            let field = with(FIELD_BG)
                .filter(|r| r.2 > 100.0 && r.3 >= 20.0)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            // With no rules, text-replace's panel has no text field: press its
            // "Add rule" (a panel button of that many glyphs) to get one.
            let add_rule =
                || with(PANEL_BUTTON).find(|r| glyphs_in(window, *r).len() == ADD_RULE_GLYPHS);
            match (page, back, field) {
                (true, Some(_), Some(f)) => {
                    d.results.push(("plugins_page_rendered", "true".into()));
                    d.target = Some(f);
                    let (x, y) = centre(f);
                    d.pending = Some(Act::Click(x, y));
                    next(d, Step::CheckFieldFocused);
                }
                // On a plugin's page (a card's "Turn off" has as many
                // glyphs as "Add rule", so not in the grid).
                (true, Some(_), None) if !d.added_rule && add_rule().is_some() && d.since > 3 => {
                    d.added_rule = true;
                    let (x, y) = centre(add_rule().expect("the button"));
                    d.pending = Some(Act::Click(x, y));
                }
                // No field on this plugin's page: back, and the next card.
                (true, Some(b), None) if d.since > 12 => {
                    d.card += 1;
                    let (x, y) = centre(b);
                    d.pending = Some(Act::Click(x, y));
                }
                (true, None, _) if d.since > 3 => match cards(quads).get(d.card) {
                    // The card's corner, clear of its button.
                    Some(c) => d.pending = Some(Act::Click(c.0 + 12.0, c.1 + 12.0)),
                    None if d.since > 30 => {
                        d.results.push(("plugins_page_rendered", "true".into()));
                        fail(d, "text_field_found");
                    }
                    None => {}
                },
                _ if timed_out => {
                    d.results.push(("plugins_page_rendered", page.to_string()));
                    fail(d, "text_field_found");
                }
                _ => {}
            }
        }
        Step::CheckFieldFocused => {
            let focused = window.focused_field();
            if focused.is_some() || timed_out {
                d.results
                    .push(("text_field_focused", focused.is_some().to_string()));
                let Some(key) = focused else {
                    next(d, Step::Done);
                    return Flow::Stop;
                };
                d.field_before = crate::panel::field_value(&key).unwrap_or_default();
                d.field = Some(key);
                d.pending = Some(Act::Type(TYPED));
                next(d, Step::CheckTyped);
            }
        }
        Step::CheckTyped => {
            let key = d.field.clone().expect("field");
            let now = crate::panel::field_value(&key).unwrap_or_default();
            let typed = now == format!("{}{TYPED}", d.field_before);
            if typed || timed_out {
                d.results.push(("text_field_typed", typed.to_string()));
                d.pending = Some(Act::Key("enter", "\n", false, false));
                next(d, Step::CheckBlurred);
            }
        }
        Step::CheckBlurred => {
            let blurred = window.focused_field().is_none();
            if blurred || timed_out {
                d.results.push(("text_field_blurred", blurred.to_string()));
                // The picker needs the client's own button (a modal surface).
                let picker = d.button.is_some() && tooltip_want() > 0;
                next(
                    d,
                    if picker {
                        Step::LeaveSettings
                    } else {
                        Step::Done
                    },
                );
            }
        }
        Step::LeaveSettings if d.since > 2 => {
            d.pending = Some(Act::Action("chat_client::Dismiss"));
            next(d, Step::AwaitPicker);
            d.renders_before = usize::MAX;
        }
        _ => return Flow::Next,
    }
    Flow::Handled
}

/// A plugin's modal with a search field and inserted text.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per step of the script, in order"
)]
fn kaomoji(d: &mut Drive, window: Live, quads: &[FrameQuad], timed_out: bool) -> Flow {
    let with = |colour: u32| filled(quads, colour);
    match d.step {
        // Open the picker once settings have closed, then click its search
        // field: the topmost wide field-coloured box in the modal.
        Step::AwaitPicker if d.renders_before == usize::MAX && d.since > 2 => {
            // Found again: the chat view was rebuilt on the way back.
            let filled: Vec<Rect> = quads
                .iter()
                .filter(|(_, c)| c[3] != 0.0)
                .map(|(r, _)| *r)
                .collect();
            if let Some(r) = client_button(window, &filled) {
                d.renders_before = load(&crate::registry::MODAL_RENDERS);
                d.clicks_before = crate::insert::inserted_count();
                let (x, y) = centre(r);
                d.pending = Some(Act::Click(x, y));
                d.since = 0;
            } else if d.dismisses >= 4 {
                fail(d, "picker_button_found");
            } else if d.since.is_multiple_of(8) {
                // Dismiss closes the innermost layer first (an open profile
                // sits above settings): again until the chat is back.
                d.dismisses += 1;
                d.pending = Some(Act::Action("chat_client::Dismiss"));
            }
        }
        Step::AwaitPicker if d.renders_before != usize::MAX => {
            let shown = load(&crate::registry::MODAL_RENDERS) > d.renders_before;
            let field = with(FIELD_BG)
                .filter(|r| r.2 > 100.0 && r.3 >= 20.0)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let (true, Some(f)) = (shown, field) {
                d.results.push(("picker_opened", "true".into()));
                d.row = Some(f);
                let (x, y) = centre(f);
                d.pending = Some(Act::Click(x, y));
                next(d, Step::TypeSearch);
            } else if timed_out {
                let fields: Vec<String> = with(FIELD_BG)
                    .map(|r| format!("{:.0}x{:.0}@{:.0},{:.0}", r.2, r.3, r.0, r.1))
                    .collect();
                d.results.push((
                    "picker_diag",
                    format!(
                        "shown={shown};clicks={};fields={}",
                        crate::button_clicks(),
                        fields.join(";")
                    ),
                ));
                fail(d, "picker_opened");
            }
        }
        Step::TypeSearch if d.since > 2 => {
            d.pending = Some(Act::Type(SEARCH));
            d.target = None;
            next(d, Step::AwaitFaces);
        }
        // The faces are the modal's panel buttons; take the topmost once it
        // has held still for a few frames (each keystroke re-shows the
        // modal, and the faces move up when the category list goes).
        Step::AwaitFaces => {
            // Under the search field and level with it: not the client's own
            // buttons of that colour (the server rail).
            let field = d.row.expect("search field");
            let face = with(PANEL_BUTTON)
                .filter(|r| (r.0 - field.0).abs() < 1.0 && r.1 > field.1)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            match (face, d.target) {
                (Some(f), Some(t)) if same_rect(f, t) && d.since > 4 => {
                    // The topmost face is the panel's first button.
                    let face = crate::registry::modal_panel()
                        .and_then(|p| {
                            p.widgets.into_iter().find_map(|w| match w {
                                crate::Widget::Button { label, .. } => Some(label),
                                _ => None,
                            })
                        })
                        .unwrap_or_default();
                    d.field_before = face;
                    // The composer's glyphs before the click (see CheckInserted).
                    let row = d.button.and_then(|b| composer_of(window, b));
                    d.field = Some(row.map_or(0, |r| glyphs_in(window, r).len()).to_string());
                    let (x, y) = centre(f);
                    d.pending = Some(Act::Click(x, y));
                    next(d, Step::CheckInserted);
                }
                (Some(f), t) if t.is_none_or(|t| !same_rect(f, t)) => {
                    d.target = Some(f);
                    d.since = 0;
                }
                _ if timed_out => {
                    let fmt = |r: Rect| format!("{:.0}x{:.0}@{:.0},{:.0}", r.2, r.3, r.0, r.1);
                    let buttons: Vec<String> = with(PANEL_BUTTON).map(fmt).collect();
                    let fields: Vec<String> = with(FIELD_BG).map(fmt).collect();
                    let widgets: Vec<String> = crate::registry::modal_panel()
                        .map(|p| {
                            p.widgets
                                .iter()
                                .map(|w| match w {
                                    crate::Widget::TextInput { value, .. } => {
                                        format!("input:{value}")
                                    }
                                    crate::Widget::Text(t) => format!("text:{t}"),
                                    crate::Widget::Button { label, .. } => {
                                        format!("button:{label}")
                                    }
                                    crate::Widget::Select { selected, .. } => {
                                        format!("select:{selected}")
                                    }
                                    _ => "other".into(),
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    d.results.push((
                        "picker_diag",
                        format!(
                            "field={};buttons={};fields={};modal={}",
                            fmt(field),
                            buttons.join(";"),
                            fields.join(";"),
                            widgets.join("|")
                        )
                        .replace(' ', "~"),
                    ));
                    fail(d, "picker_faces");
                }
                _ => {}
            }
        }
        Step::CheckInserted => {
            let inserted = crate::insert::inserted_count() > d.clicks_before;
            // The composer's glyphs gain the face's (one per visible
            // character) once the picker has closed.
            let composer = d.button.and_then(|b| composer_of(window, b));
            let glyphs = composer.map_or(0, |r| glyphs_in(window, r).len());
            let want = d
                .field_before
                .chars()
                .filter(|c| !c.is_whitespace())
                .count();
            let before: usize = d.field.as_deref().and_then(|g| g.parse().ok()).unwrap_or(0);
            // An empty composer shows its placeholder instead, which the face
            // replaces.
            let shown = crate::registry::modal_title().is_none()
                && want > 0
                && (glyphs == before + want || glyphs == want);
            if (inserted && shown) || timed_out {
                d.results.push(("kaomoji_shown", format!("{shown}")));
                if !shown {
                    d.results.push((
                        "kaomoji_glyphs",
                        format!("{before}+{want}/{glyphs}@{composer:?}").replace(' ', ""),
                    ));
                }
                if !inserted {
                    let toasts = crate::toast_texts().join("|").replace(' ', "~");
                    d.results.push((
                        "kaomoji_diag",
                        format!("{};toasts={toasts}", crate::insert::insert_state()),
                    ));
                }
                d.results.push(("kaomoji_inserted", inserted.to_string()));
                // Focus was in the picker's search field, so the composer
                // had to be focused again first.
                d.results.push((
                    "kaomoji_refocused",
                    (crate::insert::refocused_count() > 0).to_string(),
                ));
                next(d, Step::Done);
            }
        }
        _ => return Flow::Next,
    }
    Flow::Handled
}

/// Run the pending action through the client's own event dispatch
/// (`Window::dispatch_event`, through `rekindle-native`). Called from a
/// next-frame callback: on the UI thread, outside any draw.
///
/// # Safety
///
/// `window` and `cx` are the client's live `Window` and `App`, on its UI
/// thread, outside a draw.
pub(crate) unsafe fn act(window: *mut u8, cx: *mut u8) {
    use rekindle_native::client::{Mouse, dispatch_key, dispatch_mouse, refresh};
    let pending = {
        let mut g = DRIVE.lock().unwrap();
        let Some(d) = g.as_mut() else { return };
        d.since = 0;
        d.pending.take()
    };
    // SAFETY: the caller's contract, for each dispatch below.
    unsafe {
        match pending {
            Some(Act::Move(x, y)) => {
                dispatch_mouse(window, cx, Mouse::Move(x, y));
            }
            Some(Act::Click(x, y)) => {
                dispatch_mouse(window, cx, Mouse::Move(x, y));
                dispatch_mouse(window, cx, Mouse::Down(x, y));
                dispatch_mouse(window, cx, Mouse::Up(x, y));
            }
            Some(Act::Drag((x0, y0), (x1, y1))) => {
                dispatch_mouse(window, cx, Mouse::Move(x0, y0));
                dispatch_mouse(window, cx, Mouse::Down(x0, y0));
                dispatch_mouse(
                    window,
                    cx,
                    Mouse::Move(f32::midpoint(x0, x1), f32::midpoint(y0, y1)),
                );
                dispatch_mouse(window, cx, Mouse::Move(x1, y1));
                dispatch_mouse(window, cx, Mouse::Up(x1, y1));
            }
            Some(Act::RightClick(x, y)) => {
                dispatch_mouse(window, cx, Mouse::Move(x, y));
                dispatch_mouse(window, cx, Mouse::RightDown(x, y));
                dispatch_mouse(window, cx, Mouse::RightUp(x, y));
            }
            Some(Act::Key(key, typed, control, shift)) => {
                dispatch_key(window, cx, key, typed, control, shift);
            }
            Some(Act::Action(name)) => {
                if let Err(e) = rekindle_native::client::dispatch_action(window, cx, name)
                    && let Some(d) = DRIVE.lock().unwrap().as_mut()
                {
                    d.results
                        .push(("action_error", format!("{e:#}").replace(' ', "_")));
                }
            }
            Some(Act::Type(text)) => {
                for c in text.chars() {
                    let s = c.to_string();
                    dispatch_key(window, cx, &s, &s, false, false);
                }
            }
            None => {}
        }
        refresh(window);
    }
}
