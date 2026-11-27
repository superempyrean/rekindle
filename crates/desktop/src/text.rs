//! The client's own text element: GPUI's `impl Element for SharedString`,
//! and the function that turns one into an `AnyElement`.
//!
//! Every `AnyElement::new::<E>` the client kept out of line moves the
//! element into the frame's element arena through the same closure, and
//! panics with `arena callback produces a result` when the arena returns
//! nothing. Each one then writes the `AnyElement`:
//!
//! ```text
//! [sret + 0]    = valid flag (Rc<Cell<bool>>)
//! [sret + 8]    = Drawable<E>
//! [sret + 0x10] = ElementObject vtable for Drawable<E>
//! [sret + 0x18] = 0
//! ```
//!
//! The `SharedString` one takes the 24-byte string by pointer, and drops
//! what the closure left behind with `SharedString`'s own drop check: an
//! `Option<SharedString>` whose `None` is tag `0xFF`, and an `Arc<str>` for
//! tags from `0x19`. Its vtable's `inner_element` entry returns the element
//! 24 bytes before the end of the `Drawable`.

use anyhow::{Context, Result, bail};
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};

use crate::image::Image;

const ARENA_PANIC: &[u8] = b"arena callback produces a result";

/// The size of a `SharedString`.
pub const SHARED_STRING: usize = 24;

/// The recovered text element.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Text {
    /// `AnyElement::new::<SharedString>`: `(sret, &SharedString) -> sret`.
    /// The string is moved in.
    pub into_any: u64,
    /// The `ElementObject` vtable for `Drawable<SharedString>`.
    pub vtable: u64,
    /// `Drawable<SharedString>`'s size.
    pub drawable_size: usize,
    /// The `SharedString`'s offset in the `Drawable`.
    pub element_at: usize,
}

fn decode(image: &Image<'_>, start: u64, end: u64) -> Result<Vec<Instruction>> {
    let len = usize::try_from(end - start)?;
    let code = image.read(start, len).context("function is unmapped")?;
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut out = Vec::new();
    while decoder.can_decode() {
        let mut insn = Instruction::default();
        decoder.decode_out(&mut insn);
        out.push(insn);
    }
    Ok(out)
}

/// The out-of-line `AnyElement::new` functions: the ones that load the
/// arena panic message.
fn arena_conversions(image: &Image<'_>) -> Vec<u64> {
    let mut out = Vec::new();
    for string in image.find(ARENA_PANIC) {
        for lea in image.leas_to(string, string + 1) {
            if let Some((start, _)) = image.function(lea) {
                out.push(start);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The vtable a conversion stores at `[sret + 0x10]`: `lea rax, [rip + V]`
/// followed by `mov [reg + 0x10], rax`.
fn stored_vtable(instructions: &[Instruction]) -> Option<u64> {
    instructions.windows(2).find_map(|pair| {
        let (lea, mov) = (&pair[0], &pair[1]);
        (lea.mnemonic() == Mnemonic::Lea
            && lea.is_ip_rel_memory_operand()
            && mov.mnemonic() == Mnemonic::Mov
            && mov.op0_kind() == OpKind::Memory
            && mov.memory_displacement64() == 0x10
            && mov.op1_kind() == OpKind::Register
            && mov.op1_register() == lea.op0_register())
        .then(|| lea.ip_rel_memory_address())
    })
}

/// Whether the function compares `cl` with `imm`.
fn compares_cl(instructions: &[Instruction], imm: u8) -> bool {
    instructions.iter().any(|insn| {
        insn.mnemonic() == Mnemonic::Cmp
            && insn.op0_kind() == OpKind::Register
            && insn.op0_register() == Register::CL
            && insn.op1_kind() == OpKind::Immediate8
            && insn.immediate8() == imm
    })
}

/// The offset `inner_element` returns: its first instruction is
/// `lea rax, [rcx + X]`.
fn inner_element_offset(image: &Image<'_>, entry: u64) -> Option<usize> {
    let code = image.read(entry, 16)?;
    let mut decoder = Decoder::with_ip(64, code, entry, DecoderOptions::NONE);
    let insn = decoder.decode();
    (insn.mnemonic() == Mnemonic::Lea
        && insn.op0_register() == Register::RAX
        && insn.memory_base() == Register::RCX
        && insn.memory_index() == Register::None)
        .then(|| insn.memory_displacement64() as usize)
}

/// Resolve `AnyElement::new::<SharedString>` and its vtable.
pub fn resolve(image: &Image<'_>) -> Result<Text> {
    let conversions = arena_conversions(image);
    if conversions.is_empty() {
        bail!("no AnyElement::new loads the arena panic message");
    }
    let mut found = Vec::new();
    for start in conversions {
        let Some((_, end)) = image.function(start) else {
            continue;
        };
        let instructions = decode(image, start, end)?;
        // A SharedString's drop: `None` is 0xFF, an Arc<str> from 0x19.
        if !compares_cl(&instructions, 0xFF) || !compares_cl(&instructions, 0x19) {
            continue;
        }
        let Some(vtable) = stored_vtable(&instructions) else {
            continue;
        };
        let Some(size) = image
            .qword(vtable + 8)
            .and_then(|s| usize::try_from(s).ok())
        else {
            continue;
        };
        let Some(inner) = image.qword(vtable + 0x18) else {
            continue;
        };
        let Some(element_at) = inner_element_offset(image, inner) else {
            continue;
        };
        if element_at + SHARED_STRING == size {
            found.push(Text {
                into_any: start,
                vtable,
                drawable_size: size,
                element_at,
            });
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => bail!("no AnyElement::new for a 24-byte SharedString element"),
        n => bail!(
            "{n} SharedString AnyElement::new candidates: {:x?}",
            found.iter().map(|t| t.into_any).collect::<Vec<_>>()
        ),
    }
}

/// An inline `SharedString` holding `text`: its length in the first byte,
/// the bytes after it. Inline strings hold up to 23 bytes and own nothing.
pub fn inline(text: &str) -> Result<[u8; SHARED_STRING]> {
    let bytes = text.as_bytes();
    if bytes.len() >= SHARED_STRING {
        bail!(
            "{} bytes is too long for an inline SharedString",
            bytes.len()
        );
    }
    let mut out = [0u8; SHARED_STRING];
    out[0] = bytes.len() as u8;
    out[1..=bytes.len()].copy_from_slice(bytes);
    Ok(out)
}

/// A `SharedString` borrowing `text`, of any length: tag 24, the pointer at
/// `+8` and the length at `+16`. It owns nothing, so the client never frees
/// `text`.
pub fn borrowed(text: &'static str) -> [u8; SHARED_STRING] {
    let mut out = [0u8; SHARED_STRING];
    out[0] = 24;
    out[8..16].copy_from_slice(&(text.as_ptr() as u64).to_le_bytes());
    out[16..24].copy_from_slice(&(text.len() as u64).to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_string() {
        let s = inline("rekindle").unwrap();
        assert_eq!(s[0], 8);
        assert_eq!(&s[1..9], b"rekindle");
        assert!(s[9..].iter().all(|&b| b == 0));
        assert!(inline(&"x".repeat(24)).is_err());
        assert_eq!(inline(&"x".repeat(23)).unwrap()[0], 23);
    }

    #[test]
    fn borrowed_string() {
        let text: &'static str = "a toast longer than an inline string";
        let s = borrowed(text);
        assert_eq!(s[0], 24);
        assert_eq!(
            u64::from_le_bytes(s[8..16].try_into().unwrap()),
            text.as_ptr() as u64
        );
        assert_eq!(
            u64::from_le_bytes(s[16..24].try_into().unwrap()),
            text.len() as u64
        );
    }
}
