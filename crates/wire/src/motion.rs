//! A model of Phoenix's native motion engine (`ldh_motion_*`), so rekindle
//! can drive animation the way the real host does.
//!
//! On Phoenix the host owns animation. The guest declares what a view rests
//! at and how it should move, and the native engine interpolates each
//! channel per frame. No guest code runs per frame. [`Motion`] is that
//! engine for the mock: the platform feeds it from the cells a turn writes
//! and writes what it returns each frame into `view_props`.
//!
//! # Recovered
//!
//! Read out of `liblibdiscord_wasm_host.so` (build 356207):
//!
//! - the ownership split: the engine lives in the native host
//!   (`worklets/src/driver.rs`) and only the platform's calls reach it;
//! - the 26 channels, their ids 1 to 26 and names ([`Channel`]), and the
//!   resting defaults of channels 1 to 7 (translate 0, scale 1, rotate 0,
//!   opacity 1);
//! - `declare(view, channel, value)` sets a resting value. A value equal to
//!   the default removes the entry, NaN is ignored, an id outside 1..26 is
//!   rejected, and channels 8 to 26 are accepted and ignored ([`Motion::declare`]);
//! - `view_motion(view, &[u32])` takes the op stream `apply` takes
//!   ([`op`]), from `PropId` `viewMotion` (740). The cell is an array of
//!   integers, one per word, and the Android platform cuts each to 32 bits
//!   (anything that isn't a number is 0) before it calls
//!   `nativeViewMotion`. The real guest sends the array as tag 7, five
//!   words and more included, because its writer had no words table
//!   ([`Value::words`]);
//! - the per-frame loop: `wants_frame`, then `frame(dt)` returning a `u32`
//!   word stream, with `dt` in milliseconds;
//! - `set_reduced(bool)` is the only reduced-motion entry, and a change
//!   re-evaluates every view;
//! - the timing `PropIds` are fields of a spec, not live channels: `spring`
//!   (470), `stiffness` (484), `damping` (142), `mass` (290), `timing`
//!   (521), `ms` (310), `delay` (145), `easing` (170).
//!
//! # Modelled
//!
//! - Which ops a [`Spec`] is ([`Spec::to_program`]). Each entry is an op
//!   24 for this view, a recovered op whose spec `view_motion` keeps per
//!   view. What its four words mean is rekindle's.
//! - The interpolation: a timed curve with the engine's easings and a
//!   damped spring ([`Curve`]), and when a transition counts as settled.
//! - What a resting-value change does under a spec (it animates from the
//!   value shown), and that an entry with `enterFrom` (180) animates in the
//!   first time a spec reaches a view.
//! - The frame word stream's layout ([`FrameOut`]). The real stream's field
//!   meanings are on the Java side and not decoded.
//! - Which `PropId` each channel paints ([`Channel::prop`]): paired by name.
//! - Only channels 1 to 7 animate here, since only those take a resting
//!   value. On Phoenix a `worklet` (733) binds channels to slots, and its
//!   switches bind `Background` (8), `BorderColor` (10), `BorderWidth` (16),
//!   `Height` (18) and `Width` (19) beside 1, 2, 3, 6 and 7.

#![allow(
    clippy::float_cmp,
    reason = "declare compares with the default exactly, and a settled transition lands on its target exactly"
)]

pub mod op;

use std::collections::BTreeMap;

use crate::tree::NodeId;
use crate::{PropId, Value};

pub use op::Easing;
use op::{Anim, Field, Op, Program};

/// The `PropIds` the engine's platform reads. Recovered ids; the pairing of
/// `enterFrom` with a spec entry is modelled.
pub mod ids {
    use crate::PropId;

    /// A view's motion spec, the cell the platform passes to `view_motion`.
    pub const VIEW_MOTION: PropId = PropId(740);
    /// The value a channel animates in from.
    pub const ENTER_FROM: PropId = PropId(180);
    pub const SPRING: PropId = PropId(470);
    pub const STIFFNESS: PropId = PropId(484);
    pub const DAMPING: PropId = PropId(142);
    pub const MASS: PropId = PropId(290);
    pub const TIMING: PropId = PropId(521);
    pub const MS: PropId = PropId(310);
    pub const DELAY: PropId = PropId(145);
    pub const EASING: PropId = PropId(170);
    /// The per-view reduced-motion cell. The engine's own switch is
    /// [`Motion::set_reduced`](super::Motion::set_reduced).
    pub const REDUCED_MOTION: PropId = PropId(619);

    pub const TRANSLATE_X: PropId = PropId(535);
    pub const TRANSLATE_Y: PropId = PropId(536);
    pub const SCALE: PropId = PropId(428);
    pub const SCALE_X: PropId = PropId(430);
    pub const SCALE_Y: PropId = PropId(431);
    pub const ROTATE: PropId = PropId(415);
    pub const OPACITY: PropId = PropId(322);
}

/// A channel the engine animates. The ids and names are the engine's own,
/// from its `Display` table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u32)]
pub enum Channel {
    TranslateX = 1,
    TranslateY,
    Scale,
    ScaleX,
    ScaleY,
    Rotate,
    Opacity,
    Background,
    Radius,
    BorderColor,
    Tint,
    TextColor,
    VideoZoom,
    VideoX,
    VideoY,
    BorderWidth,
    PaddingBottom,
    Height,
    Width,
    Progress,
    WindowX,
    WindowY,
    SwipeOffset,
    TextStep,
    TextReveal,
    InputScale,
}

impl Channel {
    /// Every channel, by id.
    pub const ALL: [Channel; 26] = [
        Channel::TranslateX,
        Channel::TranslateY,
        Channel::Scale,
        Channel::ScaleX,
        Channel::ScaleY,
        Channel::Rotate,
        Channel::Opacity,
        Channel::Background,
        Channel::Radius,
        Channel::BorderColor,
        Channel::Tint,
        Channel::TextColor,
        Channel::VideoZoom,
        Channel::VideoX,
        Channel::VideoY,
        Channel::BorderWidth,
        Channel::PaddingBottom,
        Channel::Height,
        Channel::Width,
        Channel::Progress,
        Channel::WindowX,
        Channel::WindowY,
        Channel::SwipeOffset,
        Channel::TextStep,
        Channel::TextReveal,
        Channel::InputScale,
    ];

    /// The channel with this id, or `None` outside 1..=26.
    pub fn from_id(id: u32) -> Option<Channel> {
        Channel::ALL
            .get(usize::try_from(id.checked_sub(1)?).ok()?)
            .copied()
    }

    pub fn id(self) -> u32 {
        self as u32
    }

    pub fn name(self) -> &'static str {
        match self {
            Channel::TranslateX => "TranslateX",
            Channel::TranslateY => "TranslateY",
            Channel::Scale => "Scale",
            Channel::ScaleX => "ScaleX",
            Channel::ScaleY => "ScaleY",
            Channel::Rotate => "Rotate",
            Channel::Opacity => "Opacity",
            Channel::Background => "Background",
            Channel::Radius => "Radius",
            Channel::BorderColor => "BorderColor",
            Channel::Tint => "Tint",
            Channel::TextColor => "TextColor",
            Channel::VideoZoom => "VideoZoom",
            Channel::VideoX => "VideoX",
            Channel::VideoY => "VideoY",
            Channel::BorderWidth => "BorderWidth",
            Channel::PaddingBottom => "PaddingBottom",
            Channel::Height => "Height",
            Channel::Width => "Width",
            Channel::Progress => "Progress",
            Channel::WindowX => "WindowX",
            Channel::WindowY => "WindowY",
            Channel::SwipeOffset => "SwipeOffset",
            Channel::TextStep => "TextStep",
            Channel::TextReveal => "TextReveal",
            Channel::InputScale => "InputScale",
        }
    }

    /// The resting value `declare` falls back to. Only channels 1 to 7 have
    /// one, and only those take a declared value.
    pub fn rest_default(self) -> Option<f32> {
        match self {
            Channel::TranslateX | Channel::TranslateY | Channel::Rotate => Some(0.0),
            Channel::Scale | Channel::ScaleX | Channel::ScaleY | Channel::Opacity => Some(1.0),
            _ => None,
        }
    }

    /// The `PropId` whose cell the channel paints, paired by name. Modelled:
    /// the real engine's output goes to the platform's views, not to cells.
    pub fn prop(self) -> Option<PropId> {
        Some(match self {
            Channel::TranslateX => ids::TRANSLATE_X,
            Channel::TranslateY => ids::TRANSLATE_Y,
            Channel::Scale => ids::SCALE,
            Channel::ScaleX => ids::SCALE_X,
            Channel::ScaleY => ids::SCALE_Y,
            Channel::Rotate => ids::ROTATE,
            Channel::Opacity => ids::OPACITY,
            Channel::Background => crate::ids::BACKGROUND,
            Channel::Radius => PropId(388),
            Channel::BorderColor => crate::ids::BORDER_COLOR,
            Channel::Tint => crate::ids::TINT,
            Channel::TextColor => crate::ids::COLOR,
            Channel::BorderWidth => PropId(107),
            Channel::Height => PropId(235),
            Channel::Width => PropId(553),
            Channel::Progress => PropId(384),
            _ => return None,
        })
    }

    /// The animated channel a `PropId`'s cell declares a resting value for.
    pub fn declared_by(prop: PropId) -> Option<Channel> {
        Channel::ALL
            .into_iter()
            .filter(|c| c.rest_default().is_some())
            .find(|c| c.prop() == Some(prop))
    }
}

/// Modelled. How a channel moves to its target: the two specs the model
/// runs, out of the engine's seven ([`op::Anim`]). The engine's spring is
/// analytic, and this one is stepped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    /// `ms` long after `delay` ms, along `easing`.
    Timing { ms: f32, delay: f32, easing: Easing },
    /// A damped spring: `stiffness` and `damping` per unit `mass`, in
    /// seconds.
    Spring {
        stiffness: f32,
        damping: f32,
        mass: f32,
    },
}

impl Curve {
    /// The curve as a spec: a timing, inside a delay if `delay` isn't 0,
    /// or a spring with no flags. `to` is NaN, which the model reads as the
    /// channel's resting value.
    pub fn to_anim(self) -> Anim {
        match self {
            Curve::Timing { ms, delay, easing } => {
                let timing = Anim::Timing {
                    to: f32::NAN,
                    ms,
                    easing,
                };
                if delay == 0.0 {
                    timing
                } else {
                    Anim::Delay {
                        ms: delay,
                        then: Box::new(timing),
                    }
                }
            }
            Curve::Spring {
                stiffness,
                damping,
                mass,
            } => Anim::Spring {
                to: f32::NAN,
                mass,
                stiffness,
                damping,
                flags: 0,
                rest_displacement: 0.0,
                rest_speed: 0.0,
                velocity: 0.0,
            },
        }
    }

    /// The curve a spec of [`Curve::to_anim`]'s shape is. `None` for any
    /// other spec.
    pub fn from_anim(anim: &Anim) -> Option<Curve> {
        Some(match anim {
            Anim::Timing { ms, easing, .. } => Curve::Timing {
                ms: *ms,
                delay: 0.0,
                easing: *easing,
            },
            Anim::Delay { ms: delay, then } => match **then {
                Anim::Timing { ms, easing, .. } => Curve::Timing {
                    ms,
                    delay: *delay,
                    easing,
                },
                _ => return None,
            },
            Anim::Spring {
                mass,
                stiffness,
                damping,
                ..
            } => Curve::Spring {
                stiffness: *stiffness,
                damping: *damping,
                mass: *mass,
            },
            _ => return None,
        })
    }
}

/// One channel of a [`Spec`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    pub channel: Channel,
    /// The value the channel animates in from when the spec first reaches a
    /// view. `None` animates only changes of the resting value.
    pub enter_from: Option<f32>,
    pub curve: Curve,
}

/// Modelled. A view's motion spec: which channels animate, and how.
///
/// On the wire it is the `viewMotion` (740) cell, an op stream in the
/// engine's grammar ([`Spec::to_program`]). The grammar is recovered and
/// the ops are rekindle's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spec {
    pub entries: Vec<Entry>,
}

/// The opcode a [`Spec`] entry is written as.
const ENTRY_OP: u32 = 24;

impl Spec {
    pub fn new() -> Self {
        Spec::default()
    }

    /// Animate `channel` in from `from`, and its later changes, along `curve`.
    #[must_use]
    pub fn enter(mut self, channel: Channel, from: f32, curve: Curve) -> Self {
        self.set(Entry {
            channel,
            enter_from: Some(from),
            curve,
        });
        self
    }

    /// Animate changes of `channel`'s resting value along `curve`.
    #[must_use]
    pub fn on_change(mut self, channel: Channel, curve: Curve) -> Self {
        self.set(Entry {
            channel,
            enter_from: None,
            curve,
        });
        self
    }

    fn set(&mut self, entry: Entry) {
        self.entries.retain(|e| e.channel != entry.channel);
        self.entries.push(entry);
    }

    pub fn entry(&self, channel: Channel) -> Option<&Entry> {
        self.entries.iter().find(|e| e.channel == channel)
    }

    /// The `viewMotion` cell: [`Spec::to_program`] as an array of integers.
    pub fn to_value(&self) -> Value {
        self.to_program().to_value()
    }

    /// Read a `viewMotion` cell. `None` if it holds no words. A cell with
    /// no entries of rekindle's, such as the real guest's, reads as an
    /// empty spec.
    pub fn from_value(value: &Value) -> Option<Spec> {
        Some(Spec::from_program(&Program::from_value(value)?))
    }

    /// One op 24 per entry: this view, the channel id, 1 if it enters,
    /// `enterFrom` as `f32` bits, 0, and the curve as a spec
    /// ([`Curve::to_anim`]).
    pub fn to_program(&self) -> Program {
        Program::new(
            self.entries
                .iter()
                .map(|e| Op {
                    opcode: ENTRY_OP,
                    fields: vec![
                        Field::Word(op::THIS_VIEW),
                        Field::Word(e.channel.id()),
                        Field::Word(u32::from(e.enter_from.is_some())),
                        Field::Word(e.enter_from.unwrap_or(0.0).to_bits()),
                        Field::Word(0),
                        Field::Anim(e.curve.to_anim()),
                    ],
                })
                .collect(),
        )
    }

    /// The entries in a program's ops 24 for this view. Other ops, and ops
    /// 24 whose channel or spec [`Spec::to_program`] doesn't write, are
    /// skipped.
    pub fn from_program(program: &Program) -> Spec {
        let mut spec = Spec::new();
        for o in &program.ops {
            if let Some(e) = entry_of(o) {
                spec.set(e);
            }
        }
        spec
    }

    pub fn to_words(&self) -> Vec<u32> {
        self.to_program().to_words()
    }

    pub fn from_words(words: &[u32]) -> Spec {
        Spec::from_program(&Program::decode(words))
    }
}

fn entry_of(o: &Op) -> Option<Entry> {
    if o.opcode != ENTRY_OP || o.word(0)? != op::THIS_VIEW {
        return None;
    }
    let enter_from = o.word(3).map(f32::from_bits)?;
    Some(Entry {
        channel: Channel::from_id(o.word(1)?)?,
        enter_from: (o.word(2)? != 0).then_some(enter_from),
        curve: Curve::from_anim(o.anim()?)?,
    })
}

/// A running transition of one channel of one view.
#[derive(Clone, Copy, Debug)]
struct Running {
    from: f32,
    to: f32,
    value: f32,
    /// Units per second, for springs.
    velocity: f32,
    /// Milliseconds since the transition started.
    elapsed: f32,
    curve: Curve,
    /// Reduced motion: the next frame lands on `to`.
    jump: bool,
}

impl Running {
    fn new(from: f32, to: f32, velocity: f32, curve: Curve, jump: bool) -> Self {
        Running {
            from,
            to,
            value: from,
            velocity,
            elapsed: 0.0,
            curve,
            jump,
        }
    }

    /// Advance by `dt` ms. True once settled, with `value` on `to`.
    fn step(&mut self, dt: f32) -> bool {
        if self.jump {
            self.value = self.to;
            self.velocity = 0.0;
            return true;
        }
        self.elapsed += dt;
        let settled = match self.curve {
            Curve::Timing { ms, delay, easing } => {
                let t = self.elapsed - delay;
                if t >= ms {
                    true
                } else {
                    let p = if t <= 0.0 { 0.0 } else { easing.apply(t / ms) };
                    self.value = self.from + (self.to - self.from) * p;
                    false
                }
            }
            Curve::Spring {
                stiffness,
                damping,
                mass,
            } => {
                // Semi-implicit Euler in steps of at most 1/240 s.
                let mass = mass.max(1e-3);
                let mut left = dt / 1000.0;
                while left > 0.0 {
                    let h = left.min(1.0 / 240.0);
                    let accel =
                        (-stiffness * (self.value - self.to) - damping * self.velocity) / mass;
                    self.velocity += accel * h;
                    self.value += self.velocity * h;
                    left -= h;
                }
                let eps = 1e-3 * (self.to - self.from).abs().max(1.0);
                (self.value - self.to).abs() <= eps && self.velocity.abs() <= eps * 10.0
            }
        };
        if settled {
            self.value = self.to;
            self.velocity = 0.0;
        }
        settled
    }
}

/// Modelled. What one [`Motion::frame`] produced, read back from its words.
/// The real frame stream is a 32-opcode `apply` stream, not yet decoded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameOut {
    /// Each running channel's value this frame.
    pub values: Vec<(NodeId, Channel, f32)>,
    /// Channels that reached their target this frame.
    pub settled: Vec<(NodeId, Channel)>,
    pub wants_frame: bool,
}

impl FrameOut {
    /// Read a frame's words (modelled layout): a count and that many
    /// `(view, channel, f32 bits)` triples, a count and that many
    /// `(view, channel)` pairs, then `wants_frame` as 0 or 1.
    pub fn from_words(words: &[u32]) -> Option<FrameOut> {
        let mut it = words.iter().copied();
        let mut out = FrameOut::default();
        for _ in 0..it.next()? {
            let (view, ch, v) = (
                it.next()?,
                Channel::from_id(it.next()?)?,
                f32::from_bits(it.next()?),
            );
            out.values.push((view, ch, v));
        }
        for _ in 0..it.next()? {
            out.settled
                .push((it.next()?, Channel::from_id(it.next()?)?));
        }
        out.wants_frame = it.next()? != 0;
        it.next().is_none().then_some(out)
    }
}

/// Counters over the engine's life, in the spirit of `ldh_motion_stats`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Specs `view_motion` accepted.
    pub specs: u64,
    /// Transitions started.
    pub started: u64,
    /// Transitions that reached their target.
    pub settled: u64,
    /// Of those, the ones reduced motion jumped.
    pub jumped: u64,
    pub frames: u64,
}

/// The engine: resting values, specs and running transitions per view.
#[derive(Clone, Debug, Default)]
pub struct Motion {
    rest: BTreeMap<(NodeId, Channel), f32>,
    specs: BTreeMap<NodeId, Spec>,
    anims: BTreeMap<(NodeId, Channel), Running>,
    reduced: bool,
    stats: Stats,
}

impl Motion {
    pub fn new() -> Self {
        Self::default()
    }

    /// The resting value of a channel: declared, or its default.
    pub fn resting(&self, view: NodeId, channel: Channel) -> Option<f32> {
        self.rest
            .get(&(view, channel))
            .copied()
            .or(channel.rest_default())
    }

    /// The value shown now: the running transition's, or the resting value.
    pub fn value(&self, view: NodeId, channel: Channel) -> Option<f32> {
        self.anims
            .get(&(view, channel))
            .map(|a| a.value)
            .or_else(|| self.resting(view, channel))
    }

    /// `ldh_motion_declare`: set a channel's resting value. Under a spec
    /// entry for the channel, a change animates from the value shown.
    pub fn declare(&mut self, view: NodeId, channel: u32, value: f32) {
        let Some(channel) = Channel::from_id(channel) else {
            return;
        };
        let Some(default) = channel.rest_default() else {
            return;
        };
        if value.is_nan() {
            return;
        }
        let old = self.resting(view, channel).unwrap_or(default);
        if value == default {
            self.rest.remove(&(view, channel));
        } else {
            self.rest.insert((view, channel), value);
        }
        let Some(entry) = self
            .specs
            .get(&view)
            .and_then(|s| s.entry(channel))
            .copied()
        else {
            return;
        };
        let running = self.anims.get(&(view, channel)).copied();
        if running.map_or(old, |a| a.to) == value {
            return;
        }
        let (from, velocity) = running.map_or((old, 0.0), |a| (a.value, a.velocity));
        self.start(view, channel, from, value, velocity, entry.curve);
    }

    /// `ldh_motion_view_motion`: set a view's spec from its words. The first
    /// spec a view takes animates its entering channels in. Empty or
    /// malformed words clear the spec. Running transitions finish.
    pub fn view_motion(&mut self, view: NodeId, words: &[u32]) {
        let spec = Spec::from_words(words);
        if spec.entries.is_empty() {
            self.specs.remove(&view);
            return;
        }
        self.stats.specs += 1;
        let first = !self.specs.contains_key(&view);
        if first {
            for e in &spec.entries {
                if let (Some(from), Some(to)) = (e.enter_from, self.resting(view, e.channel)) {
                    self.start(view, e.channel, from, to, 0.0, e.curve);
                }
            }
        }
        self.specs.insert(view, spec);
    }

    fn start(
        &mut self,
        view: NodeId,
        channel: Channel,
        from: f32,
        to: f32,
        velocity: f32,
        curve: Curve,
    ) {
        self.stats.started += 1;
        self.anims.insert(
            (view, channel),
            Running::new(from, to, velocity, curve, self.reduced),
        );
    }

    /// `ldh_motion_set_reduced`. A change re-evaluates every view: with
    /// reduced motion on, each running transition lands on its target on
    /// the next frame, and so does every one started while it stays on.
    pub fn set_reduced(&mut self, reduced: bool) {
        if self.reduced == reduced {
            return;
        }
        self.reduced = reduced;
        for a in self.anims.values_mut() {
            a.jump = reduced;
        }
    }

    pub fn reduced(&self) -> bool {
        self.reduced
    }

    /// `ldh_motion_wants_frame`: true while a transition runs.
    pub fn wants_frame(&self) -> bool {
        !self.anims.is_empty()
    }

    /// `ldh_motion_drop_view`: forget a view.
    pub fn drop_view(&mut self, view: NodeId) {
        self.rest.retain(|(v, _), _| *v != view);
        self.anims.retain(|(v, _), _| *v != view);
        self.specs.remove(&view);
    }

    /// `ldh_motion_frame`: advance every transition by `dt` ms and return
    /// the frame's words ([`FrameOut::from_words`]).
    pub fn frame(&mut self, dt: f32) -> Vec<u32> {
        self.stats.frames += 1;
        let mut values = Vec::new();
        let mut settled = Vec::new();
        for (&(view, channel), a) in &mut self.anims {
            let jumped = a.jump;
            if a.step(dt.max(0.0)) {
                settled.push((view, channel));
                self.stats.settled += 1;
                self.stats.jumped += u64::from(jumped);
            }
            values.extend([view, channel.id(), a.value.to_bits()]);
        }
        for key in &settled {
            self.anims.remove(key);
        }
        let mut out = vec![(values.len() / 3) as u32];
        out.extend(values);
        out.push(settled.len() as u32);
        out.extend(settled.iter().flat_map(|(v, c)| [*v, c.id()]));
        out.push(u32::from(self.wants_frame()));
        out
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: NodeId = 7;

    fn fade() -> Curve {
        Curve::Timing {
            ms: 200.0,
            delay: 0.0,
            easing: Easing::EaseOutCubic,
        }
    }

    fn spring() -> Curve {
        Curve::Spring {
            stiffness: 300.0,
            damping: 35.0,
            mass: 1.0,
        }
    }

    fn frame(m: &mut Motion, dt: f32) -> FrameOut {
        FrameOut::from_words(&m.frame(dt)).expect("the frame's words read back")
    }

    /// Frames of `dt` ms until the engine stops asking, with the channel's
    /// value after each.
    fn run(m: &mut Motion, channel: Channel, dt: f32) -> Vec<f32> {
        let mut seen = Vec::new();
        for _ in 0..1000 {
            if !m.wants_frame() {
                return seen;
            }
            frame(m, dt);
            seen.push(m.value(VIEW, channel).unwrap());
        }
        panic!("never settled: {seen:?}");
    }

    #[test]
    fn the_ids_are_the_recovered_names() {
        for (id, name) in [
            (ids::VIEW_MOTION, "viewMotion"),
            (ids::ENTER_FROM, "enterFrom"),
            (ids::SPRING, "spring"),
            (ids::STIFFNESS, "stiffness"),
            (ids::DAMPING, "damping"),
            (ids::MASS, "mass"),
            (ids::TIMING, "timing"),
            (ids::MS, "ms"),
            (ids::DELAY, "delay"),
            (ids::EASING, "easing"),
            (ids::REDUCED_MOTION, "reducedMotion"),
        ] {
            assert_eq!(id.name(), Some(name));
        }
        // Each channel that paints a cell is paired with the PropId of the
        // same name.
        for c in Channel::ALL {
            let Some(p) = c.prop() else { continue };
            let want = match c {
                Channel::TextColor => "color",
                Channel::BorderColor => "borderColor",
                _ => c.name(),
            };
            assert!(
                p.name().unwrap().eq_ignore_ascii_case(want),
                "{c:?} paints {p:?}"
            );
        }
    }

    #[test]
    fn channels_are_numbered_one_to_twenty_six() {
        assert_eq!(Channel::from_id(0), None);
        assert_eq!(Channel::from_id(27), None);
        for (i, c) in Channel::ALL.into_iter().enumerate() {
            assert_eq!(c.id(), i as u32 + 1);
            assert_eq!(Channel::from_id(c.id()), Some(c));
        }
        assert_eq!(Channel::Opacity.id(), 7);
        assert_eq!(Channel::InputScale.id(), 26);
    }

    #[test]
    fn declare_keeps_only_values_off_the_default() {
        let mut m = Motion::new();
        m.declare(VIEW, Channel::Opacity.id(), 0.5);
        assert_eq!(m.resting(VIEW, Channel::Opacity), Some(0.5));
        assert_eq!(m.rest.len(), 1);
        m.declare(VIEW, Channel::Opacity.id(), 1.0);
        assert!(m.rest.is_empty());
        m.declare(VIEW, Channel::Scale.id(), f32::NAN);
        m.declare(VIEW, Channel::Background.id(), 3.0);
        m.declare(VIEW, 0, 3.0);
        m.declare(VIEW, 27, 3.0);
        assert!(m.rest.is_empty());
        assert_eq!(m.resting(VIEW, Channel::Background), None);
        // Without a spec nothing animates.
        m.declare(VIEW, Channel::TranslateY.id(), 10.0);
        assert!(!m.wants_frame());
    }

    #[test]
    fn a_spec_survives_the_wire_and_the_words() {
        let delayed = Curve::Timing {
            ms: 100.0,
            delay: 50.0,
            easing: Easing::Bezier([0.2, 0.0, 0.0, 1.0]),
        };
        let spec = Spec::new()
            .enter(Channel::Opacity, 0.0, fade())
            .enter(Channel::TranslateY, 12.0, spring())
            .on_change(Channel::Scale, delayed);
        assert_eq!(Spec::from_value(&spec.to_value()), Some(spec.clone()));
        assert_eq!(Spec::from_words(&spec.to_words()), spec);
        let bytes = spec.to_value().to_bytes();
        assert_eq!(bytes[0], 7, "the words go as tag 7");
        assert_eq!(
            crate::decode(&bytes).map(|v| Spec::from_value(&v)),
            Ok(Some(spec.clone()))
        );
        // A tag 11 reads the same.
        let words = spec.to_words();
        assert_eq!(
            Spec::from_value(&Value::U32Array(words.clone())),
            Some(spec.clone())
        );
        // Each entry is one op 24 for this view.
        let program = spec.to_program();
        assert_eq!(program.ops.len(), 3);
        assert!(
            program
                .ops
                .iter()
                .all(|o| o.opcode == 24 && o.word(0) == Some(op::THIS_VIEW))
        );
        // `to` is NaN, so the programs compare by their words.
        assert_eq!(Program::decode(&words).to_words(), program.to_words());
        // A truncated last op is dropped, and the words stay in the tail.
        let cut = Program::decode(&words[..words.len() - 1]);
        assert_eq!(cut.ops.len(), 2);
        assert_eq!(cut.to_words(), words[..words.len() - 1]);
        assert_eq!(Spec::from_program(&cut).entries.len(), 2);
    }

    #[test]
    fn opacity_rises_monotonically_and_settles_on_the_target() {
        let mut m = Motion::new();
        m.view_motion(
            VIEW,
            &Spec::new().enter(Channel::Opacity, 0.0, fade()).to_words(),
        );
        assert!(m.wants_frame());
        assert_eq!(m.value(VIEW, Channel::Opacity), Some(0.0));
        let seen = run(&mut m, Channel::Opacity, 16.0);
        // 200 ms at 16 ms a frame: 13 frames.
        assert_eq!(seen.len(), 13, "{seen:?}");
        assert!(seen.windows(2).all(|w| w[1] > w[0]), "{seen:?}");
        assert!(seen[0] > 0.0 && seen[seen.len() - 2] < 1.0);
        assert_eq!(*seen.last().unwrap(), 1.0);
        assert!(!m.wants_frame());
        assert_eq!(
            m.stats(),
            Stats {
                specs: 1,
                started: 1,
                settled: 1,
                jumped: 0,
                frames: 13
            }
        );
        // A settled engine emits nothing.
        assert_eq!(frame(&mut m, 16.0), FrameOut::default());
    }

    #[test]
    fn the_frame_words_carry_each_value_and_the_settles() {
        let mut m = Motion::new();
        m.view_motion(
            VIEW,
            &Spec::new()
                .enter(
                    Channel::Opacity,
                    0.0,
                    Curve::Timing {
                        ms: 32.0,
                        delay: 0.0,
                        easing: Easing::Linear,
                    },
                )
                .to_words(),
        );
        let f = frame(&mut m, 16.0);
        assert_eq!(f.values, vec![(VIEW, Channel::Opacity, 0.5)]);
        assert!(f.settled.is_empty() && f.wants_frame);
        let f = frame(&mut m, 16.0);
        assert_eq!(
            f,
            FrameOut {
                values: vec![(VIEW, Channel::Opacity, 1.0)],
                settled: vec![(VIEW, Channel::Opacity)],
                wants_frame: false
            }
        );
    }

    #[test]
    fn a_delay_holds_the_start() {
        let mut m = Motion::new();
        let curve = Curve::Timing {
            ms: 100.0,
            delay: 50.0,
            easing: Easing::Linear,
        };
        m.view_motion(
            VIEW,
            &Spec::new().enter(Channel::Opacity, 0.0, curve).to_words(),
        );
        frame(&mut m, 40.0);
        assert_eq!(m.value(VIEW, Channel::Opacity), Some(0.0));
        frame(&mut m, 60.0);
        assert!((m.value(VIEW, Channel::Opacity).unwrap() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_spring_slides_in_without_overshoot_and_settles() {
        let mut m = Motion::new();
        m.view_motion(
            VIEW,
            &Spec::new()
                .enter(Channel::TranslateY, 12.0, spring())
                .to_words(),
        );
        let seen = run(&mut m, Channel::TranslateY, 16.0);
        assert!(seen.windows(2).all(|w| w[1] <= w[0]), "{seen:?}");
        assert!(seen.iter().all(|v| *v >= 0.0));
        assert_eq!(*seen.last().unwrap(), 0.0);
        assert!(seen.len() > 5 && seen.len() < 60, "{} frames", seen.len());
    }

    #[test]
    fn a_resting_change_under_a_spec_animates_from_the_value_shown() {
        let mut m = Motion::new();
        m.view_motion(
            VIEW,
            &Spec::new().on_change(Channel::Opacity, fade()).to_words(),
        );
        assert!(!m.wants_frame(), "no entering channel");
        m.declare(VIEW, Channel::Opacity.id(), 0.2);
        assert_eq!(m.value(VIEW, Channel::Opacity), Some(1.0));
        frame(&mut m, 100.0);
        let mid = m.value(VIEW, Channel::Opacity).unwrap();
        assert!(mid < 1.0 && mid > 0.2);
        // Retargeted mid-flight: it turns around from where it is.
        m.declare(VIEW, Channel::Opacity.id(), 1.0);
        assert_eq!(m.value(VIEW, Channel::Opacity), Some(mid));
        let seen = run(&mut m, Channel::Opacity, 16.0);
        assert!(seen.windows(2).all(|w| w[1] >= w[0]), "{seen:?}");
        assert_eq!(*seen.last().unwrap(), 1.0);
        // The same value again starts nothing.
        m.declare(VIEW, Channel::Opacity.id(), 1.0);
        assert!(!m.wants_frame());
    }

    #[test]
    fn only_the_first_spec_enters() {
        let mut m = Motion::new();
        let words = Spec::new().enter(Channel::Opacity, 0.0, fade()).to_words();
        m.view_motion(VIEW, &words);
        run(&mut m, Channel::Opacity, 16.0);
        m.view_motion(VIEW, &words);
        assert!(!m.wants_frame());
        // Cleared and set again, it enters again.
        m.view_motion(VIEW, &[]);
        m.view_motion(VIEW, &words);
        assert!(m.wants_frame());
    }

    #[test]
    fn reduced_motion_jumps_to_the_target() {
        let mut m = Motion::new();
        m.set_reduced(true);
        m.view_motion(
            VIEW,
            &Spec::new()
                .enter(Channel::Opacity, 0.0, fade())
                .enter(Channel::TranslateY, 12.0, spring())
                .to_words(),
        );
        let f = frame(&mut m, 0.0);
        assert_eq!(
            f.values,
            vec![
                (VIEW, Channel::TranslateY, 0.0),
                (VIEW, Channel::Opacity, 1.0)
            ]
        );
        assert_eq!(f.settled.len(), 2);
        assert!(!f.wants_frame && !m.wants_frame());
        assert_eq!(m.stats().jumped, 2);
    }

    #[test]
    fn turning_reduced_motion_on_lands_running_transitions() {
        let mut m = Motion::new();
        m.view_motion(
            VIEW,
            &Spec::new().enter(Channel::Opacity, 0.0, fade()).to_words(),
        );
        frame(&mut m, 16.0);
        assert!(m.value(VIEW, Channel::Opacity).unwrap() < 1.0);
        m.set_reduced(true);
        let f = frame(&mut m, 1.0);
        assert_eq!(f.values, vec![(VIEW, Channel::Opacity, 1.0)]);
        assert!(!m.wants_frame());
        // And off again: transitions animate.
        m.set_reduced(false);
        m.declare(VIEW, Channel::Opacity.id(), 0.0);
        frame(&mut m, 16.0);
        assert!(m.value(VIEW, Channel::Opacity).unwrap() > 0.0);
    }

    #[test]
    fn dropping_a_view_stops_its_frames() {
        let mut m = Motion::new();
        m.view_motion(
            VIEW,
            &Spec::new().enter(Channel::Opacity, 0.0, fade()).to_words(),
        );
        m.drop_view(VIEW);
        assert!(!m.wants_frame());
        assert_eq!(m.value(VIEW, Channel::Opacity), Some(1.0));
    }
}
