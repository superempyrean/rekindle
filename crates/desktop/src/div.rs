//! The client's own `div()` constructor and its style accessor, found by
//! shape.
//!
//! `div()` is the function every UI component calls to open a `Div`:
//! it allocates the boxed `Interactivity` (`0x4E8`), initializes it through
//! a helper that allocates the `StyleRefinement` (`0x280`) and stores it at
//! `Interactivity + N`, then copies the `Interactivity` into its sret:
//!
//! ```text
//! [sret + 0]    = 0                  ; `Div` tag
//! [sret + 8]    = boxed Interactivity
//! [sret + 0x10] = 0                  ; prepaint listener
//! [sret + 0x20] = 0                  ; image cache
//! [sret + 0x30] = 0                  ; prepaint order
//! [sret + 0x80] = 0                  ; first child slot
//! ```
//!
//! The initializer's `mov [rsi + N], rdi` gives `N`, the `StyleRefinement`
//! pointer's offset. The accessor is the unique function that loads
//! `[[Div + 8] + N]` into `rax` and returns, which is what the client's own
//! components call before they refine a style.

use anyhow::{Context, Result, bail};
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};

use crate::image::Image;

/// `55 56 57 48 81 ec 40 05 00 00 ... 48 89 ce 31 c9 e8`.
const PROLOGUE: &[u8] = &[
    0x55, 0x56, 0x57, 0x48, 0x81, 0xEC, 0x40, 0x05, 0x00, 0x00, 0x48, 0x8D, 0xAC, 0x24, 0x80, 0x00,
    0x00, 0x00, 0x48, 0xC7, 0x85, 0xB8, 0x04, 0x00, 0x00, 0xFE, 0xFF, 0xFF, 0xFF, 0x48, 0x89, 0xCE,
    0x31, 0xC9, 0xE8,
];

/// The recovered `div()`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Div {
    pub new: u64,
    /// The function `div()` calls to initialize the `Interactivity`.
    pub initialize: u64,
    /// The client's `Div` size.
    pub size: usize,
    /// The boxed `Interactivity` pointer inside the `Div`.
    pub interactivity: usize,
    /// The `Drawable<Div>` element offset.
    pub drawable_element: usize,
    /// The `Interactivity`'s `StyleRefinement` pointer offset.
    pub style_at: usize,
    /// The accessor the client's own components call for a `Div`'s style.
    pub style_fn: u64,
}

pub const SIZE: usize = 0x88;
pub const INTERACTIVITY: usize = 8;
pub const DRAWABLE_ELEMENT: usize = 0x80;
pub const STYLE_SIZE: usize = 640;

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

/// The call targets in `instructions`.
fn calls(instructions: &[Instruction]) -> Vec<u64> {
    instructions
        .iter()
        .filter(|insn| insn.mnemonic() == Mnemonic::Call)
        .filter_map(|insn| match insn.op0_kind() {
            OpKind::NearBranch64 => Some(insn.near_branch64()),
            _ => None,
        })
        .collect()
}

/// `mov [base + N], rdi` with an immediate or a register source; returns `N`.
fn stores_into(instructions: &[Instruction], register: Register) -> Vec<usize> {
    instructions
        .iter()
        .filter(|insn| insn.mnemonic() == Mnemonic::Mov)
        .filter(|insn| insn.op0_kind() == OpKind::Memory)
        .filter(|insn| insn.op1_kind() == OpKind::Register && insn.op1_register() == register)
        .map(|insn| insn.memory_displacement64() as usize)
        .collect()
}

/// Resolve `div()` and validate its stores.
pub fn resolve(image: &Image<'_>) -> Result<Div> {
    let hits = image.find(PROLOGUE);
    let hits = hits
        .into_iter()
        .filter(|&va| image.function(va).is_some())
        .collect::<Vec<_>>();
    let new = match hits.as_slice() {
        [one] => *one,
        [] => bail!("no div() with the 0x540-frame prologue"),
        many => bail!("{} div()-shaped functions: {many:x?}", many.len()),
    };
    let (start, end) = image.function_named("div()", new)?;
    let instructions = decode(image, start, end)?;
    if !instructions.iter().any(|insn| {
        insn.mnemonic() == Mnemonic::Mov
            && insn.op0_kind() == OpKind::Register
            && insn.op0_register() == Register::ECX
            && insn.op1_kind() == OpKind::Immediate32
            && insn.immediate32() == 0x4E8
    }) {
        bail!("div() at {new:#x} no longer allocates a 0x4E8 Interactivity");
    }
    if !instructions.iter().any(|insn| {
        insn.mnemonic() == Mnemonic::Mov
            && insn.op0_kind() == OpKind::Memory
            && insn.memory_displacement64() == 8
            && insn.op1_kind() == OpKind::Register
    }) {
        bail!("div() at {new:#x} no longer stores the boxed Interactivity at [sret+8]");
    }
    if !instructions.iter().any(|insn| {
        insn.mnemonic() == Mnemonic::Mov
            && insn.op0_kind() == OpKind::Memory
            && insn.memory_displacement64() == 0x80
            && matches!(
                insn.op1_kind(),
                OpKind::Immediate32
                    | OpKind::Immediate32to64
                    | OpKind::Immediate8
                    | OpKind::Immediate8to64
            )
            && insn.immediate32() == 0
    }) {
        bail!("div() at {new:#x} no longer clears [sret+0x80]");
    }

    // The initializer: the `div()` callee that allocates the 640-byte
    // `StyleRefinement`. Its `mov [rsi + N], rdi` candidates are checked
    // against the accessor: the right `N` is the one whose
    // `mov rax, [rcx + 8]; mov rax, [rax + N]` shape names exactly one
    // function.
    let mut initialize = None;
    for target in calls(&instructions) {
        let Some((istart, iend)) = image.function(target) else {
            continue;
        };
        let inner = decode(image, istart, iend)?;
        let allocates_style = inner.iter().any(|insn| {
            insn.mnemonic() == Mnemonic::Mov
                && insn.op0_kind() == OpKind::Register
                && insn.op0_register() == Register::ECX
                && insn.op1_kind() == OpKind::Immediate32
                && insn.immediate32() == 0x280
        });
        if !allocates_style {
            continue;
        }
        for at in stores_into(&inner, Register::RDI) {
            if at == 0 || at % 8 != 0 {
                continue;
            }
            if let Some(style_fn) = accessor(image, at)? {
                initialize = Some((target, at, style_fn));
                break;
            }
        }
        if initialize.is_some() {
            break;
        }
    }
    let (initialize, style_at, style_fn) =
        initialize.context("div() no longer calls a StyleRefinement initializer")?;

    Ok(Div {
        new,
        initialize,
        size: SIZE,
        interactivity: INTERACTIVITY,
        drawable_element: DRAWABLE_ELEMENT,
        style_at,
        style_fn,
    })
}

/// The unique function whose body loads `[[rcx + 8] + style_at]` into `rax`.
fn accessor(image: &Image<'_>, style_at: usize) -> Result<Option<u64>> {
    let mut needle = vec![0x48, 0x8B, 0x41, 0x08, 0x48, 0x8B, 0x80];
    needle.extend_from_slice(&(style_at as u32).to_le_bytes());
    let mut functions = Vec::new();
    for hit in image.find(&needle) {
        if let Some((start, _)) = image.function(hit) {
            functions.push(start);
        }
    }
    functions.sort_unstable();
    functions.dedup();
    match functions.as_slice() {
        [one] => Ok(Some(*one)),
        [] => Ok(None),
        many => bail!(
            "{} Div style accessors for offset {style_at:#x}: {many:x?}",
            many.len()
        ),
    }
}
