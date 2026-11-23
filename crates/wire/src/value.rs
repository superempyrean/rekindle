//! The 13-tag value codec: RECOVERED byte-for-byte from Phoenix's host.
//!
//! The decode function at VA `0x172bc4c` in `liblibdiscord_wasm_host.so` reads
//! one `u8` tag, bounds-checks it `<= 12`, and branches through a dense 13-entry
//! jump table. Integers are hand-rolled **fixed-width little-endian** (no
//! LEB128/varint anywhere): lengths and counts are `u32`, `PropIds` and the
//! tag-12 indices are `u16`, the tag is `u8`. Malformed input is an error
//! return, not a panic (the host writes `-1`); [`decode`] mirrors that with a
//! `Result`.
//!
//! This module copies that shape exactly. Tag numbers, operand widths and the
//! recursion in tags 7/8/10 are all observed. The encoder at VA `0x1751a88`
//! writes the same tags from the guest's in-memory value, and its output was
//! emulated byte for byte. An integer goes out as tag 3 when it fits an `i32`
//! and as tag 4 otherwise ([`Value::int`]). Colours are integers,
//! `0xAARRGGBB` ([`Value::color`]), so an opaque colour is a tag 4.
//!
//! Tags 11 and 12 share a words table that lives with the decoder, one per
//! buffer. Each tag-11 value appends its `u32`s to it as a new list. A tag-12
//! value names an earlier list by index, copies it, overwrites the words at
//! its indices and appends the result as the next list. A list id past the
//! table or an index past the list's end fails the decode. [`Cursor`] keeps
//! the same table ([`Cursor::words`]).

use crate::PropId;

/// The host caps value nesting; a buffer past this is hostile, not real.
/// Matches the limit seen in an independent reading of the same build.
const MAX_DEPTH: usize = 128;

/// A decoded wire value. One variant per recovered tag (bool folds tags 1/2).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// tag 0 — null / none.
    Null,
    /// tags 1 (false) / 2 (true) — the value is folded into the tag.
    Bool(bool),
    /// tag 3 — `i32`, little-endian.
    I32(i32),
    /// tag 4 — `i64`, little-endian.
    I64(i64),
    /// tag 5 — `f64`, IEEE-754 little-endian.
    F64(f64),
    /// tag 6 — `u32` length + UTF-8 bytes.
    Str(String),
    /// tag 7 — `u32` length + that many values.
    Array(Vec<Value>),
    /// tag 8 — the element property set: `u32` count + count × (`u16` `PropId`,
    /// Value). This is the `view_props` write.
    Object(Vec<(PropId, Value)>),
    /// tag 9 — four `f32`s: the edge insets of `padding` (332), `margin`
    /// (287) and `hitSlopEdges` (809), the only `PropIds` `gpui-native` writes
    /// it for. The order of the edges isn't read from the binary.
    Quad([f32; 4]),
    /// tag 10 — `u32` count + count × (string key, Value). Insertion order is
    /// preserved so encoding is byte-exact.
    Map(Vec<(String, Value)>),
    /// tag 11 — `u32` length + that many `u32`s. Decoding it appends the
    /// words to the buffer's words table as a new list.
    U32Array(Vec<u32>),
    /// tag 12 — a patch of an earlier words list: `u32` list id + `u16`
    /// npairs + npairs × (`u16` index, `u32` word). Decoding it copies list
    /// `list`, writes each word at its index and appends the result as a new
    /// list. The variant keeps the wire form so encoding is byte-exact.
    Sparse { list: u32, pairs: Vec<(u16, u32)> },
}

/// The tag byte a value encodes to. `Bool(false)` is 1, `Bool(true)` is 2.
#[must_use]
pub(crate) fn tag_of(value: &Value) -> u8 {
    match value {
        Value::Null => 0,
        Value::Bool(false) => 1,
        Value::Bool(true) => 2,
        Value::I32(_) => 3,
        Value::I64(_) => 4,
        Value::F64(_) => 5,
        Value::Str(_) => 6,
        Value::Array(_) => 7,
        Value::Object(_) => 8,
        Value::Quad(_) => 9,
        Value::Map(_) => 10,
        Value::U32Array(_) => 11,
        Value::Sparse { .. } => 12,
    }
}

/// What went wrong decoding a hostile or truncated buffer. The host returns
/// `-1` for any of these; the kinds here just make tests legible.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Fewer bytes left than the next field needs.
    UnexpectedEof,
    /// A tag byte outside `0..=12`.
    BadTag(u8),
    /// A tag-6/10 string that was not valid UTF-8.
    BadUtf8,
    /// A tag-12 value named a words list this buffer hasn't defined, or an
    /// index past the end of that list.
    BadWordsRef,
    /// A length or count that cannot fit the remaining bytes.
    LengthOverflow,
    /// A turn without its version byte, an op code outside 1 to 10, a
    /// `ChildEdits` op that runs past its parent's list, or an event or host
    /// frame whose body doesn't end where the buffer does.
    BadFraming,
    /// Value nesting exceeded the decoder's depth limit, so the buffer is
    /// treated as hostile rather than recursed into.
    TooDeep,
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DecodeError::UnexpectedEof => f.write_str("unexpected end of buffer"),
            DecodeError::BadTag(t) => write!(f, "value tag {t} out of range 0..=12"),
            DecodeError::BadUtf8 => f.write_str("string was not valid UTF-8"),
            DecodeError::BadWordsRef => f.write_str("tag 12 named a missing words list or index"),
            DecodeError::LengthOverflow => f.write_str("length exceeds remaining bytes"),
            DecodeError::BadFraming => f.write_str("malformed turn, event or frame"),
            DecodeError::TooDeep => f.write_str("value nesting exceeded the depth limit"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl Value {
    /// An integer as the guest's encoder writes it: tag 3 when it fits an
    /// `i32`, tag 4 otherwise.
    #[must_use]
    pub fn int(v: i64) -> Value {
        i32::try_from(v).map_or(Value::I64(v), Value::I32)
    }

    /// A colour as `0xAARRGGBB`, through [`Value::int`]. A colour with
    /// alpha below `0x80` is a tag 3, any other a tag 4.
    #[must_use]
    pub fn color(argb: u32) -> Value {
        Value::int(i64::from(argb))
    }

    /// The integer in a tag-3 or tag-4 value.
    #[must_use]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::I32(v) => Some(i64::from(*v)),
            Value::I64(v) => Some(*v),
            _ => None,
        }
    }

    /// The `0xAARRGGBB` colour in an integer value. `None` for any other
    /// value and for an integer outside `u32`.
    #[must_use]
    pub fn as_color(&self) -> Option<u32> {
        u32::try_from(self.as_int()?).ok()
    }

    /// Append this value's wire bytes to `out`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(tag_of(self));
        match self {
            Value::Null | Value::Bool(_) => {}
            Value::I32(v) => out.extend_from_slice(&v.to_le_bytes()),
            Value::I64(v) => out.extend_from_slice(&v.to_le_bytes()),
            Value::F64(v) => out.extend_from_slice(&v.to_le_bytes()),
            Value::Str(s) => encode_str(s, out),
            Value::Array(items) => {
                out.extend_from_slice(&(items.len() as u32).to_le_bytes());
                for item in items {
                    item.encode(out);
                }
            }
            Value::Object(props) => {
                out.extend_from_slice(&(props.len() as u32).to_le_bytes());
                for (id, value) in props {
                    out.extend_from_slice(&id.0.to_le_bytes());
                    value.encode(out);
                }
            }
            Value::Quad(quad) => {
                for f in quad {
                    out.extend_from_slice(&f.to_le_bytes());
                }
            }
            Value::Map(entries) => {
                out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
                for (key, value) in entries {
                    encode_str(key, out);
                    value.encode(out);
                }
            }
            Value::U32Array(items) => {
                out.extend_from_slice(&(items.len() as u32).to_le_bytes());
                for v in items {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
            Value::Sparse { list, pairs } => {
                out.extend_from_slice(&list.to_le_bytes());
                out.extend_from_slice(&(pairs.len() as u16).to_le_bytes());
                for (index, value) in pairs {
                    out.extend_from_slice(&index.to_le_bytes());
                    out.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
    }

    /// Encode to a fresh buffer.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }
}

fn encode_str(s: &str, out: &mut Vec<u8>) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

/// A moving head over a byte buffer. Mirrors the host's `Cursor { ptr, len }`:
/// every read checks the remaining length first and advances, so truncated
/// input is an error, never a panic or an out-of-bounds read.
pub struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
    /// The words lists this buffer has defined so far, one per tag-11 or
    /// tag-12 value, in order.
    words: Vec<Vec<u32>>,
    /// Current value-nesting depth, so a hostile deeply-nested buffer is
    /// rejected before it can overflow the stack.
    depth: usize,
}

impl<'a> Cursor<'a> {
    #[must_use]
    pub fn new(buf: &'a [u8]) -> Self {
        Cursor {
            buf,
            pos: 0,
            words: Vec::new(),
            depth: 0,
        }
    }

    /// The words lists decoded so far, indexed the way a tag-12 `list` names
    /// them.
    #[must_use]
    pub fn words(&self) -> &[Vec<u32>] {
        &self.words
    }

    /// Bytes not yet consumed.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::LengthOverflow)?;
        let slice = self
            .buf
            .get(self.pos..end)
            .ok_or(DecodeError::UnexpectedEof)?;
        self.pos = end;
        Ok(slice)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    /// Read a `u32` length and reject it up front if it cannot fit the rest of
    /// the buffer, with `per` the minimum bytes per element (the host does the
    /// same `len <= remaining / width` check before allocating).
    fn length(&mut self, per: usize) -> Result<usize, DecodeError> {
        let len = self.u32()? as usize;
        if per > 0 && len > self.remaining() / per {
            return Err(DecodeError::LengthOverflow);
        }
        Ok(len)
    }

    /// Decode one value at the head, advancing past it. Nesting is capped at
    /// [`MAX_DEPTH`]; a buffer deeper than that is rejected as hostile.
    pub fn value(&mut self) -> Result<Value, DecodeError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err(DecodeError::TooDeep);
        }
        let result = self.value_inner();
        self.depth -= 1;
        result
    }

    fn value_inner(&mut self) -> Result<Value, DecodeError> {
        let tag = self.u8()?;
        Ok(match tag {
            0 => Value::Null,
            1 => Value::Bool(false),
            2 => Value::Bool(true),
            3 => Value::I32(i32::from_le_bytes(self.take(4)?.try_into().unwrap())),
            4 => Value::I64(i64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            5 => Value::F64(f64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            6 => Value::Str(self.string()?),
            7 => {
                // A value is at least one tag byte, so cap the count at the
                // bytes remaining.
                let len = self.length(1)?;
                let mut items = Vec::with_capacity(len);
                for _ in 0..len {
                    items.push(self.value()?);
                }
                Value::Array(items)
            }
            8 => {
                // Each entry is at least a u16 id + a one-byte value.
                let count = self.length(3)?;
                let mut props = Vec::with_capacity(count);
                for _ in 0..count {
                    let id = PropId(self.u16()?);
                    props.push((id, self.value()?));
                }
                Value::Object(props)
            }
            9 => {
                let bytes = self.take(16)?;
                let mut quad = [0.0f32; 4];
                for (i, slot) in quad.iter_mut().enumerate() {
                    *slot = f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
                }
                Value::Quad(quad)
            }
            10 => {
                // Each entry is at least a u32-len (>= 4) key + a one-byte value.
                let count = self.length(5)?;
                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    let key = self.string()?;
                    entries.push((key, self.value()?));
                }
                Value::Map(entries)
            }
            11 => {
                let len = self.length(4)?;
                let mut items = Vec::with_capacity(len);
                for _ in 0..len {
                    items.push(self.u32()?);
                }
                self.words.push(items.clone());
                Value::U32Array(items)
            }
            12 => {
                let list = self.u32()?;
                let npairs = self.u16()? as usize;
                if npairs > self.remaining() / 6 {
                    return Err(DecodeError::LengthOverflow);
                }
                let mut words = self
                    .words
                    .get(list as usize)
                    .cloned()
                    .ok_or(DecodeError::BadWordsRef)?;
                let mut pairs = Vec::with_capacity(npairs);
                for _ in 0..npairs {
                    let index = self.u16()?;
                    let value = self.u32()?;
                    *words
                        .get_mut(usize::from(index))
                        .ok_or(DecodeError::BadWordsRef)? = value;
                    pairs.push((index, value));
                }
                self.words.push(words);
                Value::Sparse { list, pairs }
            }
            other => return Err(DecodeError::BadTag(other)),
        })
    }

    pub(crate) fn string(&mut self) -> Result<String, DecodeError> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        core::str::from_utf8(bytes)
            .map(ToString::to_string)
            .map_err(|_| DecodeError::BadUtf8)
    }
}

/// Decode a single value from the start of `buf`. Trailing bytes are ignored
/// (the caller that wants strictness checks [`Cursor::remaining`]).
pub fn decode(buf: &[u8]) -> Result<Value, DecodeError> {
    Cursor::new(buf).value()
}
