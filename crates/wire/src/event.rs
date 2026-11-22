//! Events from the host to the guest: the wire's other direction.
//!
//! # Recovered
//!
//! The shape of the crossing. The Android host's `ldh_dispatch(handle, ptr,
//! len, reply_ctx) -> bool` hands the guest one event as a byte buffer
//! `(ptr, len)`, through slot `+0x20` of the guest handle's vtable. The
//! guest returns a boxed reply, and a non-null one is delivered through
//! `reply_ctx`. `ldh_dispatch` returns `true` when the reply was null.
//! `ldh_reply(slot, ptr, len)` copies reply bytes into a slot as
//! `{len, ptr, len}`.
//!
//! # Modelled
//!
//! Everything inside the buffer. No guest module has been recovered, so how
//! an event names its target, its kind and its data is rekindle's stand-in:
//! a `u32` node id, a kind byte ([`EventKind`]), then one value through the
//! recovered value codec ([`Value`]). A reply is one value the same way.
//! Which events the host raises, and when, is modelled too: the mock raises
//! [`EventKind::Press`] when a click lands on a node.
//!
//! The verdicts an event filter returns ([`FILTER_PASS`] and the rest) are
//! the mock's seam, in the place a detour on `ldh_dispatch` would sit on
//! Phoenix.

use crate::value::{Cursor, DecodeError};
use crate::{NodeId, Value};

/// What happened to a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    /// A click or tap landed on the node: down and up inside it. Byte 0.
    Press,
}

impl EventKind {
    fn byte(self) -> u8 {
        match self {
            EventKind::Press => 0,
        }
    }

    fn from_byte(b: u8) -> Option<EventKind> {
        match b {
            0 => Some(EventKind::Press),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            EventKind::Press => "press",
        }
    }
}

/// Modelled. One event, as the host hands it to the guest. A recovered guest
/// module would replace this layout (see the module docs).
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// The node the event targets: the one the host's hit test found.
    pub node: NodeId,
    pub kind: EventKind,
    /// What the event carries beyond its target. `Null` for a press.
    pub payload: Value,
}

impl Event {
    pub fn press(node: NodeId) -> Self {
        Event {
            node,
            kind: EventKind::Press,
            payload: Value::Null,
        }
    }

    /// The buffer `ldh_dispatch` would carry: `u32` node, kind byte, value.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(6);
        out.extend_from_slice(&self.node.to_le_bytes());
        out.push(self.kind.byte());
        self.payload.encode(&mut out);
        out
    }

    /// Decode one event. Bytes after the payload are an error, as a
    /// truncated buffer is.
    pub fn from_bytes(buf: &[u8]) -> Result<Event, DecodeError> {
        let mut cur = Cursor::new(buf);
        let node = cur.u32()?;
        let kind = EventKind::from_byte(cur.u8()?).ok_or(DecodeError::BadFraming)?;
        let payload = cur.value()?;
        if cur.remaining() != 0 {
            return Err(DecodeError::BadFraming);
        }
        Ok(Event {
            node,
            kind,
            payload,
        })
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
    fn an_event_round_trips_and_its_payload_uses_the_value_codec() {
        let e = Event {
            node: 0xF000_0002,
            kind: EventKind::Press,
            payload: Value::I32(3),
        };
        let bytes = e.to_bytes();
        assert_eq!(bytes, [0x02, 0, 0, 0xF0, 0, 3, 3, 0, 0, 0]);
        assert_eq!(Event::from_bytes(&bytes), Ok(e));
        assert_eq!(Event::press(7).to_bytes(), [7, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_bad_event_is_an_error() {
        assert_eq!(
            Event::from_bytes(&[7, 0, 0]),
            Err(DecodeError::UnexpectedEof)
        );
        assert_eq!(
            Event::from_bytes(&[7, 0, 0, 0, 9, 0]),
            Err(DecodeError::BadFraming)
        );
        assert_eq!(
            Event::from_bytes(&[7, 0, 0, 0, 0, 0, 0]),
            Err(DecodeError::BadFraming)
        );
    }
}
