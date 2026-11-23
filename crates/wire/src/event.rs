//! Events from the platform to the guest: the wire's other direction.
//!
//! # Recovered
//!
//! The crossing. The Android host's `ldh_dispatch(handle, ptr, len,
//! reply_ctx) -> bool` hands the guest one buffer `(ptr, len)`, through slot
//! `+0x20` of the guest handle's vtable. The guest returns a boxed reply, and
//! a non-null one is delivered through `reply_ctx`. `ldh_dispatch` returns
//! `true` when the reply was null. `ldh_reply(slot, ptr, len)` copies reply
//! bytes into a slot as `{len, ptr, len}`.
//!
//! The buffer. The guest's `ld_event` (`0x17e77e8`) reads a leading `u32`.
//! If it is 0, the buffer is a UI event and `0x1730f6c` decodes the rest.
//! Anything else is a host frame ([`crate::frame`]). After the `u32` 0
//! comes the version byte `0x01`, an op byte and the op's body, which must
//! end at the end of the buffer:
//!
//! | op | body |
//! |---:|---|
//! | 1 | `i32` node, `u16` `PropId`, value |
//! | 2 | none |
//! | 3 | `u32` length and UTF-8 |
//! | 4 | 5 × `u32` |
//! | 5 | `u32`, `u32`, `f32` |
//! | 6 | `u64` |
//! | 7 | none |
//! | 8 | `u32` n, n × (`u32`, `u32`) |
//! | 9 | `u32`, `u32` |
//!
//! Op 1 is an event on a node ([`Event`]). Its `PropId` is the event's name
//! ([`names`]), one of those the node's Create listed in `on` (320), and the
//! value is its data. What ops 2 to 9 mean isn't in the decoder, so
//! [`UiEvent::Other`] keeps their bodies as bytes. A node id is written as
//! a 32-bit register and read as an `i32`. rekindle's own ids sit above
//! `i32::MAX`, and the guest never receives an event for them.
//!
//! # Modelled
//!
//! The reply's format. Here it is one value through the value codec. The
//! verdicts an event filter returns ([`FILTER_PASS`] and the rest) are the
//! mock's seam, in the place a detour on `ldh_dispatch` would sit on
//! Phoenix.

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
            UiEvent::Other { .. } => Err(DecodeError::BadFraming),
        }
    }
}

/// One UI event, as the guest's decoder reads it.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    /// Op 1.
    Node(Event),
    /// Ops 2 to 9, whose meaning isn't read: the op and its body.
    Other { op: u8, body: Vec<u8> },
}

/// Whether `buf` starts with the `u32` 0 of a UI event. Anything else is a
/// host frame.
#[must_use]
pub fn is_ui(buf: &[u8]) -> bool {
    buf.first_chunk::<4>()
        .is_some_and(|h| u32::from_le_bytes(*h) == UI)
}

impl UiEvent {
    /// Decode a UI event, from the leading `u32` 0. A body that doesn't end
    /// at the end of the buffer is an error, as on the guest.
    pub fn from_bytes(buf: &[u8]) -> Result<UiEvent, DecodeError> {
        let mut cur = Cursor::new(buf);
        if cur.u32()? != UI || cur.u8()? != VERSION {
            return Err(DecodeError::BadFraming);
        }
        let op = cur.u8()?;
        let start = buf.len() - cur.remaining();
        let event = match op {
            1 => {
                let node = cur.u32()?;
                let name = PropId(cur.u16()?);
                let value = cur.value()?;
                UiEvent::Node(Event { node, name, value })
            }
            2 | 7 => UiEvent::Other {
                op,
                body: Vec::new(),
            },
            3 => {
                let len = cur.u32()? as usize;
                core::str::from_utf8(cur.take(len)?).map_err(|_| DecodeError::BadUtf8)?;
                UiEvent::Other {
                    op,
                    body: buf[start..].to_vec(),
                }
            }
            4 | 5 | 6 | 9 => {
                let width = match op {
                    4 => 20,
                    5 => 12,
                    _ => 8,
                };
                UiEvent::Other {
                    op,
                    body: cur.take(width)?.to_vec(),
                }
            }
            8 => {
                let n = cur.u32()? as usize;
                if n > cur.remaining() / 8 {
                    return Err(DecodeError::LengthOverflow);
                }
                cur.take(n * 8)?;
                UiEvent::Other {
                    op,
                    body: buf[start..].to_vec(),
                }
            }
            other => return Err(DecodeError::BadTag(other)),
        };
        if cur.remaining() != 0 {
            return Err(DecodeError::BadFraming);
        }
        Ok(event)
    }

    /// The whole buffer, from the leading `u32` 0.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            UiEvent::Node(e) => e.to_bytes(),
            UiEvent::Other { op, body } => {
                let mut out = Vec::with_capacity(6 + body.len());
                out.extend_from_slice(&UI.to_le_bytes());
                out.extend_from_slice(&[VERSION, *op]);
                out.extend_from_slice(body);
                out
            }
        }
    }
}

/// An event filter's return: deliver the event to the guest unchanged.
pub const FILTER_PASS: i32 = 0;
/// The filter emitted an event to deliver instead.
pub const FILTER_REPLACE: i32 = 1;
/// The event goes no further. If the filter emitted bytes, they are the
/// reply, as the guest's would have been.
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

    #[test]
    fn other_ops_decode_by_width() {
        for (op, body) in [
            (2u8, vec![]),
            (3, vec![2, 0, 0, 0, b'h', b'i']),
            (4, vec![0; 20]),
            (5, vec![0; 12]),
            (6, vec![0; 8]),
            (7, vec![]),
            (8, vec![1, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0]),
            (9, vec![0; 8]),
        ] {
            let e = UiEvent::Other { op, body };
            let bytes = e.to_bytes();
            assert_eq!(UiEvent::from_bytes(&bytes), Ok(e));
            let mut long = bytes.clone();
            long.push(0);
            assert_eq!(UiEvent::from_bytes(&long), Err(DecodeError::BadFraming));
        }
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
