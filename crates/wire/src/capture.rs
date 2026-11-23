//! A capture file: the turns a client sent, as the exact bytes of each.
//!
//! The format is rekindle's, not Phoenix's. `host-bridge/record.rs` in the
//! host suggests Phoenix records frames too, but its format is not recovered.
//!
//! ```text
//! header   "RKWIRECP"  8 bytes, magic
//!          u32 LE      version (2)
//! record   u64 LE      turn index, from 0
//!          u64 LE      microseconds since the capture started
//!          u32 LE      length of the turn's bytes
//!          ...         the bytes `Turn::to_bytes` produced
//! ```
//!
//! Records follow each other to the end of the file. A writer appends each
//! record with one write, so a reader tailing the file sees whole records,
//! or a partial last one it should wait on.

use std::io::{self, Write};
use std::time::Instant;

/// The first eight bytes of a capture file.
pub const MAGIC: [u8; 8] = *b"RKWIRECP";
/// The format version [`Writer`] writes and [`parse`] reads. Version 1
/// held turns in the framing rekindle modelled before the real one was
/// read, so this reader refuses it.
pub const VERSION: u32 = 2;
/// Bytes in the file header.
pub const HEADER_LEN: usize = 12;
/// Bytes in a record's header, before the turn's bytes.
pub const RECORD_HEADER_LEN: usize = 20;

/// One captured turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub turn: u64,
    pub micros: u64,
    pub bytes: Vec<u8>,
}

/// Why a capture could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureError {
    /// Fewer than [`HEADER_LEN`] bytes.
    ShortHeader,
    /// The file does not start with [`MAGIC`].
    BadMagic,
    /// A version this reader doesn't know.
    Version(u32),
}

impl core::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CaptureError::ShortHeader => f.write_str("file is shorter than a capture header"),
            CaptureError::BadMagic => f.write_str("not a wire capture (no RKWIRECP magic)"),
            CaptureError::Version(v) => {
                write!(f, "capture version {v}, this reader knows {VERSION}")
            }
        }
    }
}

impl std::error::Error for CaptureError {}

/// The file header.
#[must_use]
pub fn header() -> [u8; HEADER_LEN] {
    let mut h = [0; HEADER_LEN];
    h[..8].copy_from_slice(&MAGIC);
    h[8..].copy_from_slice(&VERSION.to_le_bytes());
    h
}

/// Append one record to `out`.
pub fn encode_record(turn: u64, micros: u64, bytes: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&turn.to_le_bytes());
    out.extend_from_slice(&micros.to_le_bytes());
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

/// Check the header of `buf`.
pub fn check_header(buf: &[u8]) -> Result<(), CaptureError> {
    if buf.len() < HEADER_LEN {
        return Err(CaptureError::ShortHeader);
    }
    if buf[..8] != MAGIC {
        return Err(CaptureError::BadMagic);
    }
    let version = u32::from_le_bytes(buf[8..12].try_into().unwrap());
    if version != VERSION {
        return Err(CaptureError::Version(version));
    }
    Ok(())
}

/// The whole records in `buf`, which starts after the header, and how many
/// bytes of it they took. A partial record at the end is left for the next
/// read.
#[must_use]
pub fn parse_records(buf: &[u8]) -> (Vec<Record>, usize) {
    let mut records = Vec::new();
    let mut pos = 0;
    while let Some(head) = buf.get(pos..pos + RECORD_HEADER_LEN) {
        let turn = u64::from_le_bytes(head[..8].try_into().unwrap());
        let micros = u64::from_le_bytes(head[8..16].try_into().unwrap());
        let len = u32::from_le_bytes(head[16..20].try_into().unwrap()) as usize;
        let start = pos + RECORD_HEADER_LEN;
        let Some(bytes) = buf.get(start..start + len) else {
            break;
        };
        records.push(Record {
            turn,
            micros,
            bytes: bytes.to_vec(),
        });
        pos = start + len;
    }
    (records, pos)
}

/// A whole capture file: its records, and the bytes of a partial last record
/// (non-zero while a client is still writing it, or if it was cut off).
pub fn parse(buf: &[u8]) -> Result<(Vec<Record>, usize), CaptureError> {
    check_header(buf)?;
    let body = &buf[HEADER_LEN..];
    let (records, used) = parse_records(body);
    Ok((records, body.len() - used))
}

/// Appends turns to a capture as they are sent.
pub struct Writer<W: Write> {
    out: W,
    start: Instant,
    buf: Vec<u8>,
}

impl<W: Write> Writer<W> {
    /// Write the header to `out`.
    pub fn new(mut out: W) -> io::Result<Self> {
        out.write_all(&header())?;
        out.flush()?;
        Ok(Writer {
            out,
            start: Instant::now(),
            buf: Vec::new(),
        })
    }

    /// Append one turn's bytes, in one write.
    pub fn record(&mut self, turn: u64, bytes: &[u8]) -> io::Result<()> {
        self.buf.clear();
        let micros = u64::try_from(self.start.elapsed().as_micros()).unwrap_or(u64::MAX);
        encode_record(turn, micros, bytes, &mut self.buf);
        self.out.write_all(&self.buf)?;
        self.out.flush()
    }

    pub fn into_inner(self) -> W {
        self.out
    }
}

impl Writer<std::fs::File> {
    /// Create (or truncate) the file at `path` and write the header.
    pub fn create(path: impl AsRef<std::path::Path>) -> io::Result<Self> {
        Writer::new(std::fs::File::create(path)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_and_a_partial_tail_waits() {
        let mut w = Writer::new(Vec::new()).unwrap();
        w.record(0, &[1, 0, 0, 0, 3, 9, 0, 0, 0]).unwrap();
        w.record(1, &[0, 0, 0, 0]).unwrap();
        let mut file = w.into_inner();
        let (records, partial) = parse(&file).unwrap();
        assert_eq!(partial, 0);
        assert_eq!(records.len(), 2);
        assert_eq!((records[0].turn, records[0].bytes.len()), (0, 9));
        assert_eq!(records[1].bytes, vec![0, 0, 0, 0]);

        // Half a record: the whole ones still parse.
        let mut more = Vec::new();
        encode_record(2, 5, &[7; 10], &mut more);
        file.extend_from_slice(&more[..15]);
        let (records, partial) = parse(&file).unwrap();
        assert_eq!((records.len(), partial), (2, 15));
    }

    #[test]
    fn bad_headers_error() {
        assert_eq!(parse(b"RKW"), Err(CaptureError::ShortHeader));
        assert_eq!(parse(b"NOTACAPTURE!"), Err(CaptureError::BadMagic));
        let mut h = header().to_vec();
        h[8] = 9;
        assert_eq!(parse(&h), Err(CaptureError::Version(9)));
    }
}
