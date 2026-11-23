//! The motion op stream: the `u32` words `ldh_motion_apply` and
//! `ldh_motion_view_motion` decode, and the words a `viewMotion` (740) cell
//! holds.
//!
//! Recovered from the Android host (build 356207): the decoder at
//! `0x5540b68`, the spec decoder at `0x55418b0` and the guest's encoders at
//! `0x5530604` and `0x55439a4`, which mirror each other. Each op is an
//! opcode from 1 to 32 followed by its operands, and [`layout`] gives the
//! operands of every opcode. An unknown opcode, a malformed spec or a
//! truncated op ends the stream, and the engine ignores the words after it.
//! [`Program`] keeps those words, so a cell re-encodes byte for byte.
//!
//! Which ops a view's program uses, and what most operands mean, is not
//! decoded. The real guest's buttons send one op 21 each:
//! `[21, 9, 5, 0xffffffff, 0]`, with the slot the button's `worklet` binds
//! first (9), 5, this view, and no spec.

use crate::Value;

/// A view field that means the view being programmed, in `view_motion`.
pub const THIS_VIEW: u32 = 0xffff_ffff;

/// The deepest a spec nests: 16 wrappers around a spec are accepted, 17
/// are not.
const MAX_DEPTH: usize = 16;
/// The most specs a sequence holds.
const MAX_SEQUENCE: u32 = 64;
/// The most words in op 16's list.
const MAX_OP16_WORDS: u32 = 16;

/// How a timed spec's progress maps to its value: the engine's six easing
/// kinds, numbered 0 to 5.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Easing {
    Linear,
    /// Quadratic.
    EaseInOut,
    /// Quadratic.
    EaseIn,
    /// Quadratic.
    EaseOut,
    EaseOutCubic,
    /// A CSS-style cubic Bezier through `(x1, y1, x2, y2)`.
    Bezier([f32; 4]),
}

impl Easing {
    /// The kind word, 0 to 5.
    pub fn kind(self) -> u32 {
        match self {
            Easing::Linear => 0,
            Easing::EaseInOut => 1,
            Easing::EaseIn => 2,
            Easing::EaseOut => 3,
            Easing::EaseOutCubic => 4,
            Easing::Bezier(_) => 5,
        }
    }

    /// The easing of kind `kind`, with the four control floats a Bezier
    /// uses. The other kinds ignore them.
    pub fn from_kind(kind: u32, points: [f32; 4]) -> Option<Easing> {
        Some(match kind {
            0 => Easing::Linear,
            1 => Easing::EaseInOut,
            2 => Easing::EaseIn,
            3 => Easing::EaseOut,
            4 => Easing::EaseOutCubic,
            5 => Easing::Bezier(points),
            _ => return None,
        })
    }

    /// `t` is in `0..=1`.
    pub fn apply(self, t: f32) -> f32 {
        match self {
            Easing::Linear => t,
            Easing::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                }
            }
            Easing::EaseIn => t * t,
            Easing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Easing::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            Easing::Bezier([x1, y1, x2, y2]) => bezier(x1, y1, x2, y2, t),
        }
    }
}

/// The curve's `y` where its `x` is `t`, solving for the parameter by
/// Newton's method with bisection as the fallback.
fn bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let at = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    let slope = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * a + 6.0 * u * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    let mut s = t;
    for _ in 0..8 {
        let d = slope(x1, x2, s);
        if d.abs() < 1e-6 {
            break;
        }
        s -= (at(x1, x2, s) - t) / d;
    }
    if !(0.0..=1.0).contains(&s) || (at(x1, x2, s) - t).abs() > 1e-4 {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        s = t;
        for _ in 0..32 {
            if at(x1, x2, s) < t {
                lo = s;
            } else {
                hi = s;
            }
            s = f32::midpoint(lo, hi);
        }
    }
    at(y1, y2, s)
}

/// An animation spec, the `S` operand of ops 2, 15, 21, 24, 28, 30 and 32.
/// Tags 1 to 7. Times are milliseconds and velocities units per second.
#[derive(Clone, Debug, PartialEq)]
pub enum Anim {
    /// Tag 1: `to, duration_ms, easing, x1, y1, x2, y2`. The guest writes
    /// the four Bezier floats only for kind 5, and zeros otherwise.
    Timing { to: f32, ms: f32, easing: Easing },
    /// Tag 2: an analytic damped spring. `flags` bit 0 clamps overshoot,
    /// bit 1 means `velocity` is given (otherwise the running velocity is
    /// kept), and bit 2 keeps a velocity pointing away from the target and
    /// snaps at the rest thresholds.
    Spring {
        to: f32,
        mass: f32,
        stiffness: f32,
        damping: f32,
        flags: u32,
        rest_displacement: f32,
        rest_speed: f32,
        velocity: f32,
    },
    /// Tag 3. `flags` bit 0 means `velocity` is given, bit 1 clamps to
    /// `min..=max`.
    Decay {
        flags: u32,
        velocity: f32,
        deceleration: f32,
        min: f32,
        max: f32,
    },
    /// Tag 4.
    Delay { ms: f32, then: Box<Anim> },
    /// Tag 5: up to 64 specs, one after another.
    Sequence(Vec<Anim>),
    /// Tag 6. A negative `count` never settles.
    Repeat {
        count: i32,
        reverse: bool,
        anim: Box<Anim>,
    },
    /// Tag 7: animates under reduced motion, where any other spec jumps to
    /// its end value.
    Exempt(Box<Anim>),
}

impl Anim {
    pub fn encode(&self, out: &mut Vec<u32>) {
        match self {
            Anim::Timing { to, ms, easing } => {
                let points = match easing {
                    Easing::Bezier(p) => *p,
                    _ => [0.0; 4],
                };
                out.extend([1, to.to_bits(), ms.to_bits(), easing.kind()]);
                out.extend(points.map(f32::to_bits));
            }
            Anim::Spring {
                to,
                mass,
                stiffness,
                damping,
                flags,
                rest_displacement,
                rest_speed,
                velocity,
            } => out.extend([
                2,
                to.to_bits(),
                mass.to_bits(),
                stiffness.to_bits(),
                damping.to_bits(),
                *flags,
                rest_displacement.to_bits(),
                rest_speed.to_bits(),
                velocity.to_bits(),
            ]),
            Anim::Decay {
                flags,
                velocity,
                deceleration,
                min,
                max,
            } => out.extend([
                3,
                *flags,
                velocity.to_bits(),
                deceleration.to_bits(),
                min.to_bits(),
                max.to_bits(),
            ]),
            Anim::Delay { ms, then } => {
                out.extend([4, ms.to_bits()]);
                then.encode(out);
            }
            Anim::Sequence(items) => {
                out.extend([5, items.len() as u32]);
                for a in items {
                    a.encode(out);
                }
            }
            Anim::Repeat {
                count,
                reverse,
                anim,
            } => {
                out.extend([6, *count as u32, u32::from(*reverse)]);
                anim.encode(out);
            }
            Anim::Exempt(anim) => {
                out.push(7);
                anim.encode(out);
            }
        }
    }

    fn decode(r: &mut Reader<'_>, depth: usize) -> Option<Anim> {
        if depth > MAX_DEPTH {
            return None;
        }
        let f = |r: &mut Reader<'_>| r.word().map(f32::from_bits);
        Some(match r.word()? {
            1 => {
                let (to, ms, kind) = (f(r)?, f(r)?, r.word()?);
                let points = [f(r)?, f(r)?, f(r)?, f(r)?];
                Anim::Timing {
                    to,
                    ms,
                    easing: Easing::from_kind(kind, points)?,
                }
            }
            2 => Anim::Spring {
                to: f(r)?,
                mass: f(r)?,
                stiffness: f(r)?,
                damping: f(r)?,
                flags: r.word()?,
                rest_displacement: f(r)?,
                rest_speed: f(r)?,
                velocity: f(r)?,
            },
            3 => Anim::Decay {
                flags: r.word()?,
                velocity: f(r)?,
                deceleration: f(r)?,
                min: f(r)?,
                max: f(r)?,
            },
            4 => Anim::Delay {
                ms: f(r)?,
                then: Box::new(Anim::decode(r, depth + 1)?),
            },
            5 => {
                let n = r.word()?;
                if n > MAX_SEQUENCE {
                    return None;
                }
                Anim::Sequence(
                    (0..n)
                        .map(|_| Anim::decode(r, depth + 1))
                        .collect::<Option<_>>()?,
                )
            }
            6 => Anim::Repeat {
                count: r.word()? as i32,
                reverse: r.word()? != 0,
                anim: Box::new(Anim::decode(r, depth + 1)?),
            },
            7 => Anim::Exempt(Box::new(Anim::decode(r, depth + 1)?)),
            _ => return None,
        })
    }
}

/// The shape of one operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operand {
    /// One word: an id, a view, a flag, or an `f32` as its bits.
    Word,
    /// A count, then that many words.
    Words,
    /// A spec.
    Anim,
    /// A presence word, 0 for none, then a spec if it isn't 0.
    MaybeAnim,
}

/// The operands of `opcode`, or `None` outside 1 to 32.
pub fn layout(opcode: u32) -> Option<&'static [Operand]> {
    use Operand::{Anim as S, MaybeAnim as O, Word as W, Words as V};
    Some(match opcode {
        13 => &[],
        7 | 9 | 10 | 12 | 19 | 20 | 23 | 25 | 31 => &[W],
        1 | 4 | 6 | 8 | 11 | 18 | 26 | 27 => &[W, W],
        5 | 17 => &[W, W, W],
        2 => &[W, W, S],
        3 => &[W, W, W, V],
        14 => &[W, W, W, W, W, W],
        15 => &[W, W, W, W, S],
        16 => &[W, W, W, W, V],
        21 => &[W, W, W, O],
        22 => &[W, W, V],
        24 => &[W, W, W, W, W, S],
        28 | 32 => &[W, W, W, S],
        29 => &[W, W, W, W, W],
        30 => &[W, W, W, V, O, O, O, O, O, W],
        _ => return None,
    })
}

/// One decoded operand.
#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    Word(u32),
    Words(Vec<u32>),
    Anim(Anim),
    MaybeAnim(Option<Anim>),
}

/// One op: its opcode and operands, in the shape [`layout`] gives.
#[derive(Clone, Debug, PartialEq)]
pub struct Op {
    pub opcode: u32,
    pub fields: Vec<Field>,
}

impl Op {
    /// The `i`th operand as a word.
    pub fn word(&self, i: usize) -> Option<u32> {
        match self.fields.get(i)? {
            Field::Word(w) => Some(*w),
            _ => None,
        }
    }

    /// The op's spec, if it carries one.
    pub fn anim(&self) -> Option<&Anim> {
        self.fields.iter().find_map(|f| match f {
            Field::Anim(a) | Field::MaybeAnim(Some(a)) => Some(a),
            _ => None,
        })
    }

    pub fn encode(&self, out: &mut Vec<u32>) {
        out.push(self.opcode);
        for f in &self.fields {
            match f {
                Field::Word(w) => out.push(*w),
                Field::Words(ws) => {
                    out.push(ws.len() as u32);
                    out.extend(ws);
                }
                Field::Anim(a) => a.encode(out),
                Field::MaybeAnim(None) => out.push(0),
                Field::MaybeAnim(Some(a)) => {
                    out.push(1);
                    a.encode(out);
                }
            }
        }
    }

    fn decode(r: &mut Reader<'_>) -> Option<Op> {
        let opcode = r.word()?;
        let mut fields = Vec::new();
        for operand in layout(opcode)? {
            fields.push(match operand {
                Operand::Word => Field::Word(r.word()?),
                Operand::Words => {
                    let n = r.word()?;
                    if opcode == 16 && n > MAX_OP16_WORDS {
                        return None;
                    }
                    Field::Words(r.take(n as usize)?.to_vec())
                }
                Operand::Anim => Field::Anim(Anim::decode(r, 0)?),
                Operand::MaybeAnim => Field::MaybeAnim(match r.word()? {
                    0 => None,
                    _ => Some(Anim::decode(r, 0)?),
                }),
            });
        }
        Some(Op { opcode, fields })
    }
}

struct Reader<'a> {
    words: &'a [u32],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn word(&mut self) -> Option<u32> {
        let w = *self.words.get(self.pos)?;
        self.pos += 1;
        Some(w)
    }

    fn take(&mut self, n: usize) -> Option<&'a [u32]> {
        let s = self.words.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
}

/// A decoded op stream: the ops up to where the engine's decoder stops,
/// and the words after that point, which the engine ignores.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Program {
    pub ops: Vec<Op>,
    pub tail: Vec<u32>,
}

impl Program {
    pub fn new(ops: Vec<Op>) -> Self {
        Program {
            ops,
            tail: Vec::new(),
        }
    }

    pub fn decode(words: &[u32]) -> Program {
        let mut r = Reader { words, pos: 0 };
        let mut ops = Vec::new();
        loop {
            let start = r.pos;
            match Op::decode(&mut r) {
                Some(op) => ops.push(op),
                None => {
                    return Program {
                        ops,
                        tail: words[start..].to_vec(),
                    };
                }
            }
        }
    }

    pub fn to_words(&self) -> Vec<u32> {
        let mut out = Vec::new();
        for op in &self.ops {
            op.encode(&mut out);
        }
        out.extend(&self.tail);
        out
    }

    /// The `viewMotion` cell as the guest writes it with no words table: an
    /// array of integers, one per word ([`Value::words`]).
    pub fn to_value(&self) -> Value {
        Value::words(&self.to_words())
    }

    /// Read a `viewMotion` cell, an array of integers or a tag 11
    /// ([`Value::as_words`]).
    pub fn from_value(value: &Value) -> Option<Program> {
        Some(Program::decode(&value.as_words()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing() -> Anim {
        Anim::Timing {
            to: 1.0,
            ms: 100.0,
            easing: Easing::Linear,
        }
    }

    #[test]
    fn every_opcode_round_trips() {
        for opcode in 1..=32 {
            let fields = layout(opcode)
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, o)| match o {
                    Operand::Word => Field::Word(i as u32 + 100),
                    Operand::Words => Field::Words(vec![1, 2, 3]),
                    Operand::Anim => Field::Anim(timing()),
                    Operand::MaybeAnim => Field::MaybeAnim((i % 2 == 0).then(timing)),
                })
                .collect();
            let program = Program::new(vec![Op { opcode, fields }]);
            let words = program.to_words();
            assert_eq!(Program::decode(&words), program, "op {opcode}");
        }
        assert_eq!(layout(0), None);
        assert_eq!(layout(33), None);
    }

    #[test]
    fn the_decoder_stops_where_the_engine_does() {
        // An unknown opcode, then a known one: both stay in the tail.
        let p = Program::decode(&[7, 3, 99, 7, 4]);
        assert_eq!(p.ops.len(), 1);
        assert_eq!(p.tail, [99, 7, 4]);
        assert_eq!(p.to_words(), [7, 3, 99, 7, 4]);
        // An easing kind of 6 rejects the op.
        let p = Program::decode(&[2, 1, 0, 1, 0, 0, 6, 0, 0, 0, 0]);
        assert!(p.ops.is_empty());
        // Op 16's list holds at most 16 words.
        let mut words = vec![16, 0, 0, 0, 0, 17];
        words.extend([0; 17]);
        assert!(Program::decode(&words).ops.is_empty());
    }

    #[test]
    fn specs_nest_sixteen_deep() {
        let nested = |n: usize| {
            let mut a = timing();
            for _ in 0..n {
                a = Anim::Exempt(Box::new(a));
            }
            let mut w = vec![2, 0, 0];
            a.encode(&mut w);
            w
        };
        assert_eq!(Program::decode(&nested(16)).ops.len(), 1);
        assert!(Program::decode(&nested(17)).ops.is_empty());
        let seq = |n: u32| {
            let mut w = vec![2, 0, 0, 5, n];
            for _ in 0..n {
                timing().encode(&mut w);
            }
            w
        };
        assert_eq!(Program::decode(&seq(64)).ops.len(), 1);
        assert!(Program::decode(&seq(65)).ops.is_empty());
    }

    #[test]
    fn easings_run_from_zero_to_one() {
        for e in [
            Easing::Linear,
            Easing::EaseInOut,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseOutCubic,
            Easing::Bezier([0.25, 0.1, 0.25, 1.0]),
        ] {
            assert!(e.apply(0.0).abs() < 1e-4, "{e:?}");
            assert!((e.apply(1.0) - 1.0).abs() < 1e-4, "{e:?}");
            assert_eq!(
                Easing::from_kind(e.kind(), [0.25, 0.1, 0.25, 1.0]).map(Easing::kind),
                Some(e.kind())
            );
        }
        assert_eq!(Easing::EaseIn.apply(0.5), 0.25);
        // CSS `ease` is past halfway at the midpoint.
        assert!(Easing::Bezier([0.25, 0.1, 0.25, 1.0]).apply(0.5) > 0.75);
    }
}
