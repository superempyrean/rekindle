//! What the desktop backend needs from a specific client build, found by
//! shape rather than by address.
//!
//! - `MessageRow::render`: the function that loads the wire's
//!   `\x10message-content-\xc0\x00` template. Its prologue is the eight-push
//!   Rust shape, and its extent is in `.pdata`.
//! - `Drawable<Div>`'s `dyn ElementObject` vtable: a 264-byte `Drawable` whose
//!   drop glue drops a `Div` at `+0x80`, and that `Div` drop frees the boxed
//!   `Interactivity` (1248 bytes plus the `Rc` header) and its children at
//!   `+0x40`.
//! - `StyleRefinement`'s field offsets: [`crate::debug`] reads them out of
//!   the derived `Debug::fmt`.
//!
//! Every one is a check: a build whose shapes changed fails here, before
//! anything is written.

use anyhow::{Context, Result, bail};
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind};
use serde::Serialize;

use crate::debug::{self, StyleOffsets};
use crate::image::Image;

/// The template `MessageRow` loads for a message's content.
const MESSAGE_CONTENT: &[u8] = b"\x10message-content-\xc0\x00";
/// The eight-push prologue Rust emits for a large frame.
const ROW_PROLOGUE: &[u8] = &[
    0x55, 0x41, 0x57, 0x41, 0x56, 0x41, 0x55, 0x41, 0x54, 0x56, 0x57, 0x53,
];
/// `Drawable<E>` for the client's `Div`: 0x108 bytes, the `Div` at `+0x80`.
/// (The client's build differs from the published crate, which measures 0xF8.)
const DRAWABLE_SIZE: u64 = 0x108;
/// The boxed `Interactivity`'s allocation (`1248` plus the `Rc` header).
const INTERACTIVITY_ALLOC: u32 = 0x4e8;

/// Everything resolved for one build.
#[derive(Clone, Debug)]
pub struct Symbols {
    pub base: u64,
    /// Every row-shaped renderer that loads the message-content template,
    /// lowest first. The desktop accessory hook uses the first; the rest are
    /// listed so a build where the row split in two is visible.
    pub message_row_renders: Vec<u64>,
    pub div_vtable: u64,
    /// Every structurally valid `Drawable<Div>` vtable, lowest first.
    pub div_vtables: Vec<u64>,
    pub style: StyleOffsets,
}

/// The report a `check` writes.
#[derive(Serialize)]
pub struct Report {
    pub image_base: u64,
    pub message_row_renders: Vec<Rva>,
    pub div_vtable: Rva,
    pub div_vtables: Vec<Rva>,
    pub style_size: usize,
    pub style: std::collections::BTreeMap<String, usize>,
    pub checks: Vec<Check>,
}

#[derive(Serialize)]
pub struct Rva {
    pub va: u64,
    pub rva: u64,
}

#[derive(Serialize)]
pub struct Check {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

impl Symbols {
    pub fn resolve(image: &Image<'_>) -> Result<Self> {
        let message_row_renders = message_rows(image)?;
        let div_vtables = div_vtables(image)?;
        let div_vtable = *div_vtables.first().context("no Drawable<Div> vtable")?;
        let style = debug::decode(image)?;
        Ok(Self {
            base: image.base(),
            message_row_renders,
            div_vtable,
            div_vtables,
            style,
        })
    }

    #[must_use]
    pub fn report(&self) -> Report {
        let mut checks = vec![
            Check {
                name: "message-row-prologue",
                ok: !self.message_row_renders.is_empty(),
                detail: format!(
                    "{} row renderer(s): {:x?}",
                    self.message_row_renders.len(),
                    self.message_row_renders
                        .iter()
                        .map(|va| va - self.base)
                        .collect::<Vec<_>>()
                ),
            },
            Check {
                name: "drawable-div",
                ok: !self.div_vtables.is_empty(),
                detail: format!(
                    "{} Drawable<Div> vtable(s), using RVA {:#x}",
                    self.div_vtables.len(),
                    self.div_vtable - self.base
                ),
            },
            Check {
                name: "style-size",
                ok: self.style.size == 640,
                detail: format!("StyleRefinement is {} bytes", self.style.size),
            },
        ];
        for required in debug::REQUIRED {
            checks.push(Check {
                name: "style-field",
                ok: self.style.has(required),
                detail: format!("{required} at {:?}", self.style.fields.get(*required)),
            });
        }
        Report {
            image_base: self.base,
            message_row_renders: self
                .message_row_renders
                .iter()
                .map(|&va| Rva {
                    va,
                    rva: va - self.base,
                })
                .collect(),
            div_vtable: Rva {
                va: self.div_vtable,
                rva: self.div_vtable - self.base,
            },
            div_vtables: self
                .div_vtables
                .iter()
                .map(|&va| Rva {
                    va,
                    rva: va - self.base,
                })
                .collect(),
            style_size: self.style.size,
            style: self.style.fields.clone(),
            checks,
        }
    }
}

/// Find every `MessageRow`-shaped renderer from the message-content
/// template.
pub fn message_rows(image: &Image<'_>) -> Result<Vec<u64>> {
    let templates = image.find(MESSAGE_CONTENT);
    if templates.is_empty() {
        bail!("the message-content template is absent");
    }
    let mut found = Vec::new();
    for template in templates {
        for lea in image.leas_to(template, template + 1) {
            let Some((start, _end)) = image.function(lea) else {
                continue;
            };
            let Some(prologue) = image.read(start, ROW_PROLOGUE.len()) else {
                continue;
            };
            if prologue == ROW_PROLOGUE {
                found.push(start);
            }
        }
    }
    found.sort_unstable();
    found.dedup();
    if found.is_empty() {
        bail!("no eight-push function loads the message-content template");
    }
    Ok(found)
}

/// Find every `Drawable<Div>` vtable by its size and its drop glue.
pub fn div_vtables(image: &Image<'_>) -> Result<Vec<u64>> {
    let mut found = Vec::new();
    for (start, end) in image.data() {
        let len = usize::try_from(end - start).context("data section too large")?;
        let Some(bytes) = image.read(start, len) else {
            continue;
        };
        for (i, chunk) in bytes.chunks_exact(8).enumerate() {
            let at = start + i as u64 * 8;
            let Some(drop) = u64::from_le_bytes(chunk.try_into().unwrap()).checked_sub(0) else {
                continue;
            };
            if drop < image.base() {
                continue;
            }
            let Some(size) = image.qword(at + 8) else {
                continue;
            };
            let Some(align) = image.qword(at + 16) else {
                continue;
            };
            if size != DRAWABLE_SIZE || align != 8 {
                continue;
            }
            if image.section(drop) != Some(".text") {
                continue;
            }
            if drops_div(image, drop) {
                found.push(at);
            }
        }
    }
    found.sort_unstable();
    found.dedup();
    if found.is_empty() {
        bail!("no 264-byte Drawable whose drop glue drops a Div");
    }
    Ok(found)
}

/// Whether `drop` calls a function that looks like `Div::drop`: it frees a
/// boxed `Interactivity` at `0x4e8` and drops the children `SmallVec` at
/// `+0x40`.
fn drops_div(image: &Image<'_>, drop: u64) -> bool {
    let Some((start, end)) = image.function(drop) else {
        return false;
    };
    let len = usize::try_from(end - start).unwrap_or(0);
    let Some(code) = image.read(start, len) else {
        return false;
    };
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    while decoder.can_decode() {
        decoder.decode_out(&mut insn);
        if insn.mnemonic() == Mnemonic::Call
            && let Some(target) = branch_target(&insn)
            && let Some((cs, ce)) = image.function(target)
            && div_drop_shape(image, cs, ce)
        {
            return true;
        }
    }
    // Some builds keep `Div::drop` in the same function.
    div_drop_shape(image, start, end)
}

fn div_drop_shape(image: &Image<'_>, start: u64, end: u64) -> bool {
    let len = usize::try_from(end.saturating_sub(start)).unwrap_or(0);
    let Some(code) = image.read(start, len) else {
        return false;
    };
    let mut decoder = Decoder::with_ip(64, code, start, DecoderOptions::NONE);
    let mut insn = Instruction::default();
    let mut frees_interactivity = false;
    let mut drops_children = false;
    while decoder.can_decode() {
        decoder.decode_out(&mut insn);
        if let Some(value) = immediate(&insn)
            && value == INTERACTIVITY_ALLOC
        {
            frees_interactivity = true;
        }
        if insn.mnemonic() == Mnemonic::Lea
            && insn.op0_kind() == OpKind::Register
            && insn.op1_kind() == OpKind::Memory
            && insn.memory_displacement64() == 0x40
        {
            drops_children = true;
        }
        if insn.mnemonic() == Mnemonic::Add
            && insn.op1_kind() == OpKind::Immediate8to32
            && insn.immediate8to32() == 0x40
        {
            drops_children = true;
        }
    }
    frees_interactivity && drops_children
}

fn immediate(insn: &Instruction) -> Option<u32> {
    match insn.op1_kind() {
        OpKind::Immediate32 => Some(insn.immediate32()),
        OpKind::Immediate8to32 => u32::try_from(insn.immediate8to32()).ok(),
        _ => None,
    }
}

fn branch_target(insn: &Instruction) -> Option<u64> {
    match insn.op0_kind() {
        OpKind::NearBranch64 | OpKind::NearBranch32 => Some(insn.near_branch64()),
        _ => None,
    }
}
