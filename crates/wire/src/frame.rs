//! Host frames: a guest's call into a host module, and the events and
//! batches a host module sends back.
//!
//! Read out of the guest code compiled into Phoenix's Android host
//! (`liblibdiscord_wasm_host.so`, build 356207). `0x17ccf68` writes the
//! header and `0x17ccf20` patches the record length, and about 280 generated
//! stubs build calls through them. `0x17ba1d0` reads a frame and accepts
//! kinds 1 to 4. The header is 16 bytes, little-endian:
//!
//! ```text
//! u8  kind          1 CALL (guest to host), 3 EVENT, 4 BATCH (host to guest)
//! u8  0
//! u16 module
//! u16 method        the event's id in an EVENT
//! u16 0
//! u32 call id       from a counter in TLS
//! u32 record length
//! ```
//!
//! The record is one value, and the body runs from the record's end to the
//! end of the frame. A BATCH's body is a sequence of `u32` length and frame
//! with no bytes left over (`0x17ba0d4`), and each frame in it goes back
//! through the same entry, so one batch can carry UI events and EVENT frames
//! together. A CALL's first byte is 1, so its leading `u32` is never 0, the
//! mark of a turn or a UI event. What kind 2 is for isn't read.

use crate::value::{Cursor, DecodeError};
use crate::{Value, decode};

/// A guest's call into a host module.
pub const CALL: u8 = 1;
/// An event from a host module.
pub const EVENT: u8 = 3;
/// Frames and UI events from the host, one after another.
pub const BATCH: u8 = 4;
/// Bytes before the record.
pub const HEADER_LEN: usize = 16;

/// One host frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// [`CALL`], 2, [`EVENT`] or [`BATCH`].
    pub kind: u8,
    pub module: u16,
    /// The method called, or an EVENT's id.
    pub method: u16,
    pub call: u32,
    /// The record's bytes: one value.
    pub record: Vec<u8>,
    /// The bytes after the record.
    pub body: Vec<u8>,
}

impl Frame {
    /// A CALL of `module`'s `method` with `record`, and no body.
    #[must_use]
    pub fn call(module: u16, method: u16, call: u32, record: &Value) -> Frame {
        Frame {
            kind: CALL,
            module,
            method,
            call,
            record: record.to_bytes(),
            body: Vec::new(),
        }
    }

    /// The record's value.
    pub fn record_value(&self) -> Result<Value, DecodeError> {
        decode(&self.record)
    }

    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.record.len() + self.body.len());
        out.extend_from_slice(&[self.kind, 0]);
        out.extend_from_slice(&self.module.to_le_bytes());
        out.extend_from_slice(&self.method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&self.call.to_le_bytes());
        out.extend_from_slice(&(self.record.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.record);
        out.extend_from_slice(&self.body);
        out
    }

    /// Read one frame. A kind outside 1 to 4 or a record past the end of
    /// the buffer is an error.
    pub fn from_bytes(buf: &[u8]) -> Result<Frame, DecodeError> {
        let mut cur = Cursor::new(buf);
        let kind = cur.u8()?;
        if !(1..=4).contains(&kind) {
            return Err(DecodeError::BadTag(kind));
        }
        cur.u8()?;
        let module = cur.u16()?;
        let method = cur.u16()?;
        cur.u16()?;
        let call = cur.u32()?;
        let len = cur.u32()? as usize;
        let record = cur.take(len)?.to_vec();
        let body = buf[buf.len() - cur.remaining()..].to_vec();
        Ok(Frame {
            kind,
            module,
            method,
            call,
            record,
            body,
        })
    }

    /// The buffers a BATCH's body holds. An entry that runs past the end is
    /// an error.
    pub fn batch(&self) -> Result<Vec<&[u8]>, DecodeError> {
        let mut out = Vec::new();
        let mut cur = Cursor::new(&self.body);
        while cur.remaining() > 0 {
            let len = cur.u32()? as usize;
            out.push(cur.take(len)?);
        }
        Ok(out)
    }
}

/// A BATCH body holding `entries`.
#[must_use]
pub fn batch_body<'a>(entries: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
    let mut out = Vec::new();
    for e in entries {
        out.extend_from_slice(&(e.len() as u32).to_le_bytes());
        out.extend_from_slice(e);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;

    #[test]
    fn a_call_has_a_16_byte_header_and_a_nonzero_leading_u32() {
        let f = Frame::call(0x12, 0x34, 7, &Value::Null);
        let bytes = f.to_bytes();
        assert_eq!(
            bytes,
            [1, 0, 0x12, 0, 0x34, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0, 0]
        );
        assert_ne!(u32::from_le_bytes(bytes[..4].try_into().unwrap()), 0);
        assert_eq!(Frame::from_bytes(&bytes), Ok(f));
    }

    #[test]
    fn a_batch_carries_ui_events_and_frames() {
        let press = Event::press(7).to_bytes();
        let event = Frame {
            kind: EVENT,
            module: 3,
            method: 9,
            call: 0,
            record: Value::Bool(true).to_bytes(),
            body: vec![0xaa],
        }
        .to_bytes();
        let batch = Frame {
            kind: BATCH,
            module: 0,
            method: 0,
            call: 0,
            record: Value::Null.to_bytes(),
            body: batch_body([&press[..], &event[..]]),
        };
        let back = Frame::from_bytes(&batch.to_bytes()).unwrap();
        assert_eq!(back.batch().unwrap(), vec![&press[..], &event[..]]);
        let inner = Frame::from_bytes(back.batch().unwrap()[1]).unwrap();
        assert_eq!(
            (inner.record_value(), inner.body),
            (Ok(Value::Bool(true)), vec![0xaa])
        );
        let mut cut = back.clone();
        cut.body.pop();
        assert!(cut.batch().is_err());
        assert_eq!(Frame::from_bytes(&[5]), Err(DecodeError::BadTag(5)));
    }
}
