//! The guest's motion, run by the host library's own engine.
//!
//! The Android platform (`mh.u`, decompiled from the 356207 base APK) owns
//! one engine from `ldh_motion_new_native_first(4, NULL)`. It hands the
//! engine the turn's Motion op (`apply`), each view's `worklet` (733) and
//! `panWorklet` (734) cells as op 3 or op 4, `viewMotion` (740) and the
//! view's resting transform and opacity (`declare`), and its inputs: the
//! pressed state, scroll offsets, the density and the views' sizes and
//! rects. Each frame it calls `tick` and writes the channel values the
//! engine returns onto its views (`mh.e0.d`), and sends the guest the
//! frame's word pairs as UI event op 8. phoenix-view does the same with the
//! same exports, on the UI thread, once per GPUI frame while the engine
//! wants one.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use rekindle_wire::turn::NodeOp;
use rekindle_wire::{NodeId, PropId, Turn, Value};

/// The `PropIds` the platform hands the engine (`modules.qg`), and the
/// channel `declare` takes for each resting value.
mod cell {
    use rekindle_wire::PropId;

    pub const WORKLET: PropId = PropId(733);
    pub const PAN_WORKLET: PropId = PropId(734);
    pub const VIEW_MOTION: PropId = PropId(740);

    /// `translateX`, `translateY`, `scaleFactor`, `scaleX`, `scaleY`,
    /// `rotation`, `opacity` go to channels 1 to 7 (`p.s3`).
    pub fn declared(p: PropId) -> Option<u32> {
        Some(match p.name()? {
            "translateX" => 1,
            "translateY" => 2,
            "scaleFactor" => 3,
            "scaleX" => 4,
            "scaleY" => 5,
            "rotation" => 6,
            "opacity" => 7,
            _ => return None,
        })
    }
}

/// The engine's channels phoenix-view paints.
pub mod channel {
    pub const TRANSLATE_X: u32 = 1;
    pub const TRANSLATE_Y: u32 = 2;
    pub const SCALE: u32 = 3;
    pub const OPACITY: u32 = 7;
    pub const BACKGROUND: u32 = 8;
    pub const RADIUS: u32 = 9;
    pub const BORDER_COLOR: u32 = 10;
    pub const TINT: u32 = 11;
    pub const TEXT_COLOR: u32 = 12;
    pub const BORDER_WIDTH: u32 = 16;
    pub const PADDING_BOTTOM: u32 = 17;
    pub const HEIGHT: u32 = 18;
    pub const WIDTH: u32 = 19;
}

/// `ldh_motion_system` kinds the Android platform reports.
mod input {
    pub const SCROLL_X: u32 = 3;
    pub const SCROLL_Y: u32 = 4;
    pub const PRESSED: u32 = 5;
    pub const DENSITY: u32 = 9;
    pub const OFFLOAD: u32 = 10;
}

/// What the engine has done, for the log and the self-test.
#[derive(Clone, Copy, Debug, Default)]
pub struct Counts {
    pub applied: u64,
    pub view_motions: u64,
    pub worklets: u64,
    pub declares: u64,
    pub ticks: u64,
    pub writes: u64,
    pub events: u64,
}

pub struct MotionHost {
    /// False with `--no-motion`: no engine is made.
    pub enabled: bool,
    engine: Option<phoenix_guest::Motion>,
    /// Creating the engine failed once; it isn't tried again.
    failed: bool,
    density: f32,
    /// The last value the engine wrote for each channel of each view, as
    /// `mh.e0.d` leaves it on the Android view until the next write.
    vals: HashMap<NodeId, Vec<(u32, u32)>>,
    /// A frame is due: the engine wants one, or an input or program came
    /// in, as the platform schedules one after each (`mh.u.n`).
    due: bool,
    last: Option<Instant>,
    /// The time between the display's refreshes, in ms: the first frame's
    /// step.
    pub frame_ms: f32,
    /// Views with a `viewMotion` cell, whose size the engine is told, and
    /// the size it was last told.
    tracked: HashSet<NodeId>,
    sizes: HashMap<NodeId, (f32, f32)>,
    /// The views whose rect the engine asked for, and the rect it was last
    /// told.
    want_layout: HashSet<NodeId>,
    layouts: HashMap<NodeId, [f32; 8]>,
    /// The view held pressed, and each scroll's offset as last reported.
    pressed: Vec<NodeId>,
    scrolls: HashMap<(NodeId, u32), f32>,
    /// UI event op 8's pairs, for the platform to send.
    pub outbox: Vec<Vec<(u32, u32)>>,
    pub counts: Counts,
    /// Every (view, channel) the engine has written, for the self-test.
    pub seen: HashSet<(NodeId, u32)>,
    /// The views written since the platform last took them.
    pub touched: Vec<NodeId>,
    described: u32,
}

impl MotionHost {
    pub fn new() -> MotionHost {
        MotionHost {
            enabled: true,
            engine: None,
            failed: false,
            density: 1.0,
            vals: HashMap::new(),
            due: false,
            last: None,
            frame_ms: 1000.0 / 60.0,
            tracked: HashSet::new(),
            sizes: HashMap::new(),
            want_layout: HashSet::new(),
            layouts: HashMap::new(),
            pressed: Vec::new(),
            scrolls: HashMap::new(),
            outbox: Vec::new(),
            counts: Counts::default(),
            seen: HashSet::new(),
            touched: Vec::new(),
            described: 0,
        }
    }

    /// The engine, created on first use after the library has loaded, as
    /// `mh.u.e` creates it: reduced motion off, the density, no offloaded
    /// keyframes (the platform runs every frame through `tick`), and the
    /// inputs `mh.x` reports at start.
    fn engine(&mut self) -> Option<&phoenix_guest::Motion> {
        if self.engine.is_none() && !self.failed && self.enabled {
            let lib = phoenix_guest::library()?;
            match lib.motion() {
                Ok(m) => {
                    m.set_reduced(false);
                    m.system(input::DENSITY, 0, self.density, 0.0);
                    m.system(input::OFFLOAD, 0, 0.0, 0.0);
                    for (kind, value) in [(1, 0.0), (6, 0.0), (14, 1.0), (17, 0.0)] {
                        m.system(kind, 0, value, 0.0);
                    }
                    crate::guest::log("[motion] engine created");
                    self.engine = Some(m);
                }
                Err(e) => {
                    self.failed = true;
                    crate::guest::log(&format!("[motion] no engine: {e:#}"));
                }
            }
        }
        self.engine.as_ref()
    }

    pub fn set_density(&mut self, density: f32) {
        self.density = density;
        if let Some(m) = &self.engine {
            m.system(input::DENSITY, 0, density, 0.0);
            self.due = true;
        }
    }

    /// Hand the engine what a turn programs, op by op in the order the
    /// turn holds them, as the platform applies each op's cells.
    pub fn feed(&mut self, turn: &Turn) {
        let mut turn = turn.clone();
        if turn.own_words().is_err() {
            return;
        }
        let touches = turn.ops.iter().any(|op| match op {
            NodeOp::Motion { .. } | NodeOp::Delete { .. } => true,
            NodeOp::Create { props, .. } | NodeOp::Update { props, .. } => props
                .iter()
                .any(|(p, _)| motion_cell(*p)),
            _ => false,
        });
        if !touches || self.engine().is_none() {
            return;
        }
        self.describe(&turn);
        for op in &turn.ops {
            match op {
                NodeOp::Motion { words } => {
                    self.counts.applied += 1;
                    self.with(|m| m.apply(words));
                }
                NodeOp::Create { node, props, .. } | NodeOp::Update { node, props } => {
                    for (p, v) in props {
                        self.cell(*node, *p, v);
                    }
                }
                NodeOp::Delete { nodes } => {
                    for n in nodes {
                        self.with(|m| m.drop_view(*n));
                        self.vals.remove(n);
                        self.tracked.remove(n);
                        self.sizes.remove(n);
                        self.layouts.remove(n);
                        self.want_layout.remove(n);
                        self.scrolls.retain(|(v, _), _| v != n);
                        self.pressed.retain(|p| p != n);
                    }
                }
                _ => {}
            }
        }
        self.due = true;
    }

    /// A line in the log for each of the first turns that program the
    /// engine: the worklets named, the `viewMotion` opcodes and the Motion
    /// op's opcodes.
    fn describe(&mut self, turn: &Turn) {
        use rekindle_wire::motion::op::Program;
        if self.described >= 200 {
            return;
        }
        self.described += 1;
        let mut parts = Vec::new();
        for op in &turn.ops {
            match op {
                NodeOp::Motion { words } => {
                    let p = Program::decode(words);
                    let codes: Vec<u32> = p.ops.iter().map(|o| o.opcode).collect();
                    parts.push(format!("Motion {codes:?}{}", if p.tail.is_empty() { "" } else { " +tail" }));
                }
                NodeOp::Create { node, props, .. } | NodeOp::Update { node, props } => {
                    for (p, v) in props {
                        match *p {
                            cell::WORKLET | cell::PAN_WORKLET => {
                                let id = v.as_words().and_then(|w| w.first().copied());
                                parts.push(format!("#{node} {} {id:x?}", p.name().unwrap_or("?")));
                            }
                            cell::VIEW_MOTION => {
                                let p = v.as_words().map(|w| Program::decode(&w));
                                let codes: Option<Vec<u32>> =
                                    p.map(|p| p.ops.iter().map(|o| o.opcode).collect());
                                parts.push(format!("#{node} viewMotion {codes:?}"));
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        if !parts.is_empty() {
            crate::guest::log(&format!("[motion] {}", parts.join("; ")));
        }
    }

    fn with(&self, f: impl FnOnce(&phoenix_guest::Motion)) {
        if let Some(m) = &self.engine {
            f(m);
        }
    }

    fn cell(&mut self, node: NodeId, p: PropId, v: &Value) {
        let words = v.as_words();
        match p {
            cell::WORKLET | cell::PAN_WORKLET => {
                // `mh.u.b`: `[3, view, kind, id, n - 1, args]`, or
                // `[4, view, kind]` for an empty cell.
                let kind = u32::from(p == cell::PAN_WORKLET);
                let op = match words.as_deref() {
                    Some([id, args @ ..]) => {
                        let mut op = vec![3, node, kind, *id, args.len() as u32];
                        op.extend(args);
                        op
                    }
                    _ => vec![4, node, kind],
                };
                self.counts.worklets += 1;
                self.with(|m| m.apply(&op));
            }
            cell::VIEW_MOTION => {
                self.counts.view_motions += 1;
                let spec = words.filter(|w| !w.is_empty());
                self.with(|m| m.view_motion(node, spec.as_deref()));
                if spec.is_some() {
                    self.tracked.insert(node);
                } else {
                    self.tracked.remove(&node);
                    self.sizes.remove(&node);
                }
            }
            p => {
                if let Some(ch) = cell::declared(p) {
                    let value = match v {
                        Value::F64(f) => *f as f32,
                        v => v.as_int().map_or(f32::NAN, |i| i as f32),
                    };
                    self.counts.declares += 1;
                    self.with(|m| m.declare(node, ch, value));
                }
            }
        }
    }

    /// Every channel value the engine last wrote for a view.
    pub fn written(&self, node: NodeId) -> Vec<(u32, u32)> {
        self.vals.get(&node).cloned().unwrap_or_default()
    }

    /// The views held pressed: the pressed view and the descendants its
    /// `dispatchSetPressed` reaches. Empty on release.
    pub fn press(&mut self, nodes: Vec<NodeId>) {
        if self.pressed == nodes {
            return;
        }
        for old in std::mem::take(&mut self.pressed) {
            self.with(|m| m.system(input::PRESSED, old, 0.0, 0.0));
        }
        for n in &nodes {
            self.with(|m| m.system(input::PRESSED, *n, 1.0, 0.0));
        }
        self.pressed = nodes;
        self.due = self.engine.is_some();
    }

    /// Whether a view has a `viewMotion` cell.
    pub fn tracks(&self, node: NodeId) -> bool {
        self.tracked.contains(&node)
    }

    /// A scroll view's offset in dp (`mh.g1`: kind 3 horizontal, 4
    /// vertical).
    pub fn scrolled(&mut self, node: NodeId, horizontal: bool, offset: f32) {
        let kind = if horizontal { input::SCROLL_X } else { input::SCROLL_Y };
        if self.scrolls.get(&(node, kind)) == Some(&offset) {
            return;
        }
        self.scrolls.insert((node, kind), offset);
        self.with(|m| m.system(kind, node, offset, 0.0));
        self.due = self.engine.is_some();
    }

    /// Tell the engine the sizes of the views it tracks and the rects of
    /// the views it asked for, from the last paint. `rect` gives a node's
    /// rect against the window with its own and its ancestors' motion
    /// translation taken out, and its parent's.
    pub fn report_layout(
        &mut self,
        mut rect: impl FnMut(NodeId) -> Option<([f32; 4], Option<[f32; 4]>)>,
    ) {
        if self.engine.is_none() {
            return;
        }
        let mut changed = false;
        let tracked: Vec<NodeId> = self.tracked.iter().copied().collect();
        for n in tracked {
            let Some(([_, _, w, h], _)) = rect(n) else {
                continue;
            };
            if w <= 0.0 || h <= 0.0 || self.sizes.get(&n) == Some(&(w, h)) {
                continue;
            }
            self.sizes.insert(n, (w, h));
            self.with(|m| m.size(n, w, h));
            changed = true;
        }
        let wanted: Vec<NodeId> = self.want_layout.iter().copied().collect();
        for n in wanted {
            let Some((r, parent)) = rect(n) else { continue };
            let pr = parent.unwrap_or([0.0, 0.0, 0.0, 0.0]);
            let out = [r[0], r[1], r[2], r[3], r[0] - pr[0], r[1] - pr[1], pr[2], pr[3]];
            if self.layouts.get(&n) == Some(&out) {
                continue;
            }
            self.layouts.insert(n, out);
            self.with(|m| m.layout(n, out));
            changed = true;
        }
        if changed {
            if crate::view::frame_logging() {
                crate::guest::log("[frame] motion layout changed");
            }
            self.due = true;
        }
    }

    /// Whether the engine runs.
    pub fn running(&self) -> bool {
        self.engine.is_some()
    }

    /// Whether a view's size or rect is to be reported.
    pub fn has_layout_work(&self) -> bool {
        self.engine.is_some() && !(self.tracked.is_empty() && self.want_layout.is_empty())
    }

    /// Whether a frame should run now.
    pub fn due(&self) -> bool {
        self.due && self.engine.is_some()
    }

    /// One frame: `tick` with the time since the last one, the writes kept,
    /// the event pairs queued. Returns whether the engine wants another.
    pub fn frame(&mut self) -> bool {
        let Some(m) = &self.engine else {
            return false;
        };
        let now = Instant::now();
        let dt = self
            .last
            .map_or(self.frame_ms, |t| now.duration_since(t).as_secs_f32() * 1000.0);
        self.last = Some(now);
        let t = m.tick(dt);
        let mut changed = 0;
        let pairs = t.events.len();
        if t.has_trips {
            for line in m.trips().lines() {
                crate::guest::log(&format!("[motion] {line}"));
            }
        }
        let (views, _) = m.layout_views();
        self.want_layout = views.into_iter().collect();
        self.counts.ticks += 1;
        self.counts.writes += t.writes.len() as u64;
        for (view, ch, bits) in &t.writes {
            let vals = self.vals.entry(*view).or_default();
            // A write of the value the view already holds changes nothing
            // drawn, as setting an Android view's property to its value
            // doesn't invalidate it.
            match vals.iter_mut().find(|(c, _)| c == ch) {
                Some(v) if v.1 == *bits => {}
                Some(v) => {
                    v.1 = *bits;
                    self.touched.push(*view);
                    changed += 1;
                }
                None => {
                    vals.push((*ch, *bits));
                    self.touched.push(*view);
                    changed += 1;
                }
            }
            self.seen.insert((*view, *ch));
        }
        if !t.events.is_empty() {
            if self.counts.events < 5 {
                crate::guest::log(&format!("[motion] op 8 to the guest: {:x?}", t.events));
            }
            self.counts.events += 1;
            self.outbox.push(t.events);
        }
        let wants = t.wants_frame || t.frame_wants || m.wants_frame();
        if crate::view::frame_logging() {
            crate::guest::log(&format!(
                "[frame] motion tick {dt:.1} ms, took {:.3} ms: {} writes, {changed} changed, {} event pairs, wants {}/{}/{}",
                now.elapsed().as_secs_f64() * 1000.0,
                t.writes.len(),
                pairs,
                t.wants_frame,
                t.frame_wants,
                m.wants_frame()
            ));
        }
        self.due = wants;
        if !wants {
            self.last = None;
        }
        wants
    }
}

/// A channel value as the dump shows it: a colour as `#aarrggbb`, the
/// rest as a float.
pub fn show(ch: u32, bits: u32) -> String {
    use channel::{BACKGROUND, BORDER_COLOR, TEXT_COLOR, TINT};
    if matches!(ch, BACKGROUND | BORDER_COLOR | TINT | TEXT_COLOR) {
        format!("#{bits:08x}")
    } else {
        format!("{}", f32::from_bits(bits))
    }
}

fn motion_cell(p: PropId) -> bool {
    matches!(p, cell::WORKLET | cell::PAN_WORKLET | cell::VIEW_MOTION) || cell::declared(p).is_some()
}
