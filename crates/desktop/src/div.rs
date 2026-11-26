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
    /// `Div::into_any_element`: the arena conversion that copies the `Div`
    /// into a `Drawable<Div>` and returns an owned `AnyElement`.
    pub into_any: u64,
    /// The children `SmallVec` and its element.
    pub children: Children,
}

/// `Div`'s children: a `SmallVec<[AnyElement; 2]>`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Children {
    /// The `SmallVec`'s offset in the `Div`.
    pub at: usize,
    /// The `AnyElement` stride (the client's is four words).
    pub element: usize,
    /// Inline capacity; while the vector is inline the capacity word is
    /// also its length.
    pub inline: usize,
    /// The capacity/length word's offset in the `Div`.
    pub capacity_at: usize,
}

pub const SIZE: usize = 0x88;
pub const INTERACTIVITY: usize = 8;
pub const DRAWABLE_ELEMENT: usize = 0x80;
pub const STYLE_SIZE: usize = 640;

/// A small immediate operand, whatever width iced chose.
fn small_immediate(insn: &Instruction) -> Option<i64> {
    match insn.op1_kind() {
        OpKind::Immediate8 => Some(i64::from(insn.immediate8())),
        OpKind::Immediate8to32 => Some(i64::from(insn.immediate8to32())),
        OpKind::Immediate8to64 => Some(insn.immediate8to64()),
        OpKind::Immediate32 => Some(i64::from(insn.immediate32())),
        OpKind::Immediate32to64 => Some(insn.immediate32to64()),
        _ => None,
    }
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
    let children = children(image, &instructions)?;
    let vtable = crate::symbols::div_vtables(image)?
        .first()
        .copied()
        .context("no Drawable<Div> vtable")?;
    let into_any = into_any_function(image, vtable)?;

    Ok(Div {
        new,
        initialize,
        size: SIZE,
        interactivity: INTERACTIVITY,
        drawable_element: DRAWABLE_ELEMENT,
        style_at,
        style_fn,
        into_any,
        children,
    })
}

/// `Div::into_any_element`: the only function that references the plain
/// `Drawable<Div>` vtable, checks the live element arena (`xor ecx, ecx;
/// call`), and copies a 0x88-byte `Div`.
fn into_any_function(image: &Image<'_>, vtable: u64) -> Result<u64> {
    let mut functions = Vec::new();
    for hit in image.leas_to(vtable, vtable + 1) {
        if let Some((start, _)) = image.function(hit) {
            functions.push(start);
        }
    }
    functions.sort_unstable();
    functions.dedup();
    let mut live = Vec::new();
    for start in functions {
        let (_, end) = image.function(start).context("into_any is outside .pdata")?;
        let len = usize::try_from(end - start)?;
        let code = image.read(start, len).context("into_any is unmapped")?;
        if code.windows(3).any(|w| w == [0x31, 0xC9, 0xE8])
            && code
                .windows(6)
                .any(|w| w == [0x41, 0xB8, 0x88, 0x00, 0x00, 0x00])
        {
            live.push(start);
        }
    }
    match live.as_slice() {
        [one] => Ok(*one),
        [] => bail!("no arena into_any for the Drawable<Div> vtable {vtable:#x}"),
        many => bail!(
            "{} arena into_any functions for vtable {vtable:#x}: {many:x?}",
            many.len()
        ),
    }
}

/// The children `SmallVec`'s shape, from the function `Div::drop` calls on
/// `&children`: `mov rax, [rcx + cap]; cmp rax, 2`, then a loop that steps
/// `rdi` by the element stride.
fn children(image: &Image<'_>, div: &[Instruction]) -> Result<Children> {
    /// `mov rax, [rcx + cap]; cmp rax, inline`.
    const CAP: &[u8] = &[0x48, 0x8B, 0x41, 0x40, 0x48, 0x83, 0xF8, 0x02];
    let mut shapes = Vec::new();
    for hit in image.find(CAP) {
        let Some((start, end)) = image.function(hit) else {
            continue;
        };
        let len = usize::try_from(end - start)?;
        let Some(code) = image.read(start, len) else {
            continue;
        };
        // The push copies from `[rdx]`; the drop does not.
        if code.windows(3).any(|w| w == [0x0F, 0x10, 0x02]) {
            continue;
        }
        // `add rdi, stride`.
        if !code.windows(4).any(|w| w == [0x48, 0x83, 0xC7, 0x20]) {
            continue;
        }
        let body = decode(image, start, end)?;
        let mut cap_rel = None;
        let mut inline = None;
        let mut element = None;
        for insn in &body {
            if insn.mnemonic() == Mnemonic::Mov
                && insn.op0_kind() == OpKind::Register
                && insn.op0_register() == Register::RAX
                && insn.op1_kind() == OpKind::Memory
                && insn.memory_base() == Register::RCX
                && insn.memory_displacement64() != 0
            {
                cap_rel = Some(insn.memory_displacement64() as usize);
            }
            if insn.mnemonic() == Mnemonic::Cmp
                && insn.op0_kind() == OpKind::Register
                && insn.op0_register() == Register::RAX
                && let Some(value) = small_immediate(insn)
            {
                inline = Some(value as usize);
            }
            if insn.mnemonic() == Mnemonic::Add
                && insn.op0_kind() == OpKind::Register
                && insn.op0_register() == Register::RDI
                && let Some(value) = small_immediate(insn)
            {
                element = Some(value as usize);
            }
        }
        if let (Some(cap), Some(inline), Some(element)) = (cap_rel, inline, element) {
            shapes.push((start, cap, inline, element));
        }
    }
    shapes.sort_unstable();
    shapes.dedup();
    let (_, cap_rel, inline, element) = match shapes.as_slice() {
        [one] => *one,
        [] => bail!("no Div children drop found"),
        many => bail!("{} Div children drops: {many:x?}", many.len()),
    };
    if inline == 0 || element < 8 || !element.is_multiple_of(8) {
        bail!("implausible children: inline {inline}, element {element}");
    }
    // The capacity word is one of the words `div()` clears; the only one
    // that leaves an aligned `SmallVec` after the three option boxes is the
    // right one.
    let mut found = None;
    for insn in div {
        if insn.mnemonic() != Mnemonic::Mov
            || insn.op0_kind() != OpKind::Memory
            || small_immediate(insn) != Some(0)
        {
            continue;
        }
        let capacity_at = insn.memory_displacement64() as usize;
        let Some(at) = capacity_at.checked_sub(cap_rel) else {
            continue;
        };
        if at < 0x38
            || !at.is_multiple_of(8)
            || capacity_at + 8 > SIZE
            || at + element * inline > capacity_at
            || capacity_at - at != cap_rel
        {
            continue;
        }
        if found.replace((at, capacity_at)).is_some() {
            bail!("more than one children capacity word");
        }
    }
    let (at, capacity_at) = found.context("no children capacity word among div()'s clears")?;
    Ok(Children {
        at,
        element,
        inline,
        capacity_at,
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
