//! The `worklet` (733) cell: which of the engine's native worklets runs on
//! a view, and its arguments.
//!
//! Recovered from the Android host (build 356207) and the platform's Java.
//! The cell is an array of `u32` words, like `viewMotion`. The first word
//! names the worklet by the low 32 bits of its 64-bit hash in the hoisted
//! worklet table (`0x60fd2c0`), and the rest are its arguments. The
//! platform hands the cell to the engine as one op 3
//! ([`Worklet::to_op`]). Each frame the worklet reads values from the
//! engine's table and writes channel values for its view.
//!
//! [`PressLook`] decodes the worklet the real guest puts on its buttons.
//! The others are kept as words.

use super::Channel;
use super::op::{Field, Op, code};
use crate::Value;

/// Worklet ids, the first word of a `worklet` cell.
pub mod ids {
    /// `libdiscord_motion_kit::press::press_look`, [`super::PressLook`].
    pub const PRESS_LOOK: u32 = 0x3b9f_a1cc;
    /// `libdiscord_motion_kit::transition::worklets::transition_style`: a
    /// count, then per channel its id, 1 for a colour, and four value ids,
    /// then specs. The real guest's text inputs use it.
    pub const TRANSITION_STYLE: u32 = 0x9524_55ed;
    /// `libdiscord_motion_kit::keyboard_follow::keyboard_pad`.
    pub const KEYBOARD_PAD: u32 = 0x0fd3_0db4;
}

/// A `worklet` cell: the worklet's id and its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worklet {
    pub id: u32,
    pub args: Vec<u32>,
}

impl Worklet {
    pub fn from_words(words: &[u32]) -> Option<Worklet> {
        let (&id, args) = words.split_first()?;
        Some(Worklet {
            id,
            args: args.to_vec(),
        })
    }

    pub fn to_words(&self) -> Vec<u32> {
        let mut w = Vec::with_capacity(1 + self.args.len());
        w.push(self.id);
        w.extend(&self.args);
        w
    }

    /// Read a `worklet` cell, an array of integers or a tag 11.
    pub fn from_value(value: &Value) -> Option<Worklet> {
        Worklet::from_words(&value.as_words()?)
    }

    /// The cell as the guest writes it with no words table, one integer a
    /// word ([`Value::words`]).
    pub fn to_value(&self) -> Value {
        Value::words(&self.to_words())
    }

    /// The op the Android platform sends `apply` for this cell on `view`:
    /// op 3 with kind 0, the id, and the arguments as its word list.
    pub fn to_op(&self, view: u32) -> Op {
        Op {
            opcode: code::BIND,
            fields: vec![
                Field::Word(view),
                Field::Word(0),
                Field::Word(self.id),
                Field::Words(self.args.clone()),
            ],
        }
    }
}

/// How a worklet moves one of its values: 13 words, read by `0x26e3280`.
/// `kind` 0 sets the value at once, 1 runs the spring, and 2 the timing
/// (or sets the value at once if `ms` isn't above 0). Both halves are
/// always written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transition {
    pub kind: u32,
    pub mass: f32,
    pub stiffness: f32,
    pub damping: f32,
    /// Non-zero clamps overshoot.
    pub clamp: u32,
    /// The engine takes its absolute value. Its sign bit is a second flag.
    pub rest_displacement: f32,
    pub rest_speed: f32,
    pub ms: f32,
    /// An easing kind, 0 to 5 ([`super::Easing`]).
    pub easing: u32,
    pub bezier: [f32; 4],
}

impl Transition {
    const WORDS: usize = 13;

    fn read(r: &mut Words<'_>) -> Option<Transition> {
        let kind = r.word()?;
        let t = Transition {
            kind,
            mass: r.f32()?,
            stiffness: r.f32()?,
            damping: r.f32()?,
            clamp: r.word()?,
            rest_displacement: r.f32()?,
            rest_speed: r.f32()?,
            ms: r.f32()?,
            easing: r.word()?,
            bezier: [r.f32()?, r.f32()?, r.f32()?, r.f32()?],
        };
        (kind <= 2 && t.easing < 6).then_some(t)
    }

    fn write(&self, out: &mut Vec<u32>) {
        out.extend([
            self.kind,
            self.mass.to_bits(),
            self.stiffness.to_bits(),
            self.damping.to_bits(),
            self.clamp,
            self.rest_displacement.to_bits(),
            self.rest_speed.to_bits(),
            self.ms.to_bits(),
            self.easing,
        ]);
        out.extend(self.bezier.map(f32::to_bits));
    }
}

/// One channel a [`PressLook`] moves between two values as the view is
/// pressed: 4 words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub channel: u32,
    /// The value at rest and fully pressed. For a colour channel these are
    /// `0xAARRGGBB`.
    pub rest: u32,
    pub pressed: u32,
    /// Non-zero writes the channel only while the press is above 0, so the
    /// view's own cell shows at rest.
    pub only_pressed: u32,
}

impl Look {
    /// The channel holds a colour: `Background`, `BorderColor`, `Tint` or
    /// `TextColor`.
    pub fn is_color(&self) -> bool {
        matches!(
            Channel::from_id(self.channel),
            Some(Channel::Background | Channel::BorderColor | Channel::Tint | Channel::TextColor)
        )
    }
}

/// `press_look`, the worklet on the real guest's buttons (`0x26bd0f0`, its
/// arguments read by `0x26e6094`).
///
/// The press value drives two eased values: `scale` shrinks the view,
/// and `look` moves each [`Look`] channel and the opacity from rest to
/// pressed. The press value is 0 or 1, and the guest binds it to the
/// platform's pressed input with an op 21 in `viewMotion`.
///
/// Each frame the worklet writes the view's `Background` (and any other
/// look channel) from its own colours, so a recolour of the `background`
/// cell alone doesn't reach a button that has one.
#[derive(Clone, Debug, PartialEq)]
pub struct PressLook {
    /// The value the press drives, 0 or 1. The worklet reads it at 0.5.
    pub press: u32,
    /// The value that eases the scale, `0xffffffff` for none.
    pub scale: u32,
    /// The value that eases the look channels and opacity, `0xffffffff` for
    /// none.
    pub look: u32,
    /// The `portalSlot` whose measured width sets the pressed scale.
    pub portal_slot: u32,
    /// The points the view shrinks by when pressed: the pressed scale is
    /// `(width - shrink) / width`.
    pub shrink: f32,
    /// The pressed scale while the width isn't measured.
    pub pressed_scale: f32,
    pub scale_transition: Transition,
    pub looks: Vec<Look>,
    /// The opacity at rest (NaN for 1) and pressed (NaN to leave it
    /// alone).
    pub opacity: [f32; 2],
    pub press_transition: Transition,
    /// The look's transition back to rest. A `kind` of 0 uses
    /// `press_transition`.
    pub release_transition: Transition,
    /// Words after `release_transition`, which the engine doesn't read.
    pub tail: Vec<u32>,
}

impl PressLook {
    /// The arguments of a [`Worklet`] whose id is [`ids::PRESS_LOOK`].
    /// `None` if they're short or a transition is invalid, as the engine
    /// rejects them.
    pub fn from_args(args: &[u32]) -> Option<PressLook> {
        let mut r = Words { w: args, pos: 0 };
        let (press, scale, look) = (r.word()?, r.word()?, r.word()?);
        let (portal_slot, shrink, pressed_scale) = (r.word()?, r.f32()?, r.f32()?);
        let scale_transition = Transition::read(&mut r)?;
        let n = r.word()? as usize;
        if n > r.left() / 4 {
            return None;
        }
        let mut looks = Vec::new();
        for _ in 0..n {
            looks.push(Look {
                channel: r.word()?,
                rest: r.word()?,
                pressed: r.word()?,
                only_pressed: r.word()?,
            });
        }
        Some(PressLook {
            press,
            scale,
            look,
            portal_slot,
            shrink,
            pressed_scale,
            scale_transition,
            looks,
            opacity: [r.f32()?, r.f32()?],
            press_transition: Transition::read(&mut r)?,
            release_transition: Transition::read(&mut r)?,
            tail: r.w[r.pos..].to_vec(),
        })
    }

    pub fn from_worklet(w: &Worklet) -> Option<PressLook> {
        (w.id == ids::PRESS_LOOK)
            .then(|| PressLook::from_args(&w.args))
            .flatten()
    }

    pub fn to_args(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(9 + 4 * self.looks.len() + 3 * Transition::WORDS);
        out.extend([
            self.press,
            self.scale,
            self.look,
            self.portal_slot,
            self.shrink.to_bits(),
            self.pressed_scale.to_bits(),
        ]);
        self.scale_transition.write(&mut out);
        out.push(self.looks.len() as u32);
        for l in &self.looks {
            out.extend([l.channel, l.rest, l.pressed, l.only_pressed]);
        }
        out.extend(self.opacity.map(f32::to_bits));
        self.press_transition.write(&mut out);
        self.release_transition.write(&mut out);
        out.extend(&self.tail);
        out
    }

    pub fn to_worklet(&self) -> Worklet {
        Worklet {
            id: ids::PRESS_LOOK,
            args: self.to_args(),
        }
    }

    /// Replace the colours of the colour looks with `f`'s answer, where it
    /// has one. True if any changed.
    pub fn recolor(&mut self, mut f: impl FnMut(u32) -> Option<u32>) -> bool {
        let mut changed = false;
        for l in self.looks.iter_mut().filter(|l| l.is_color()) {
            for c in [&mut l.rest, &mut l.pressed] {
                if let Some(to) = f(*c)
                    && to != *c
                {
                    *c = to;
                    changed = true;
                }
            }
        }
        changed
    }
}

struct Words<'a> {
    w: &'a [u32],
    pos: usize,
}

impl Words<'_> {
    fn word(&mut self) -> Option<u32> {
        let w = *self.w.get(self.pos)?;
        self.pos += 1;
        Some(w)
    }

    fn f32(&mut self) -> Option<f32> {
        self.word().map(f32::from_bits)
    }

    fn left(&self) -> usize {
        self.w.len() - self.pos
    }
}
