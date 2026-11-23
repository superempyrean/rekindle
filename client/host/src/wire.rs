//! The guest's emit call and the client's event dispatch, detoured.
//!
//! A client that splits its UI across a wire sends each turn's op buffer
//! out of its guest through one call, the emit method of the guest's
//! transport. The same call carries the guest's host calls. A buffer whose
//! leading `u32` is 0 holds a turn's op stream after it, and any other
//! value marks a host call. rekindle finds the emit method by the
//! `wire_turn_emit` anchor and detours it. Each turn's op stream passes
//! through rekindle's wire engine before the host receives it, and host
//! calls pass through unchanged.
//!
//! Input crosses the other way, one event buffer at a time, through the
//! function the `wire_event_dispatch` anchor finds. Its detour passes each
//! event through the engine before the guest receives it, and the UI
//! events inside a BATCH frame one by one. A reply the engine makes for
//! an event it consumed goes back through the client's own reply
//! function, found by `wire_event_reply`. Phoenix's dispatch returns no
//! data for an event, so that reply is rekindle's own model.
//!
//! The engine is `rekindle_wire_inject::Engine`, configured by a `Work`
//! value. [`follow`] makes it from the plugins' contributions every tick:
//!
//! - **Theme.** Each token a plugin overrides becomes a recolour of the
//!   client's own value for it, from the palettes framework theming uses.
//!   Which palette the client shows is read from the colours the guest
//!   sends. On a real launch the theme goes on the wire whenever
//!   `wire_turn_emit` resolves, and framework theming stops recolouring
//!   the scene, so no colour is recoloured twice. A test run puts it there
//!   only with `REKINDLE_HOST_WIRE_THEME` set.
//! - **Chat-bar buttons.** With a wire map (`wire.json` beside
//!   `anchors.json`, `REKINDLE_HOST_WIRE_MAP`) that says where the
//!   `chat-bar.button` slot is in the guest's tree, each plugin's button
//!   is mounted there, and taken out when the plugin removes it or is
//!   disabled. The client's registry and the framework layer then leave
//!   them out. A press on one is consumed and queued as the click the
//!   plugin receives from any other chat-bar button.
//! - **Named surfaces.** With a wire map that gives a selector for a named
//!   surface (`member-list`, `user-area`), `theming.surfaces()` lists it
//!   and each plugin's `set-surface` becomes a row rule on the nodes the
//!   selector finds: `hidden` drops the surface's subtree from the host's
//!   tree, and `background`, `opacity` and `radius` are cells written on
//!   its node. Where two plugins style one surface, the last style set
//!   applies. A cell a surface rule writes isn't recoloured by the theme.
//!   Without the wire, a build has no named surfaces.
//!
//! A test run can also name a JSON file holding fixed `Work` in
//! `REKINDLE_HOST_WIRE_WORK`, which the plugins' work is added to.
//!
//! The engine reads turns and events as `rekindle_wire` has them, read
//! from Phoenix's Android host. A desktop build should be checked against
//! both, and should supply its entries' signatures and the wire map for
//! that build: where its guest puts each
//! slot and each named surface, and which cells a pressable node needs.
//! Which value tag the renderer reads in `opacity` (322) and `radius`
//! (388) is modelled as `f64`. A hidden surface is dropped from the tree
//! rather than given the `hidden` (236) cell, since no build has shown a
//! renderer that honours that cell.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use rekindle_extension::Driver;
use rekindle_plugin_host::model::{SLOT_CHAT_BAR_BUTTON, SURFACES, SurfaceStyle};
use rekindle_ui_hook::{Button, Click};
use rekindle_wire::a11y::{Role, ids as a11y};
use rekindle_wire::{ElementKind, NodeId, PropId, Value};
use rekindle_wire_inject::{
    Cell, Engine, Handler, Mount, MountNode, RESERVED, Recolor, RowAction, RowRule, Selector, TEXT,
    Verdict, Work,
};
use serde::Deserialize;

use crate::hooks::Hooks;
use crate::install::installed_mode;

/// The JSON file holding the `Work`, in a test run.
const WORK_ENV: &str = "REKINDLE_HOST_WIRE_WORK";
/// Set in a test run to put the plugins' theme on the wire.
const THEME_ENV: &str = "REKINDLE_HOST_WIRE_THEME";
/// The anchor of the guest's emit call.
const ANCHOR: &str = "wire_turn_emit";
/// The anchor of the client's event dispatch.
const EVENT_ANCHOR: &str = "wire_event_dispatch";
/// The anchor of the function the client delivers a guest's reply through.
const REPLY_ANCHOR: &str = "wire_event_reply";

/// `(transport, ptr, len) -> reply`: the transport's data pointer, then
/// one buffer as its pointer and length. The vtable's shape and the
/// leading `u32` are recovered from Phoenix's Android host, where the
/// reply is a handle. A client whose emit method passes the buffer or the
/// reply another way needs this signature recovered from its own build.
type EmitFn = unsafe extern "C" fn(*mut c_void, *const u8, usize) -> i64;
/// The leading `u32` of a buffer that holds a turn's op stream.
const OPS: u32 = 0;
/// `(handle, ptr, len, reply_ctx) -> bool`: one event's buffer to the
/// guest `handle` names, and the slot its reply goes to. True when the
/// guest made no reply. A client whose dispatch takes the event another
/// way needs this signature recovered from its own build.
type DispatchFn = unsafe extern "C" fn(*mut c_void, *const u8, usize, *mut c_void) -> bool;
/// `(slot, ptr, len)`: copy reply bytes into the slot a dispatch was given.
type ReplyFn = unsafe extern "C" fn(*mut c_void, *const u8, usize);

/// Where a build's guest puts the slots plugins add to, as `wire.json`
/// holds it. Modelled: the selectors are written against the cells one
/// build's guest sends, and a desktop build's map comes from that build.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireMap {
    /// By slot name (`chat-bar.button`).
    #[serde(default)]
    slots: BTreeMap<String, SlotMap>,
    /// The node of each named surface (`member-list`), by name.
    #[serde(default)]
    surfaces: BTreeMap<String, Selector>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SlotMap {
    /// The node a slot's items are mounted under, after its own children.
    under: Selector,
    /// Cells each item's pressable node gets besides its role and label,
    /// for a guest whose host raises presses only on nodes that ask.
    #[serde(default)]
    cells: Vec<Cell>,
}

/// What the plugins put on the wire, decided once the anchors resolved.
struct Routes {
    /// The fixed work of a test run, which the plugins' work is added to.
    base: Work,
    /// The plugins' theme goes on the wire.
    theme: bool,
    /// Where the chat-bar slot is, when the plugins' buttons go there.
    chat_bar: Option<SlotMap>,
    /// The named surfaces the plugins can restyle, by name. Empty when
    /// their styles don't go on the wire.
    surfaces: BTreeMap<String, Selector>,
}

/// The engine both detours run. It is made before either goes live.
static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);
/// Set when [`follow`] has plugins' work to keep the engine on.
static ROUTES: Mutex<Option<Routes>> = Mutex::new(None);
/// The plugins' buttons are mounted ([`start_ui`]).
static UI_LIVE: AtomicBool = AtomicBool::new(false);
/// How many recolours the engine runs for the plugins' theme.
static RECOLORS: AtomicUsize = AtomicUsize::new(0);
/// How many of the plugins' buttons are mounted.
static MOUNTS: AtomicUsize = AtomicUsize::new(0);
/// The named surfaces the plugins' styles apply to, joined by `,`.
static STYLED: Mutex<String> = Mutex::new(String::new());
/// Presses on the plugins' buttons, queued for the plugins.
static PRESSES: AtomicU64 = AtomicU64::new(0);
/// `Work` the engine refused.
static REFUSED: AtomicU64 = AtomicU64::new(0);
/// Calls to [`follow`], which reads the shown palette every
/// [`DETECT_EVERY`]th of them.
static FOLLOWS: AtomicUsize = AtomicUsize::new(0);
/// The palette the guest's colours fit, by name.
static SHOWN: Mutex<Option<String>> = Mutex::new(None);
const DETECT_EVERY: usize = 30;

static TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);
static TURNS: AtomicU64 = AtomicU64::new(0);
static REWRITTEN: AtomicU64 = AtomicU64::new(0);
static FAILED: AtomicU64 = AtomicU64::new(0);
/// Host calls the emit detour passed through.
static CALLS: AtomicU64 = AtomicU64::new(0);

static EVENT_TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);
static REPLY: AtomicUsize = AtomicUsize::new(0);
static EVENTS: AtomicU64 = AtomicU64::new(0);
static EVENTS_REPLACED: AtomicU64 = AtomicU64::new(0);
static EVENTS_CONSUMED: AtomicU64 = AtomicU64::new(0);
static EVENTS_FAILED: AtomicU64 = AtomicU64::new(0);

/// What [`install`] decided, for the report.
pub(crate) struct Installed {
    /// `wire_detour`: `off` with no work, `on`, or why not.
    pub(crate) turns: String,
    /// `wire_event_detour`, the same.
    pub(crate) events: String,
    /// `theme_route`: `wire` or `scene`.
    pub(crate) theme: &'static str,
    /// `wire_ui`: `on` when the plugins' buttons go on the wire, `off`, or
    /// why the wire map couldn't be read.
    pub(crate) ui: String,
    /// `wire_surfaces`: the named surfaces plugins can restyle on the wire,
    /// joined by `,`, or `off`.
    pub(crate) surfaces: String,
}

/// Detour the emit call and the event dispatch if there is wire work.
pub(crate) fn install(hooks: &Hooks) -> Installed {
    let off = |theme, ui: String| Installed {
        turns: "off".into(),
        events: "off".into(),
        theme,
        ui,
        surfaces: "off".into(),
    };
    let failed = |e: &anyhow::Error, ui: String| Installed {
        turns: crate::err(e),
        events: crate::err(e),
        theme: "scene",
        ui,
        surfaces: "off".into(),
    };
    let fixed = match work() {
        Ok(w) => w,
        Err(e) => return failed(&e, "off".into()),
    };
    let (map, ui) = match wire_map() {
        Ok(m) => (m, "off".to_string()),
        Err(e) => (None, crate::err(&e)),
    };
    // On a real launch the anchor decides. A client whose UI isn't split
    // across a wire has no emit call to find, and keeps both on the
    // framework layer.
    let split = hooks.addr(ANCHOR).is_ok();
    let theme = if installed_mode() {
        split
    } else {
        std::env::var_os(THEME_ENV).is_some()
    };
    // The plugins' UI goes on the wire on a split build, or in a test run
    // without fixed work.
    let plugins_ui = if installed_mode() {
        split
    } else {
        fixed.is_none()
    };
    let (chat_bar, surfaces) = match map.filter(|_| plugins_ui) {
        Some(mut m) => {
            m.surfaces.retain(|s, _| SURFACES.contains(&s.as_str()));
            (m.slots.remove(SLOT_CHAT_BAR_BUTTON), m.surfaces)
        }
        None => (None, BTreeMap::new()),
    };
    let ui = if chat_bar.is_some() { "on".into() } else { ui };
    let route = if theme { "wire" } else { "scene" };
    if fixed.is_none() && !theme && chat_bar.is_none() && surfaces.is_empty() {
        return off(route, ui);
    }
    let base = fixed.unwrap_or_default();
    let named = if surfaces.is_empty() {
        "off".to_string()
    } else {
        surfaces.keys().cloned().collect::<Vec<_>>().join(",")
    };
    if theme || chat_bar.is_some() || !surfaces.is_empty() {
        *routes() = Some(Routes {
            base: base.clone(),
            theme,
            chat_bar,
            surfaces,
        });
    }
    match Engine::new(base) {
        Ok(e) => *lock() = Some(e),
        Err(e) => return failed(&anyhow::Error::new(e), ui),
    }
    let verdict = |r: anyhow::Result<()>| match r {
        Ok(()) => "on".to_string(),
        Err(e) => crate::err(&e),
    };
    let turns = verdict(install_turns(hooks));
    // Without the emit call's detour nothing reaches the wire: the theme and
    // the buttons stay on the framework layer and the client's registry,
    // and the build has no named surfaces.
    if turns != "on" {
        *routes() = None;
        return Installed {
            turns,
            events: "off".into(),
            theme: "scene",
            ui: "off".into(),
            surfaces: "off".into(),
        };
    }
    Installed {
        turns,
        events: verdict(install_events(hooks)),
        theme: route,
        ui,
        surfaces: named,
    }
}

/// The named surfaces plugins can restyle on the wire, for
/// `theming.surfaces()`. Empty when there are none.
pub(crate) fn surfaces() -> Vec<String> {
    routes()
        .as_ref()
        .map(|r| r.surfaces.keys().cloned().collect())
        .unwrap_or_default()
}

/// Whether the plugins' theme goes on the wire, so framework theming must
/// stop recolouring the scene.
pub(crate) fn theme_on_wire() -> bool {
    routes().as_ref().is_some_and(|r| r.theme)
}

/// Mount the plugins' chat-bar buttons and apply their surface styles on
/// the wire from now on, and take the buttons out of the client's registry
/// and the framework layer. Nothing happens without a wire map that names
/// the slot or a surface.
pub(crate) fn start_ui(driver: &mut Driver) {
    let (buttons, surfaces) = routes().as_ref().map_or((false, false), |r| {
        (r.chat_bar.is_some(), !r.surfaces.is_empty())
    });
    if buttons {
        driver.use_wire_chat_bar();
    }
    if buttons || surfaces {
        UI_LIVE.store(true, Ordering::SeqCst);
    }
}

/// The `Work` a test run names, or `None` when there is none to do.
fn work() -> anyhow::Result<Option<Work>> {
    let Some(path) = std::env::var_os(WORK_ENV).filter(|_| !installed_mode()) else {
        return Ok(None);
    };
    let path = std::path::Path::new(&path);
    let json = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("can't read {}: {e}", path.display()))?;
    let work = Work::from_json(&json)?;
    Ok((!work.is_empty()).then_some(work))
}

/// The build's wire map, if it ships one.
fn wire_map() -> anyhow::Result<Option<WireMap>> {
    let Some(path) = std::env::var_os(rekindle_install::env::WIRE_MAP) else {
        return Ok(None);
    };
    let path = std::path::Path::new(&path);
    let json = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("can't read {}: {e}", path.display()))?;
    let map =
        serde_json::from_str(&json).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok(Some(map))
}

fn lock() -> std::sync::MutexGuard<'static, Option<Engine>> {
    ENGINE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn routes() -> std::sync::MutexGuard<'static, Option<Routes>> {
    ROUTES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Keep the engine's work in step with the plugins, and hand the presses
/// it kept to them. The live client calls it every tick.
pub(crate) fn follow(driver: &Driver) {
    let routes = routes();
    let Some(r) = routes.as_ref() else {
        return;
    };
    let mut engine = lock();
    let Some(engine) = engine.as_mut() else {
        return;
    };
    for p in engine.take_presses() {
        if p.surface == SLOT_CHAT_BAR_BUTTON {
            PRESSES.fetch_add(1, Ordering::Relaxed);
            rekindle_ui_hook::queue_click(Click::Button {
                plugin: p.plugin,
                item: p.element,
            });
        }
    }
    // The scene and the wire never both recolour.
    let recolors = if r.theme && !driver.scene_recolor() {
        theme_recolors(driver, engine)
    } else {
        r.base.recolors.clone()
    };
    let mut mounts = r.base.mounts.clone();
    let buttons = match &r.chat_bar {
        Some(slot) if UI_LIVE.load(Ordering::SeqCst) => {
            button_mounts(&driver.chat_bar_buttons(), slot)
        }
        _ => Vec::new(),
    };
    let n = buttons.len();
    mounts.extend(buttons);
    let styles = if UI_LIVE.load(Ordering::SeqCst) {
        driver.surface_styles()
    } else {
        Vec::new()
    };
    let (mut surface_rows, styled) = surface_rules(&styles, &r.surfaces);
    let mut rows = r.base.rows.clone();
    rows.append(&mut surface_rows);
    let work = Work {
        recolors,
        mounts,
        rows,
        ..r.base.clone()
    };
    if *engine.work() == work {
        return;
    }
    let themed = work.recolors.len();
    if engine.set_work(work).is_ok() {
        RECOLORS.store(themed, Ordering::SeqCst);
        MOUNTS.store(n, Ordering::SeqCst);
        *STYLED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = styled.join(",");
    } else {
        REFUSED.fetch_add(1, Ordering::Relaxed);
    }
}

/// The recolours for the plugins' theme, for the palette the guest's
/// colours fit.
fn theme_recolors(driver: &Driver, engine: &Engine) -> Vec<Recolor> {
    let palettes = driver.recolors();
    let mut shown = SHOWN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if FOLLOWS
        .fetch_add(1, Ordering::Relaxed)
        .is_multiple_of(DETECT_EVERY)
        && let Some(i) = shown_palette(engine.guest(), &palettes)
    {
        *shown = Some(palettes[i].name.clone());
    }
    palettes
        .iter()
        .find(|p| Some(&p.name) == shown.as_ref())
        .map(recolors)
        .unwrap_or_default()
}

/// The first node id the plugins' mounts take, above the ids a test run's
/// fixed work uses.
const PLUGIN_IDS: NodeId = RESERVED + 0x0100_0000;
/// Node ids each plugin's block holds.
const BLOCK: NodeId = 0x1000;

/// Node ids handed out, kept for the life of the process so a button keeps
/// its nodes while it stays.
#[derive(Default)]
struct Ids {
    /// Per plugin: its block's first id and how many of it are used.
    blocks: BTreeMap<String, (NodeId, NodeId)>,
    /// Per item key: its root.
    roots: BTreeMap<String, NodeId>,
}

static IDS: Mutex<Option<Ids>> = Mutex::new(None);

/// The root id of `key`'s two nodes, from `plugin`'s block. Plugins never
/// see ids, and one plugin's nodes can't fall in another's block. `None`
/// once the block or the range is full.
fn button_root(plugin: &str, key: &str) -> Option<NodeId> {
    let mut ids = IDS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let ids = ids.get_or_insert_with(Ids::default);
    if let Some(r) = ids.roots.get(key) {
        return Some(*r);
    }
    if !ids.blocks.contains_key(plugin) {
        let next = u32::try_from(ids.blocks.len()).ok()?;
        let first = PLUGIN_IDS.checked_add(next.checked_mul(BLOCK)?)?;
        first.checked_add(BLOCK - 1)?;
        ids.blocks.insert(plugin.to_string(), (first, 0));
    }
    let block = ids.blocks.get_mut(plugin)?;
    if block.1 + 2 > BLOCK {
        return None;
    }
    let root = block.0 + block.1;
    block.1 += 2;
    ids.roots.insert(key.to_string(), root);
    Some(root)
}

/// One mount per chat-bar button under the slot's node: a `view` with the
/// `button` role, the button's tooltip as its label and a plugin handler,
/// holding a `text` with its glyph.
fn button_mounts(buttons: &[(String, Button)], slot: &SlotMap) -> Vec<Mount> {
    buttons
        .iter()
        .filter_map(|(plugin, b)| {
            let (_, item) = b.key.split_once('\u{1f}')?;
            let root = button_root(plugin, &b.key)?;
            let label = if b.tooltip.is_empty() {
                item
            } else {
                &b.tooltip
            };
            let mut cells = vec![
                Cell::new(a11y::ROLE, Value::Str(Role::Button.name().into())),
                Cell::new(a11y::LABEL, Value::Str(label.into())),
            ];
            cells.extend(slot.cells.iter().cloned());
            Some(Mount {
                under: slot.under.clone(),
                root,
                nodes: vec![
                    MountNode {
                        id: root,
                        kind: ElementKind::VIEW,
                        cells,
                        children: vec![root + 1],
                        on_press: Some(Handler::Plugin {
                            plugin: plugin.clone(),
                            surface: SLOT_CHAT_BAR_BUTTON.into(),
                            element: item.into(),
                        }),
                    },
                    MountNode {
                        id: root + 1,
                        kind: ElementKind::TEXT,
                        cells: vec![Cell::new(TEXT, Value::Str(b.glyph.clone()))],
                        children: Vec::new(),
                        on_press: None,
                    },
                ],
            })
        })
        .collect()
}

/// `opacity` (322), 0 to 1. Modelled as `f64`.
const OPACITY: PropId = PropId(322);
/// `radius` (388), in pixels. Modelled as `f64`.
const RADIUS: PropId = PropId(388);

/// The row rules for the plugins' surface styles, and the surfaces they
/// apply to. A hidden surface's node and everything under it are dropped,
/// and the other fields are cells on its node. A style for a surface the
/// map lacks, or one that changes nothing, makes no rule.
fn surface_rules(
    styles: &[(String, SurfaceStyle)],
    map: &BTreeMap<String, Selector>,
) -> (Vec<RowRule>, Vec<String>) {
    let mut rules = Vec::new();
    let mut styled = Vec::new();
    for (name, style) in styles {
        let Some(select) = map.get(name) else {
            continue;
        };
        let action = if style.hidden {
            RowAction::Drop
        } else {
            let mut cells = Vec::new();
            if let Some(c) = style.background {
                let hex = u32::from_be_bytes([c.r, c.g, c.b, c.a]);
                cells.push(Cell::new(
                    rekindle_wire::ids::BACKGROUND,
                    rekindle_wire_inject::rgba(hex),
                ));
            }
            if let Some(o) = style.opacity {
                cells.push(Cell::new(OPACITY, Value::F64(o.into())));
            }
            if let Some(r) = style.radius {
                cells.push(Cell::new(RADIUS, Value::F64(r.into())));
            }
            if cells.is_empty() {
                continue;
            }
            RowAction::Cells(cells)
        };
        rules.push(RowRule {
            select: select.clone(),
            action,
        });
        styled.push(name.clone());
    }
    (rules, styled)
}

/// One recolour per token of `palette` a plugin overrides: the theme's
/// value to the plugin's. Where two tokens hold the same colour, the first
/// decides, as in framework theming.
fn recolors(palette: &rekindle_ui_hook::ThemePalette) -> Vec<Recolor> {
    let mut seen = std::collections::BTreeSet::new();
    palette
        .pairs
        .iter()
        .map(|(from, to)| Recolor {
            from: rgba(*from) >> 8,
            to: rgba(*to),
        })
        .filter(|r| seen.insert(r.from))
        .collect()
}

/// The palette most of the guest's colour cells come from, if any of them
/// does. A tie goes to the earlier palette.
fn shown_palette(
    guest: &rekindle_wire::Tree,
    palettes: &[rekindle_ui_hook::ThemePalette],
) -> Option<usize> {
    let mut sent = std::collections::BTreeMap::<u32, usize>::new();
    for n in guest.nodes() {
        for p in rekindle_wire_inject::COLOR_PROPS {
            if let Some(c) = guest.props().get(n.id, *p).and_then(Value::as_color) {
                *sent.entry(c & 0x00ff_ffff).or_default() += 1;
            }
        }
    }
    palettes
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let colors: std::collections::BTreeSet<u32> =
                p.colors.iter().map(|c| rgba(*c) >> 8).collect();
            let n: usize = colors.iter().filter_map(|c| sent.get(c)).sum();
            (n, i)
        })
        .filter(|(n, _)| *n > 0)
        .max_by_key(|(n, i)| (*n, std::cmp::Reverse(*i)))
        .map(|(_, i)| i)
}

/// An HSLA colour as `0xRRGGBBAA`, each channel rounded to a byte.
fn rgba(hsla: [f32; 4]) -> u32 {
    let c = rekindle_native::Rgba::from_hsla(hsla);
    (rgb8([c.r, c.g, c.b]) << 8) | byte(c.a)
}

/// An RGB colour as `0xRRGGBB`.
fn rgb8([r, g, b]: [f32; 3]) -> u32 {
    (byte(r) << 16) | (byte(g) << 8) | byte(b)
}

fn byte(v: f32) -> u32 {
    u32::from((v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn install_turns(hooks: &Hooks) -> anyhow::Result<()> {
    use rekindle_hook::{Detour, InlineDetour};
    let target = hooks.addr(ANCHOR)?;
    // SAFETY: the anchor resolved against this process's own image to the
    // emit method's entry, whose signature is `EmitFn`, and the client
    // hasn't started its UI: the shim holds it until the host is ready.
    let detour = unsafe { InlineDetour.install(target, hooked_emit as *const () as usize)? };
    TRAMPOLINE.store(detour.trampoline(), Ordering::SeqCst);
    // The detour stays for the life of the process.
    std::mem::forget(detour);
    Ok(())
}

fn install_events(hooks: &Hooks) -> anyhow::Result<()> {
    use rekindle_hook::{Detour, InlineDetour};
    let target = hooks.addr(EVENT_ANCHOR)?;
    REPLY.store(hooks.addr(REPLY_ANCHOR)?, Ordering::SeqCst);
    // SAFETY: as for the emit method, the dispatch's entry, whose signature is
    // `DispatchFn`, before the client's UI starts.
    let detour = unsafe { InlineDetour.install(target, hooked_dispatch as *const () as usize)? };
    EVENT_TRAMPOLINE.store(detour.trampoline(), Ordering::SeqCst);
    std::mem::forget(detour);
    Ok(())
}

/// The counts each installed detour adds to the drive result and the
/// status line: turns emitted, rewritten and failed on, host calls passed
/// through, and events dispatched, replaced, consumed and failed on.
pub(crate) fn counts() -> Option<String> {
    let n = |c: &AtomicU64| c.load(Ordering::SeqCst);
    let mut out = Vec::new();
    if TRAMPOLINE.load(Ordering::SeqCst) != 0 {
        out.push(format!(
            "wire_detour_turns={} wire_detour_rewritten={} wire_detour_failed={} wire_detour_calls={}",
            n(&TURNS),
            n(&REWRITTEN),
            n(&FAILED),
            n(&CALLS),
        ));
    }
    if let Some(r) = routes().as_ref() {
        if r.theme {
            out.push(format!(
                "wire_theme_recolors={}",
                RECOLORS.load(Ordering::SeqCst)
            ));
        }
        if r.chat_bar.is_some() {
            out.push(format!(
                "wire_ui_live={} wire_ui_mounts={} wire_plugin_presses={}",
                u8::from(UI_LIVE.load(Ordering::SeqCst)),
                MOUNTS.load(Ordering::SeqCst),
                PRESSES.load(Ordering::SeqCst),
            ));
        }
        if !r.surfaces.is_empty() {
            let styled = STYLED
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            out.push(format!(
                "wire_surfaces_live={} wire_surfaces_styled={}",
                u8::from(UI_LIVE.load(Ordering::SeqCst)),
                if styled.is_empty() { "none" } else { &styled },
            ));
        }
        out.push(format!(
            "wire_work_refused={}",
            REFUSED.load(Ordering::SeqCst)
        ));
    }
    if EVENT_TRAMPOLINE.load(Ordering::SeqCst) != 0 {
        out.push(format!(
            "wire_detour_events={} wire_detour_events_replaced={} wire_detour_events_consumed={} wire_detour_events_failed={}",
            n(&EVENTS),
            n(&EVENTS_REPLACED),
            n(&EVENTS_CONSUMED),
            n(&EVENTS_FAILED),
        ));
    }
    (!out.is_empty()).then(|| out.join(" "))
}

/// Run `f` on the engine. `None` when there is none or it panicked, since
/// a panic must not unwind into the client's frame.
fn with_engine<T>(f: impl FnOnce(&mut Engine) -> T) -> Option<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lock().as_mut().map(f)))
        .ok()
        .flatten()
}

/// Run `f` on the tree rekindle mirrors, for the debug endpoint.
pub(crate) fn mirror<T>(f: impl FnOnce(&rekindle_wire::Tree) -> T) -> Option<T> {
    with_engine(|e| f(e.mirror()))
}

/// The op stream `buf` carries, or `None` for a host call.
fn op_stream(buf: &[u8]) -> Option<&[u8]> {
    let (head, ops) = buf.split_first_chunk::<4>()?;
    (u32::from_le_bytes(*head) == OPS).then_some(ops)
}

/// Called by the guest in place of its transport's emit method, with its
/// arguments.
unsafe extern "C" fn hooked_emit(transport: *mut c_void, bytes: *const u8, len: usize) -> i64 {
    // SAFETY: TRAMPOLINE was stored before the detour went live, and has
    // this signature.
    let original =
        unsafe { std::mem::transmute::<usize, EmitFn>(TRAMPOLINE.load(Ordering::SeqCst)) };
    // SAFETY: the guest passes `len` readable bytes.
    let buf = unsafe { std::slice::from_raw_parts(bytes, len) };
    let Some(ops) = op_stream(buf) else {
        CALLS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the guest's own arguments.
        return unsafe { original(transport, bytes, len) };
    };
    TURNS.fetch_add(1, Ordering::Relaxed);
    crate::debug::turn(ops);
    match with_engine(|e| e.filter(ops)) {
        Some(Ok(Some(rewritten))) => {
            REWRITTEN.fetch_add(1, Ordering::Relaxed);
            let mut framed = Vec::with_capacity(4 + rewritten.len());
            framed.extend_from_slice(&OPS.to_le_bytes());
            framed.extend_from_slice(&rewritten);
            // SAFETY: the guest's transport, with the rewritten turn. The
            // host reads the buffer within the call.
            unsafe { original(transport, framed.as_ptr(), framed.len()) }
        }
        r => {
            if !matches!(r, Some(Ok(None))) {
                FAILED.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: the guest's own arguments.
            unsafe { original(transport, bytes, len) }
        }
    }
}

/// Called by the client in place of its event dispatch, with its arguments.
unsafe extern "C" fn hooked_dispatch(
    handle: *mut c_void,
    bytes: *const u8,
    len: usize,
    reply_ctx: *mut c_void,
) -> bool {
    // SAFETY: the statics were stored before the detour went live, and have
    // these signatures.
    let (original, reply) = unsafe {
        (
            std::mem::transmute::<usize, DispatchFn>(EVENT_TRAMPOLINE.load(Ordering::SeqCst)),
            std::mem::transmute::<usize, ReplyFn>(REPLY.load(Ordering::SeqCst)),
        )
    };
    EVENTS.fetch_add(1, Ordering::Relaxed);
    // SAFETY: the client passes `len` readable bytes.
    let input = unsafe { std::slice::from_raw_parts(bytes, len) };
    crate::debug::event(input);
    match with_engine(|e| e.event(input)) {
        Some(Ok(Verdict::Pass)) => {}
        Some(Ok(Verdict::Replace(ev))) => {
            EVENTS_REPLACED.fetch_add(1, Ordering::Relaxed);
            let replaced = ev.to_bytes();
            // SAFETY: the client's handle and reply slot, with the event the
            // engine made. The guest reads the buffer within the call.
            return unsafe { original(handle, replaced.as_ptr(), replaced.len(), reply_ctx) };
        }
        Some(Ok(Verdict::Batch(batch))) => {
            EVENTS_REPLACED.fetch_add(1, Ordering::Relaxed);
            // SAFETY: as for a replaced event, with the batch the engine
            // filtered.
            return unsafe { original(handle, batch.as_ptr(), batch.len(), reply_ctx) };
        }
        Some(Ok(Verdict::Consume(value))) => {
            EVENTS_CONSUMED.fetch_add(1, Ordering::Relaxed);
            let Some(value) = value else { return true };
            let bytes = value.to_bytes();
            // SAFETY: the slot the client gave this dispatch, and the
            // handler's reply, which the client copies.
            unsafe { reply(reply_ctx, bytes.as_ptr(), bytes.len()) };
            return false;
        }
        Some(Err(_)) | None => {
            EVENTS_FAILED.fetch_add(1, Ordering::Relaxed);
        }
    }
    // SAFETY: the client's own arguments.
    unsafe { original(handle, bytes, len, reply_ctx) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button(plugin: &str, item: &str, tooltip: &str) -> (String, Button) {
        (
            plugin.into(),
            Button {
                key: format!("{plugin}\u{1f}{item}"),
                glyph: "B".into(),
                tooltip: tooltip.into(),
            },
        )
    }

    fn cell(n: &MountNode, p: rekindle_wire::PropId) -> Option<&Value> {
        n.cells.iter().find(|c| c.prop == p).map(|c| &c.value)
    }

    fn slot() -> SlotMap {
        let map: WireMap = serde_json::from_str(
            r#"{"slots": {"chat-bar.button": {
                "under": {"all": [{"kind": "div"}, {"parent_of": {"cell": {"prop": "type", "value": {"str": "input"}}}}]},
                "cells": [{"prop": "pressable", "value": {"bool": true}}]
            }}}"#,
        )
        .unwrap();
        map.slots.into_values().next().unwrap()
    }

    #[test]
    fn only_a_buffer_led_by_a_zero_u32_is_a_turn() {
        assert_eq!(op_stream(&[0, 0, 0, 0, 7, 8]), Some(&[7, 8][..]));
        assert_eq!(op_stream(&[0, 0, 0, 0]), Some(&[][..]));
        assert_eq!(op_stream(&[13, 0, 0, 0, b'@']), None);
        assert_eq!(op_stream(&[0, 0, 0]), None);
    }

    #[test]
    fn each_button_is_a_mount_with_a_plugin_handler_in_its_plugins_block() {
        let buttons = [
            button("dev.a", "open", "Open · dev.a"),
            button("dev.b", "toggle", ""),
            button("dev.a", "more", "More · dev.a"),
        ];
        let mounts = button_mounts(&buttons, &slot());
        let work = Work {
            mounts: mounts.clone(),
            ..Work::default()
        };
        work.check().unwrap();
        let roots: Vec<NodeId> = mounts.iter().map(|m| m.root).collect();
        // dev.a's two buttons share its block, and dev.b's is in the next.
        assert_eq!(roots[2], roots[0] + 2);
        assert!(roots[1] >= roots[0] + BLOCK && roots[0] >= PLUGIN_IDS);
        // The same buttons keep their nodes.
        let again: Vec<NodeId> = button_mounts(&buttons, &slot())
            .iter()
            .map(|m| m.root)
            .collect();
        assert_eq!(again, roots);

        let open = &mounts[0].nodes[0];
        assert_eq!(
            open.on_press,
            Some(Handler::Plugin {
                plugin: "dev.a".into(),
                surface: SLOT_CHAT_BAR_BUTTON.into(),
                element: "open".into(),
            })
        );
        assert_eq!(
            cell(open, a11y::LABEL),
            Some(&Value::Str("Open · dev.a".into()))
        );
        assert_eq!(cell(open, a11y::ROLE), Some(&Value::Str("button".into())));
        assert_eq!(
            cell(open, rekindle_wire::PropId::from_name("pressable").unwrap()),
            Some(&Value::Bool(true))
        );
        // A button without a tooltip is labelled by its id.
        assert_eq!(
            cell(&mounts[1].nodes[0], a11y::LABEL),
            Some(&Value::Str("toggle".into()))
        );
    }

    #[test]
    fn a_hidden_surface_is_dropped_and_a_restyled_one_gets_cells() {
        use rekindle_plugin_host::model::Color;
        let map: WireMap = serde_json::from_str(
            r#"{"surfaces": {
                "member-list": {"label": "Members"},
                "user-area": {"cell": {"prop": "type", "value": {"str": "area"}}}
            }}"#,
        )
        .unwrap();
        let style = |hidden, background| SurfaceStyle {
            hidden,
            opacity: None,
            radius: None,
            background,
        };
        let styles = [
            ("member-list".to_string(), style(true, None)),
            (
                "user-area".to_string(),
                SurfaceStyle {
                    opacity: Some(0.5),
                    ..style(
                        false,
                        Some(Color {
                            r: 0x12,
                            g: 0x34,
                            b: 0x56,
                            a: 0xff,
                        }),
                    )
                },
            ),
            // Not in the map, and a style that changes nothing.
            ("guild-list".to_string(), style(true, None)),
            ("channel-list".to_string(), style(false, None)),
        ];
        let (rules, styled) = surface_rules(&styles, &map.surfaces);
        assert_eq!(styled, ["member-list", "user-area"]);
        assert_eq!(rules[0].select, Selector::Label("Members".into()));
        assert_eq!(rules[0].action, RowAction::Drop);
        assert_eq!(
            rules[1].action,
            RowAction::Cells(vec![
                Cell::new(
                    rekindle_wire::ids::BACKGROUND,
                    rekindle_wire_inject::rgba(0x1234_56ff)
                ),
                Cell::new(OPACITY, Value::F64(0.5)),
            ])
        );
        assert_eq!(OPACITY.name(), Some("opacity"));
        assert_eq!(RADIUS.name(), Some("radius"));
    }
}
