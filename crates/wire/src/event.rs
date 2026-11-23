//! Events from the platform to the guest: the wire's other direction.
//!
//! # Recovered
//!
//! The crossing. The Android host's `ldh_dispatch(handle, ptr, len,
//! reply_ctx) -> bool` hands the guest one buffer `(ptr, len)`, through slot
//! `+0x20` of the guest handle's vtable. What comes back is null when the
//! guest took the event and otherwise an error object, which `ldh_dispatch`
//! hands to `reply_ctx` and the Android JNI path raises as an
//! `IllegalStateException`. An event's dispatch returns no data.
//! `ldh_reply(slot, ptr, len)` copies bytes into a slot as `{len, ptr, len}`
//! and has no caller in the host. It fills the reply slot of a host call
//! ([`crate::frame`]).
//!
//! The buffer. The guest's `ld_event` (`0x17e77e8`) reads a leading `u32`.
//! If it is 0, the buffer is a UI event and `0x1730f6c` decodes the rest.
//! Anything else is a host frame ([`crate::frame`]). After the `u32` 0
//! comes the version byte `0x01`, an op byte and the op's body, which must
//! end at the end of the buffer. The names are the guest's, from its event
//! telemetry, and the meanings are from the Android platform's Java
//! writers:
//!
//! | op | name | body |
//! |---:|---|---|
//! | 1 | `ui` | `i32` node, `u16` `PropId`, value |
//! | 2 | `back` | none: the system back gesture or button |
//! | 3 | `keyCommand` | `u16` length and UTF-8: a key command the guest registered with the host module `KeyCommands` |
//! | 4 | `insets` | 5 × `f32` dp: top, bottom, left, right, keyboard height |
//! | 5 | `window` | 3 × `f32`: width and height in dp, then the density |
//! | 6 | `frameInterval` | `f64`: the display's frame interval in ms, `1000 / refresh rate` |
//! | 7 | `frame` | none: a display frame, sent when the guest asked for one |
//! | 8 | `motion` | `u32` n, n × (`u32`, `u32`): the pairs the motion engine's frame step hands the guest |
//! | 9 | `navSides` | 2 × `f32` dp: the navigation bar's left and right insets |
//!
//! Op 1 is an event on a node ([`Event`]). Its `PropId` is the event's name
//! ([`names`]), one of those the node's Create listed in `on` (320), and the
//! value is its data. A node id is written as a 32-bit register and read as
//! an `i32`. rekindle's own ids sit above `i32::MAX`, and the guest never
//! receives an event for them.
//!
//! The guest's app drops a `frameInterval` that isn't a positive, normal
//! number, and clamps `navSides` to zero or more after dropping one that
//! isn't finite ([`UiEvent::as_guest_takes`]). On Android the platform
//! queues its events and sends them in one BATCH frame.
//!
//! What op 8's pairs mean isn't read. [`UiEvent::Motion`] keeps them as
//! words.
//!
//! # Modelled
//!
//! A reply to an event. Phoenix's dispatch returns no data, so a consumed
//! event's reply ([`FILTER_CONSUME`] with bytes) is rekindle's own model: the
//! mock's guest reads it as one value. The verdicts an event filter returns
//! ([`FILTER_PASS`] and the rest) are the mock's seam, in the place a detour
//! on `ldh_dispatch` would sit on Phoenix.

use crate::value::{Cursor, DecodeError};
use crate::{NodeId, PropId, Value};

/// The leading `u32` of a UI event.
pub const UI: u32 = 0;
/// The byte after it.
pub const VERSION: u8 = 1;

/// The `PropIds` that name events, the ones the Android host's guest listens
/// for by name.
pub mod names {
    use crate::PropId;

    pub const PRESS: PropId = PropId(614);
    pub const TAP: PropId = PropId(496);
    pub const LONG_PRESS: PropId = PropId(642);
    pub const CHANGE: PropId = PropId(575);
    pub const SUBMIT: PropId = PropId(628);
    pub const FOCUS: PropId = PropId(584);
    pub const BLUR: PropId = PropId(102);
    pub const LAYOUT: PropId = PropId(595);
}

/// An event on a node: UI event op 1.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// The node the platform's hit test found.
    pub node: NodeId,
    /// The event's name, as a `PropId`.
    pub name: PropId,
    /// What the event carries. Null for a press.
    pub value: Value,
}

impl Event {
    /// A press on `node`, with no data.
    #[must_use]
    pub fn press(node: NodeId) -> Self {
        Event {
            node,
            name: names::PRESS,
            value: Value::Null,
        }
    }

    #[must_use]
    pub fn is_press(&self) -> bool {
        self.name == names::PRESS
    }

    /// The whole buffer, from the leading `u32` 0.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(14);
        out.extend_from_slice(&UI.to_le_bytes());
        out.extend_from_slice(&[VERSION, 1]);
        out.extend_from_slice(&self.node.to_le_bytes());
        out.extend_from_slice(&self.name.0.to_le_bytes());
        self.value.encode(&mut out);
        out
    }

    /// Decode a buffer that holds an event on a node. Any other UI event or
    /// a host frame is an error.
    pub fn from_bytes(buf: &[u8]) -> Result<Event, DecodeError> {
        match UiEvent::from_bytes(buf)? {
            UiEvent::Node(e) => Ok(e),
            _ => Err(DecodeError::BadFraming),
        }
    }
}

/// The op byte of each UI event.
pub mod ops {
    pub const UI: u8 = 1;
    pub const BACK: u8 = 2;
    pub const KEY_COMMAND: u8 = 3;
    pub const INSETS: u8 = 4;
    pub const WINDOW: u8 = 5;
    pub const FRAME_INTERVAL: u8 = 6;
    pub const FRAME: u8 = 7;
    pub const MOTION: u8 = 8;
    pub const NAV_SIDES: u8 = 9;
}

/// The window's insets in dp, as UI event op 4 carries them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub top: f32,
    pub bottom: f32,
    pub left: f32,
    pub right: f32,
    /// The keyboard's height: the IME's bottom inset.
    pub keyboard: f32,
}

/// One UI event, as the guest's decoder reads it.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    /// Op 1, `ui`: an event on a node.
    Node(Event),
    /// Op 2, `back`: the system back gesture or button.
    Back,
    /// Op 3, `keyCommand`: the name of a key command the guest registered
    /// with the host module `KeyCommands`.
    KeyCommand(String),
    /// Op 4, `insets`.
    Insets(Insets),
    /// Op 5, `window`: the window's size in dp and its density.
    Window {
        width: f32,
        height: f32,
        density: f32,
    },
    /// Op 6, `frameInterval`: the display's frame interval in ms.
    FrameInterval(f64),
    /// Op 7, `frame`: a display frame the guest asked for.
    Frame,
    /// Op 8, `motion`: the word pairs the motion engine's frame step hands
    /// the guest. What a pair means isn't read.
    Motion(Vec<(u32, u32)>),
    /// Op 9, `navSides`: the navigation bar's left and right insets in dp.
    NavSides { left: f32, right: f32 },
}

/// Whether `buf` starts with the `u32` 0 of a UI event. Anything else is a
/// host frame.
#[must_use]
pub fn is_ui(buf: &[u8]) -> bool {
    buf.first_chunk::<4>()
        .is_some_and(|h| u32::from_le_bytes(*h) == UI)
}

fn read_f32s<const N: usize>(cur: &mut Cursor<'_>) -> Result<[f32; N], DecodeError> {
    let mut out = [0.0; N];
    for v in &mut out {
        *v = f32::from_bits(cur.u32()?);
    }
    Ok(out)
}

impl UiEvent {
    /// The op byte.
    #[must_use]
    pub fn op(&self) -> u8 {
        match self {
            UiEvent::Node(_) => ops::UI,
            UiEvent::Back => ops::BACK,
            UiEvent::KeyCommand(_) => ops::KEY_COMMAND,
            UiEvent::Insets(_) => ops::INSETS,
            UiEvent::Window { .. } => ops::WINDOW,
            UiEvent::FrameInterval(_) => ops::FRAME_INTERVAL,
            UiEvent::Frame => ops::FRAME,
            UiEvent::Motion(_) => ops::MOTION,
            UiEvent::NavSides { .. } => ops::NAV_SIDES,
        }
    }

    /// The guest's name for the op, from its event telemetry.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            UiEvent::Node(_) => "ui",
            UiEvent::Back => "back",
            UiEvent::KeyCommand(_) => "keyCommand",
            UiEvent::Insets(_) => "insets",
            UiEvent::Window { .. } => "window",
            UiEvent::FrameInterval(_) => "frameInterval",
            UiEvent::Frame => "frame",
            UiEvent::Motion(_) => "motion",
            UiEvent::NavSides { .. } => "navSides",
        }
    }

    /// The event as the guest's app receives it from its decoder
    /// (`0x4c7974c`), or `None` if the app drops it. A `frameInterval` that
    /// isn't a positive, normal number is dropped. A `navSides` with a side
    /// that isn't finite is dropped, and a negative side becomes 0.
    #[must_use]
    pub fn as_guest_takes(&self) -> Option<UiEvent> {
        match *self {
            UiEvent::FrameInterval(ms) => {
                (ms.is_normal() && ms > 0.0).then_some(UiEvent::FrameInterval(ms))
            }
            UiEvent::NavSides { left, right } => {
                (left.is_finite() && right.is_finite()).then_some(UiEvent::NavSides {
                    left: left.max(0.0),
                    right: right.max(0.0),
                })
            }
            _ => Some(self.clone()),
        }
    }

    /// Decode a UI event, from the leading `u32` 0. A body that doesn't end
    /// at the end of the buffer is an error, as on the guest.
    pub fn from_bytes(buf: &[u8]) -> Result<UiEvent, DecodeError> {
        let mut cur = Cursor::new(buf);
        if cur.u32()? != UI || cur.u8()? != VERSION {
            return Err(DecodeError::BadFraming);
        }
        let event = match cur.u8()? {
            ops::UI => {
                let node = cur.u32()?;
                let name = PropId(cur.u16()?);
                let value = cur.value()?;
                UiEvent::Node(Event { node, name, value })
            }
            ops::BACK => UiEvent::Back,
            ops::KEY_COMMAND => {
                let len = usize::from(cur.u16()?);
                let name =
                    core::str::from_utf8(cur.take(len)?).map_err(|_| DecodeError::BadUtf8)?;
                UiEvent::KeyCommand(name.to_string())
            }
            ops::INSETS => {
                let [top, bottom, left, right, keyboard] = read_f32s(&mut cur)?;
                UiEvent::Insets(Insets {
                    top,
                    bottom,
                    left,
                    right,
                    keyboard,
                })
            }
            ops::WINDOW => {
                let [width, height, density] = read_f32s(&mut cur)?;
                UiEvent::Window {
                    width,
                    height,
                    density,
                }
            }
            ops::FRAME_INTERVAL => {
                UiEvent::FrameInterval(f64::from_le_bytes(cur.take(8)?.try_into().unwrap()))
            }
            ops::FRAME => UiEvent::Frame,
            ops::MOTION => {
                let n = cur.u32()? as usize;
                if n > cur.remaining() / 8 {
                    return Err(DecodeError::LengthOverflow);
                }
                let mut pairs = Vec::with_capacity(n);
                for _ in 0..n {
                    pairs.push((cur.u32()?, cur.u32()?));
                }
                UiEvent::Motion(pairs)
            }
            ops::NAV_SIDES => {
                let [left, right] = read_f32s(&mut cur)?;
                UiEvent::NavSides { left, right }
            }
            other => return Err(DecodeError::BadTag(other)),
        };
        if cur.remaining() != 0 {
            return Err(DecodeError::BadFraming);
        }
        Ok(event)
    }

    /// The whole buffer, from the leading `u32` 0. A key command longer
    /// than `u16::MAX` bytes is cut at a char boundary.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        if let UiEvent::Node(e) = self {
            return e.to_bytes();
        }
        let mut out = Vec::with_capacity(32);
        out.extend_from_slice(&UI.to_le_bytes());
        out.extend_from_slice(&[VERSION, self.op()]);
        let mut put_f32s = |vs: &[f32]| {
            for v in vs {
                out.extend_from_slice(&v.to_le_bytes());
            }
        };
        match self {
            UiEvent::Node(_) | UiEvent::Back | UiEvent::Frame => {}
            UiEvent::Insets(i) => put_f32s(&[i.top, i.bottom, i.left, i.right, i.keyboard]),
            UiEvent::Window {
                width,
                height,
                density,
            } => put_f32s(&[*width, *height, *density]),
            UiEvent::NavSides { left, right } => put_f32s(&[*left, *right]),
            UiEvent::KeyCommand(name) => {
                let mut end = name.len().min(usize::from(u16::MAX));
                while !name.is_char_boundary(end) {
                    end -= 1;
                }
                out.extend_from_slice(&(end as u16).to_le_bytes());
                out.extend_from_slice(&name.as_bytes()[..end]);
            }
            UiEvent::FrameInterval(ms) => out.extend_from_slice(&ms.to_le_bytes()),
            UiEvent::Motion(pairs) => {
                out.extend_from_slice(&(pairs.len() as u32).to_le_bytes());
                for (a, b) in pairs {
                    out.extend_from_slice(&a.to_le_bytes());
                    out.extend_from_slice(&b.to_le_bytes());
                }
            }
        }
        out
    }
}

/// An event filter's return: deliver the event to the guest unchanged.
pub const FILTER_PASS: i32 = 0;
/// The filter emitted an event to deliver instead.
pub const FILTER_REPLACE: i32 = 1;
/// The event goes no further. If the filter emitted bytes, they are the
/// reply. Phoenix's dispatch returns no data for an event, so the reply is
/// rekindle's own model.
pub const FILTER_CONSUME: i32 = 2;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_is_the_bytes_the_guest_decodes() {
        let bytes = Event::press(7).to_bytes();
        assert_eq!(
            bytes,
            [0, 0, 0, 0, 0x01, 0x01, 0x07, 0, 0, 0, 0x66, 0x02, 0x00]
        );
        assert_eq!(Event::from_bytes(&bytes), Ok(Event::press(7)));
        assert!(is_ui(&bytes));
    }

    /// The Android platform's writers' bytes, which the guest's decoder
    /// accepted under emulation.
    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    #[test]
    fn ops_2_to_9_are_the_bytes_the_guest_decodes() {
        for (bytes, event) in [
            ("00 00 00 00 01 02", UiEvent::Back),
            (
                "00 00 00 00 01 03 04 00 73 65 6e 64",
                UiEvent::KeyCommand("send".into()),
            ),
            (
                "00 00 00 00 01 04 00 00 c0 41 00 00 40 42 00 00 00 00 00 00 00 00 00 00 96 43",
                UiEvent::Insets(Insets {
                    top: 24.0,
                    bottom: 48.0,
                    left: 0.0,
                    right: 0.0,
                    keyboard: 300.0,
                }),
            ),
            (
                "00 00 00 00 01 05 33 b3 cd 43 33 93 64 44 00 00 28 40",
                UiEvent::Window {
                    width: 411.4,
                    height: 914.3,
                    density: 2.625,
                },
            ),
            (
                "00 00 00 00 01 06 ab aa aa aa aa aa 20 40",
                UiEvent::FrameInterval(1000.0 / 120.0),
            ),
            ("00 00 00 00 01 07", UiEvent::Frame),
            (
                "00 00 00 00 01 08 02 00 00 00 01 00 10 00 05 00 00 00 03 00 00 00 00 00 80 3f",
                UiEvent::Motion(vec![(0x0010_0001, 5), (3, 0x3f80_0000)]),
            ),
            (
                "00 00 00 00 01 09 00 00 00 00 00 00 40 42",
                UiEvent::NavSides {
                    left: 0.0,
                    right: 48.0,
                },
            ),
        ] {
            let bytes = hex(bytes);
            assert_eq!(
                UiEvent::from_bytes(&bytes),
                Ok(event.clone()),
                "{bytes:02x?}"
            );
            assert_eq!(event.to_bytes(), bytes);
            let mut long = bytes.clone();
            long.push(0);
            assert_eq!(UiEvent::from_bytes(&long), Err(DecodeError::BadFraming));
        }
    }

    #[test]
    fn a_key_command_has_a_u16_length() {
        let u32_len = hex("00 00 00 00 01 03 04 00 00 00 73 65 6e 64");
        assert!(UiEvent::from_bytes(&u32_len).is_err());
        assert_eq!(
            UiEvent::from_bytes(&hex("00 00 00 00 02 07")),
            Err(DecodeError::BadFraming)
        );
    }

    #[test]
    fn the_guest_drops_a_bad_frame_interval_and_clamps_nav_sides() {
        for ms in [0.0, -8.0, f64::NAN, f64::INFINITY, f64::MIN_POSITIVE / 2.0] {
            assert_eq!(UiEvent::FrameInterval(ms).as_guest_takes(), None);
        }
        let ok = UiEvent::FrameInterval(16.6);
        assert_eq!(ok.as_guest_takes(), Some(ok));
        assert_eq!(
            UiEvent::NavSides {
                left: -3.0,
                right: 48.0
            }
            .as_guest_takes(),
            Some(UiEvent::NavSides {
                left: 0.0,
                right: 48.0
            })
        );
        assert_eq!(
            UiEvent::NavSides {
                left: f32::NAN,
                right: 0.0
            }
            .as_guest_takes(),
            None
        );
    }

    #[test]
    fn a_bad_event_is_an_error() {
        let press = Event::press(7).to_bytes();
        assert_eq!(
            Event::from_bytes(&press[..8]),
            Err(DecodeError::UnexpectedEof)
        );
        let mut trailing = press.clone();
        trailing.push(0);
        assert_eq!(Event::from_bytes(&trailing), Err(DecodeError::BadFraming));
        let mut version = press.clone();
        version[4] = 2;
        assert_eq!(Event::from_bytes(&version), Err(DecodeError::BadFraming));
        let mut op = press;
        op[5] = 11;
        assert_eq!(Event::from_bytes(&op), Err(DecodeError::BadTag(11)));
        assert!(!is_ui(&[1, 0, 0, 0]));
    }
}
