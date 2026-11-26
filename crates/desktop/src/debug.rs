//! Recover `StyleRefinement`'s field offsets from the build's own
//! `Debug::fmt`.
//!
//! The client prints a style with a derived `Debug` impl. Each field is
//! emitted as a `lea` to its name, a `mov` of the name length, a `lea` to the
//! field inside `self`, and a call to `DebugStruct::field`. That shape is
//! stable across builds, so the offsets are read out of the code instead of
//! being carried as constants:
//!
//! ```text
//! lea r9,  [rsi + 0x150]       ; &self.background
//! lea rax, [rip + vtable]      ; &dyn Debug for Option<Fill>
//! mov [rsp + 0x20], rax
//! lea rdx, [rip + "background"]
//! mov r8d, 10                  ; the name's length
//! call DebugStruct::field
//! ```
//!
//! Nested structs (`text`, `padding`, `gap`, ...) have their own derived
//! `Debug::fmt`, reached through the field's `&dyn Debug` vtable at `+0x18`.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use iced_x86::{Code, Decoder, DecoderOptions, Instruction, OpKind, Register};

use crate::image::Image;

/// A style field's byte offset, keyed by the name its `Debug::fmt` prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleOffsets {
    pub fields: BTreeMap<String, usize>,
    /// The size the fields imply, aligned to 8.
    pub size: usize,
}

impl StyleOffsets {
    pub fn get(&self, name: &str) -> Result<usize> {
        self.fields
            .get(name)
            .copied()
            .with_context(|| format!("the build's StyleRefinement Debug has no {name:?}"))
    }

    pub fn has(&self, name: &str) -> bool {
        self.fields.contains_key(name)
    }
}

/// The fields the native writer needs. A build that loses one is a build the
/// backend refuses rather than guessing.
pub const REQUIRED: &[&str] = &[
    "background",
    "border_color",
    "border_widths",
    "corner_radii",
    "padding",
    "gap",
    "size",
    "min_size",
    "inset",
    "opacity",
    "flex_shrink",
    "overflow",
    "position",
    "display",
    "align_items",
    "justify_content",
];

/// Fields whose value is itself a struct with a derived `Debug`.
const NESTED: &[&str] = &[
    "text",
    "padding",
    "border_widths",
    "corner_radii",
    "inset",
    "gap",
    "size",
    "min_size",
    "max_size",
    "margin",
];

/// The widest value the runtime may write into a field, by name. Used only to
/// bound the struct size; an unknown field is assumed to be one word.
fn width(name: &str) -> usize {
    match name {
        "background" => 72,                  // Option<Fill>
        "border_color" | "text.color" => 20, // Option<Hsla>
        "text.font_family" => 24,            // SharedString
        "border_widths" | "corner_radii" | "padding" | "inset" | "margin" => 32,
        "gap" | "size" | "min_size" | "max_size" => 16,
        // One-byte enums and bools, and the two-byte Point<Overflow>.
        "display"
        | "visibility"
        | "position"
        | "flex_wrap"
        | "flex_direction"
        | "align_items"
        | "align_self"
        | "align_content"
        | "justify_content"
        | "mouse_cursor"
        | "border_style"
        | "allow_concurrent_scroll"
        | "restrict_scroll_to_axis" => 1,
        "overflow" => 2,
        _ => 8,
    }
}

/// Decode the offsets from the image's own `StyleRefinement` Debug impl.
pub fn decode(image: &Image<'_>) -> Result<StyleOffsets> {
    let names: Vec<u64> = image
        .find(b"StyleRefinement")
        .into_iter()
        .filter(|&va| standalone(image, va))
        .collect();
    if names.is_empty() {
        bail!("no standalone \"StyleRefinement\" string in the image");
    }
    let mut found = Vec::new();
    for name in names {
        for lea in image.leas_to(name, name + 1) {
            let Some((start, end)) = image.function(lea) else {
                continue;
            };
            let fields = scan(image, start, end, 1);
            if std::env::var_os("REKINDLE_DEBUG_RESOLVE").is_some() {
                eprintln!(
                    "debug: StyleRefinement xref {lea:#x} in {start:#x}..{end:#x}: {} field(s) {:?}",
                    fields.len(),
                    fields.keys().collect::<Vec<_>>()
                );
            }
            if fields.contains_key("background")
                && fields.contains_key("padding")
                && fields.contains_key("gap")
            {
                found.push(fields);
            }
        }
    }
    let fields = match found.as_slice() {
        [one] => one.clone(),
        [] => bail!("no function with a StyleRefinement-shaped Debug::fmt"),
        many => bail!(
            "{} functions look like StyleRefinement::fmt; refusing to guess",
            many.len()
        ),
    };
    let mut all = fields;
    let mut nested: Vec<(String, usize)> = Vec::new();
    for key in NESTED {
        // The scan keeps the vtable separately under a hidden key.
        if let Some(vtable) = all.remove(&format!("@{key}")) {
            nested.push(((*key).to_string(), vtable));
        }
    }
    for (key, vtable) in nested {
        let Some(fmt) = image.qword(vtable as u64 + 0x18) else {
            continue;
        };
        let Some((start, end)) = image.function(fmt) else {
            continue;
        };
        let base = all.get(&key).copied().unwrap_or(0);
        let inner = scan(image, start, end, 0);
        for (name, offset) in inner {
            if name.starts_with('@') {
                continue;
            }
            all.entry(format!("{key}.{name}")).or_insert(base + offset);
        }
    }
    all.retain(|k, _| !k.starts_with('@'));
    for required in REQUIRED {
        if !all.contains_key(*required) {
            bail!("StyleRefinement::fmt is missing {required:?}");
        }
    }
    let max = all
        .iter()
        .map(|(name, &offset)| offset.saturating_add(width(name)))
        .max()
        .unwrap_or(0);
    Ok(StyleOffsets {
        fields: all,
        size: max.next_multiple_of(8),
    })
}

fn standalone(image: &Image<'_>, va: u64) -> bool {
    let prev = image.read(va.wrapping_sub(1), 1).map_or(0, |b| b[0]);
    let after = image.read(va + 15, 1).map_or(0, |b| b[0]);
    !prev.is_ascii_alphanumeric() && prev != b'_' && after == 0
}

/// Scan one `Debug::fmt` for `name -> offset` and `@name -> vtable`.
fn scan(image: &Image<'_>, start: u64, end: u64, depth: u8) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    let len = usize::try_from(end.saturating_sub(start)).unwrap_or(0);
    let Some(code) = image.read(start, len) else {
        return out;
    };
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    let mut name: Option<(u64, usize)> = None;
    let mut value: Option<usize> = None;
    let mut vtable: Option<u64> = None;
    while decoder.can_decode() {
        decoder.decode_out(&mut insn);
        match insn.mnemonic() {
            iced_x86::Mnemonic::Lea => {
                if insn.op0_kind() == OpKind::Register
                    && insn.op1_kind() == OpKind::Memory
                    && insn.memory_base() == Register::RIP
                {
                    let target = insn.ip_rel_memory_address();
                    if insn.op0_register() == Register::RDX
                        && (insn.code() == Code::Lea_r64_m || insn.code() == Code::Lea_r32_m)
                    {
                        name = Some((target, 0));
                    } else if insn.op0_register() == Register::RAX {
                        vtable = Some(target);
                    }
                } else if insn.op0_kind() == OpKind::Register
                    && insn.op1_kind() == OpKind::Memory
                    && insn.op0_register() == Register::R9
                {
                    let base = insn.memory_base();
                    if !matches!(
                        base,
                        Register::RSP | Register::RBP | Register::RIP | Register::None
                    ) {
                        value = Some(insn.memory_displacement64() as usize);
                    }
                }
            }
            iced_x86::Mnemonic::Mov => {
                if insn.op0_kind() == OpKind::Register
                    && insn.op0_register() == Register::R8D
                    && insn.op1_kind() == OpKind::Immediate32
                    && let Some((_, len)) = &mut name
                {
                    *len = insn.immediate32() as usize;
                }
            }
            iced_x86::Mnemonic::Call => {
                if let (Some((name_va, name_len)), Some(offset)) = (name, value)
                    && (1..=64).contains(&name_len)
                    && let Some(text) = image.read(name_va, name_len)
                    && let Ok(text) = std::str::from_utf8(text)
                    && text
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b == b'_' || b.is_ascii_digit())
                {
                    out.entry(text.to_string()).or_insert(offset);
                    if depth > 0
                        && let Some(vtable) = vtable
                    {
                        out.entry(format!("@{text}")).or_insert(vtable as usize);
                    }
                }
                name = None;
                value = None;
                vtable = None;
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-encode the field sequence the scanner must recognise:
    /// `lea r9,[rsi+0x150]; lea rax,[rip+0x10]; mov [rsp+0x20],rax;
    ///  lea rdx,[rip+0x20]; mov r8d,10; call +0`
    fn sequence() -> Vec<u8> {
        let mut code = Vec::new();
        // lea r9, [rsi + 0x150]
        code.extend_from_slice(&[0x4c, 0x8d, 0x8e, 0x50, 0x01, 0x00, 0x00]);
        // lea rax, [rip + 0x10]
        code.extend_from_slice(&[0x48, 0x8d, 0x05, 0x10, 0x00, 0x00, 0x00]);
        // mov [rsp+0x20], rax
        code.extend_from_slice(&[0x48, 0x89, 0x44, 0x24, 0x20]);
        // lea rdx, [rip + 0x20]
        code.extend_from_slice(&[0x48, 0x8d, 0x15, 0x20, 0x00, 0x00, 0x00]);
        // mov r8d, 10
        code.extend_from_slice(&[0x41, 0xb8, 0x0a, 0x00, 0x00, 0x00]);
        // call rel32 0
        code.extend_from_slice(&[0xe8, 0x00, 0x00, 0x00, 0x00]);
        code
    }

    #[test]
    fn the_field_sequence_is_decoded() {
        let code = sequence();
        let start = 0x1000u64;
        let mut decoder = Decoder::with_ip(64, &code, start, DecoderOptions::NONE);
        let mut insn = Instruction::default();
        let mut name = None;
        let mut len = 0usize;
        let mut value = None;
        while decoder.can_decode() {
            decoder.decode_out(&mut insn);
            if insn.mnemonic() == iced_x86::Mnemonic::Lea && insn.op0_register() == Register::R9 {
                value = Some(insn.memory_displacement64() as usize);
            }
            if insn.mnemonic() == iced_x86::Mnemonic::Lea && insn.op0_register() == Register::RDX {
                name = Some(insn.ip_rel_memory_address());
            }
            if insn.mnemonic() == iced_x86::Mnemonic::Mov && insn.op0_register() == Register::R8D {
                len = insn.immediate32() as usize;
            }
        }
        assert_eq!(value, Some(0x150));
        assert_eq!(len, 10);
        assert!(name.is_some());
    }
}
