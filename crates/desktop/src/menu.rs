//! The client's native message context menu, resolved from the build.
//!
//! The menu is a `Vec<MenuItem>` returned by `MessageContextMenu`'s item
//! builder. Each `MenuItem` is an enum: tag 2 is an `Action`, tag 4 a
//! `Separator`. An `Action` carries a `MenuItemProps` at `props`; the visible
//! label is a `SharedString` at `props + label`, whose static form is
//! `(tag 0x8018, pointer, length)`.
//!
//! Everything is recovered by shape:
//! - `build_fn` from the `MessageContextMenu` string and its `lea`;
//! - `vec_push` from the call after the `Copy Message ID` label block;
//! - `item_size` from `Vec::push`'s `imul reg, reg, imm32` stride;
//! - `props` from the `rep movsb` in `MenuItem`'s core constructor;
//! - `label` from `MenuItemProps::new`'s two stores of the label
//!   (`movups [rax+disp], xmm0` and `mov [rax+disp+0x10], rdx`).

use anyhow::{Context, Result, bail};
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};
use serde::Serialize;

use crate::image::Image;

const MENU_STRING: &[u8] = b"MessageContextMenu";
const COPY_ID: &[u8] = b"Copy Message ID";
const ROW_PROLOGUE: &[u8] = &[
    0x55, 0x41, 0x57, 0x41, 0x56, 0x41, 0x55, 0x41, 0x54, 0x56, 0x57, 0x53,
];

/// A recovered `MenuItem` layout.
#[derive(Clone, Debug, Serialize)]
pub struct Menu {
    pub build_fn: u64,
    pub vec_push: u64,
    pub item_size: usize,
    /// `MenuItemProps` inside a `MenuItem`.
    pub props: usize,
    pub props_size: usize,
    /// The visible `SharedString` inside `MenuItemProps`.
    pub label: Label,
    pub action_tag: u64,
    /// The tag observed on separator entries in the builder's own list.
    pub separator_tag: u64,
}

/// A `SharedString`'s fields, relative to the item.
#[derive(Clone, Debug, Serialize)]
pub struct Label {
    pub tag: usize,
    pub ptr: usize,
    pub len: usize,
}

impl Menu {
    /// Resolve the menu for `image`.
    pub fn resolve(image: &Image<'_>) -> Result<Self> {
        let build_fn = build_fn(image)?;
        let (start, end) = image
            .function_named("MessageContextMenu", build_fn)
            .context("MessageContextMenu has no extent")?;
        let copy_lea = lea_in_function(image, start, end, COPY_ID)
            .context("Copy Message ID lea not found in MessageContextMenu")?;
        let calls = calls_after(image, copy_lea, 0x200);
        if calls.len() < 5 {
            bail!("Copy Message ID block has only {} calls", calls.len());
        }
        let vec_push = calls[4];
        let core = core_ctor(image, calls[1])?;
        let (props, props_size) = props_layout(image, core)?;
        let (label_tag, label_ptr, label_len) = label_layout(image, core)?;
        let item_size = push_stride(image, vec_push)?;
        if item_size < props + props_size {
            bail!("MenuItem is {item_size:#x} but props ({props:#x}+{props_size:#x}) does not fit");
        }
        let label = Label {
            tag: props + label_tag,
            ptr: props + label_ptr,
            len: props + label_len,
        };
        for (name, at) in [
            ("label tag", label.tag),
            ("label ptr", label.ptr),
            ("label len", label.len),
        ] {
            if at + 8 > item_size {
                bail!("{name} at {at:#x} is outside the {item_size:#x}-byte item");
            }
        }
        let action_tag = action_tag(image, core)?;
        Ok(Self {
            build_fn,
            vec_push,
            item_size,
            props,
            props_size,
            label,
            action_tag,
            separator_tag: 4,
        })
    }
}

fn build_fn(image: &Image<'_>) -> Result<u64> {
    let strings = image
        .find(MENU_STRING)
        .into_iter()
        .filter(|&va| {
            let before = image.read(va.wrapping_sub(1), 1).map_or(0, |b| b[0]);
            before == 0 || !before.is_ascii_alphanumeric()
        })
        .collect::<Vec<_>>();
    if strings.is_empty() {
        bail!("no MessageContextMenu string");
    }
    let mut found = Vec::new();
    for string in strings {
        for lea in image.leas_to(string, string + 1) {
            if let Some((start, _)) = image.function(lea)
                && image
                    .read(start, ROW_PROLOGUE.len())
                    .is_some_and(|p| p == ROW_PROLOGUE)
            {
                found.push(start);
            }
        }
    }
    found.sort_unstable();
    found.dedup();
    match found.as_slice() {
        [one] => Ok(*one),
        [] => bail!("no eight-push function loads MessageContextMenu"),
        many => bail!(
            "{} candidate MessageContextMenu functions: {many:x?}",
            many.len()
        ),
    }
}

fn lea_in_function(image: &Image<'_>, start: u64, end: u64, needle: &[u8]) -> Option<u64> {
    let targets = image.find(needle);
    if targets.is_empty() {
        return None;
    }
    let len = usize::try_from(end - start).ok()?;
    let code = image.read(start, len)?;
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    while decoder.can_decode() {
        decoder.decode_out(&mut insn);
        if insn.mnemonic() == Mnemonic::Lea
            && insn.op1_kind() == OpKind::Memory
            && insn.memory_base() == Register::RIP
            && targets.contains(&insn.ip_rel_memory_address())
        {
            return Some(insn.ip());
        }
    }
    None
}

fn calls_after(image: &Image<'_>, start: u64, len: usize) -> Vec<u64> {
    let Some(code) = image.read(start, len) else {
        return Vec::new();
    };
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    let mut out = Vec::new();
    while decoder.can_decode() {
        decoder.decode_out(&mut insn);
        if insn.mnemonic() == Mnemonic::Call
            && let Some(target) = branch_target(&insn)
        {
            out.push(target);
        }
    }
    out
}

fn branch_target(insn: &Instruction) -> Option<u64> {
    match insn.op0_kind() {
        OpKind::NearBranch64 | OpKind::NearBranch32 => Some(insn.near_branch64()),
        _ => None,
    }
}

fn last_call(image: &Image<'_>, start: u64, len: usize) -> Option<u64> {
    calls_after(image, start, len).last().copied()
}

fn core_ctor(image: &Image<'_>, copy_ctor: u64) -> Result<u64> {
    let wrapper =
        last_call(image, copy_ctor, 0x80).context("MenuItem copy constructor has no inner call")?;
    last_call(image, wrapper, 0x100).context("MenuItem wrapper has no core call")
}

fn props_layout(image: &Image<'_>, core: u64) -> Result<(usize, usize)> {
    let (start, end) = image.function_named("MenuItem core ctor", core)?;
    let len = usize::try_from(end - start)?;
    let code = image
        .read(start, len)
        .context("MenuItem core ctor is unmapped")?;
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    let mut props = None;
    let mut props_size = None;
    while decoder.can_decode() {
        decoder.decode_out(&mut insn);
        if insn.mnemonic() == Mnemonic::Lea
            && insn.op0_kind() == OpKind::Register
            && insn.op1_kind() == OpKind::Memory
            && insn.memory_displacement64() >= 0x20
            && insn.memory_displacement64() < 0x80
        {
            props = Some(insn.memory_displacement64() as usize);
        }
        if insn.mnemonic() == Mnemonic::Mov
            && insn.op0_kind() == OpKind::Register
            && insn.op0_register() == Register::ECX
            && insn.op1_kind() == OpKind::Immediate32
        {
            props_size = Some(insn.immediate32() as usize);
        }
    }
    match (props, props_size) {
        (Some(props), Some(size)) if size > 0 => Ok((props, size)),
        _ => bail!("MenuItem props layout not found"),
    }
}

fn label_layout(image: &Image<'_>, core: u64) -> Result<(usize, usize, usize)> {
    let props_new = calls_after(image, core, 0x80)
        .first()
        .copied()
        .context("MenuItem core ctor has no props_new call")?;
    let (start, end) = image
        .function(props_new)
        .unwrap_or((props_new, props_new + 0x200));
    let len = usize::try_from(end - start)?;
    let code = image
        .read(start, len)
        .context("MenuItemProps::new is unmapped")?;
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    let mut tag = None;
    while decoder.can_decode() {
        decoder.decode_out(&mut insn);
        if insn.mnemonic() == Mnemonic::Movups
            && insn.op0_kind() == OpKind::Memory
            && insn.op1_kind() == OpKind::Register
            && insn.op1_register() == Register::XMM0
        {
            let disp = insn.memory_displacement64() as usize;
            if (0x80..0x100).contains(&disp) {
                tag = Some(disp);
            }
        }
    }
    let tag = tag.context("MenuItemProps label store not found")?;
    Ok((tag, tag + 8, tag + 16))
}

fn push_stride(image: &Image<'_>, vec_push: u64) -> Result<usize> {
    let (start, end) = image.function_named("Vec<MenuItem>::push", vec_push)?;
    let len = usize::try_from(end - start)?;
    let code = image.read(start, len).context("Vec push is unmapped")?;
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    while decoder.can_decode() {
        decoder.decode_out(&mut insn);
        if insn.mnemonic() == Mnemonic::Imul {
            for i in 0..insn.op_count() {
                match insn.op_kind(i) {
                    OpKind::Immediate32 => return Ok(insn.immediate32() as usize),
                    OpKind::Immediate32to64 => return Ok(insn.immediate32to64() as usize),
                    OpKind::Immediate8to32 => return Ok(insn.immediate8to32() as usize),
                    _ => {}
                }
            }
        }
    }
    bail!("Vec<MenuItem>::push has no stride")
}

fn action_tag(image: &Image<'_>, core: u64) -> Result<u64> {
    let (start, end) = image.function_named("MenuItem core ctor", core)?;
    let len = usize::try_from(end - start)?;
    let code = image.read(start, len).context("core ctor is unmapped")?;
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    while decoder.can_decode() {
        decoder.decode_out(&mut insn);
        if insn.mnemonic() == Mnemonic::Mov
            && insn.op0_kind() == OpKind::Memory
            && insn.memory_displacement64() == 0
        {
            let tag = match insn.op1_kind() {
                OpKind::Immediate32 => u64::from(insn.immediate32()),
                OpKind::Immediate32to64 => insn.immediate32to64() as u64,
                OpKind::Immediate8to32 => insn.immediate8to32() as u64,
                _ => continue,
            };
            if tag < 16 {
                return Ok(tag);
            }
        }
    }
    bail!("MenuItem action tag not found")
}
