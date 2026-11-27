//! The client's own text field: `mana`'s `FieldState` entity and the hosted
//! `TextInput` element drawn from it.
//!
//! A `FieldState` holds the field's text and its focus handle. The client's
//! `TextInput` draws one, and gives it the keyboard, the caret, selection,
//! the clipboard and IME composition. What rekindle needs is found here:
//!
//! - `FieldState::new(out, window, cx)` is a thunk,
//!   `mov r9, r8; mov r8, rdx; mov rsi, rcx; xor edx, edx; call inner`,
//!   and `new_multiline` the same one with `mov dl, 1`. Both call the same
//!   inner function.
//! - `cx.new(|cx| FieldState::new(window, cx))` is generic over its
//!   closure, and the client keeps many copies of it. Each one bumps the
//!   `App`'s pending updates (`inc qword [rdx + disp32]`), keeps `window`
//!   and makes ten calls, the fifth of them the thunk. The second call is
//!   `Entity::clone`.
//! - `AnyEntity`'s drop is the function that loads `Detected over-release
//!   of a entity.` and takes one off the count with `lock xadd`.
//! - `FieldState::text(out, field, app)` loads `RichTextState`'s type
//!   name, keeps `out` in `rsi`, reads its entity with
//!   `lea rcx, [r8 + entities]`, where `entities` is the `App`'s
//!   `EntityMap`, and writes a 24-byte `SharedString` into `out`. Its
//!   first call is the map's lookup, `(map, index, generation) -> data`.
//! - `TextInput`'s conversion to an `AnyElement` loads its type name and
//!   copies the element, `mov r8d, size`. `TextInput::new(out, entity)`
//!   opens with `add rcx, at; call Entity::clone`, and the last of its
//!   stores ends at that size.

use anyhow::{Context, Result, bail};
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};
use serde::Serialize;

use crate::image::{Image, one_of};

const OVER_RELEASE: &[u8] = b"Detected over-release of a entity.";
const RICH_TEXT_STATE: &[u8] = b"mana_gpui::components::rich_text::state::RichTextState";
const TEXT_INPUT: &[u8] = b"mana_html::hosted::text_input::TextInput";
/// `mov r9, r8; mov r8, rdx; mov rsi, rcx`, then `xor edx, edx` or
/// `mov dl, 1`, then the call.
const THUNK: &[u8] = &[0x4d, 0x89, 0xc1, 0x49, 0x89, 0xd0, 0x48, 0x89, 0xce];
const SINGLE: &[u8] = &[0x31, 0xd2, 0xe8];
const MULTILINE: &[u8] = &[0xb2, 0x01, 0xe8];
/// The size of an `Entity`: its id, then the entity map's `Weak` and the
/// type.
pub const ENTITY: usize = 0x20;
/// The longest `cx.new` copy taken.
const CX_NEW_MAX: u64 = 0x300;

/// The text field's functions and sizes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Field {
    /// `cx.new(|cx| FieldState::new(window, cx))`:
    /// `(out: *mut Entity, app, window) -> out`.
    pub cx_new: u64,
    /// `<AnyEntity as Drop>::drop(&mut Entity)`.
    pub entity_drop: u64,
    /// `EntityMap`'s lookup: `(map, index: u32, generation: u32) -> data`.
    pub entity_get: u64,
    /// The `App`'s `EntityMap`.
    pub entities_at: u32,
    /// `FieldState::text(out: *mut SharedString, field, app) -> out`.
    pub text: u64,
    /// `TextInput::new(out, &Entity<FieldState>) -> out`.
    pub input_new: u64,
    /// `AnyElement::new::<TextInput>`: `(out, &mut TextInput) -> out`.
    pub input_into_any: u64,
    /// The size of a `TextInput`.
    pub input_size: usize,
}

fn decode(code: &[u8], ip: u64) -> Vec<Instruction> {
    Decoder::with_ip(64, code, ip, DecoderOptions::NONE)
        .into_iter()
        .take_while(|i| !i.is_invalid())
        .collect()
}

fn function_code(image: &Image<'_>, start: u64) -> Option<(u64, Vec<Instruction>)> {
    let (start, end) = image.function(start)?;
    let code = image.read(start, usize::try_from(end - start).ok()?)?;
    Some((end, decode(code, start)))
}

fn direct_calls(insns: &[Instruction]) -> Vec<u64> {
    insns
        .iter()
        .filter(|i| i.mnemonic() == Mnemonic::Call && i.op0_kind() == OpKind::NearBranch64)
        .map(Instruction::near_branch_target)
        .collect()
}

/// The `call rel32` target at `at`.
fn call_target(image: &Image<'_>, at: u64) -> Option<u64> {
    let rel = image.dword(at)? as i32;
    Some((at + 4).wrapping_add_signed(i64::from(rel)))
}

/// `FieldState::new`: the single-line thunk whose inner function the
/// multi-line one calls too.
fn field_state_new(image: &Image<'_>) -> Result<u64> {
    let mut single = Vec::new();
    let mut multi = Vec::new();
    for at in image.find(THUNK) {
        let tail = at + THUNK.len() as u64;
        let Some(next) = image.read(tail, 3) else {
            continue;
        };
        let Some(inner) = call_target(image, tail + 3) else {
            continue;
        };
        // The thunk starts with `push rsi; sub rsp, 0x20`, five bytes before.
        let start = at - 5;
        if next == SINGLE {
            single.push((start, inner));
        } else if next == MULTILINE {
            multi.push(inner);
        }
    }
    let pairs: Vec<u64> = single
        .iter()
        .filter(|(_, inner)| multi.contains(inner))
        .map(|&(thunk, _)| thunk)
        .collect();
    one_of("FieldState::new", &pairs)
}

/// The `cx.new` copies that make `FieldState::new` their fifth call, and
/// the `Entity::clone` they call second.
fn cx_new(image: &Image<'_>, thunk: u64) -> Result<(u64, u64)> {
    let mut found = Vec::new();
    for (site, _) in image.calls_into(&[thunk]) {
        let Some((start, _)) = image.function(site) else {
            continue;
        };
        let Some((end, insns)) = function_code(image, start) else {
            continue;
        };
        if end - start > CX_NEW_MAX {
            continue;
        }
        let head = &insns[..insns.len().min(16)];
        let bumps = head.iter().any(|i| {
            i.mnemonic() == Mnemonic::Inc
                && i.op0_kind() == OpKind::Memory
                && i.memory_base() == Register::RDX
        });
        let keeps_window = head.iter().any(|i| {
            i.mnemonic() == Mnemonic::Mov
                && i.op0_kind() == OpKind::Register
                && i.op1_kind() == OpKind::Register
                && i.op1_register() == Register::R8
        });
        let calls = direct_calls(&insns);
        if bumps && keeps_window && calls.len() == 10 && calls[4] == thunk {
            found.push((start, calls[1]));
        }
    }
    found.sort_unstable();
    let clones: std::collections::BTreeSet<u64> = found.iter().map(|f| f.1).collect();
    if clones.len() > 1 {
        bail!("the cx.new copies call different clones: {clones:#x?}");
    }
    found.first().copied().context("no cx.new(FieldState::new)")
}

/// `AnyEntity`'s drop.
fn entity_drop(image: &Image<'_>) -> Result<u64> {
    let mut found = Vec::new();
    for (lea, _) in image.leas_into(&image.find(OVER_RELEASE)) {
        let Some((start, _)) = image.function(lea) else {
            continue;
        };
        let Some((_, insns)) = function_code(image, start) else {
            continue;
        };
        let takes_one_off = insns.windows(2).any(|w| {
            w[0].mnemonic() == Mnemonic::Mov
                && w[0].op1_kind() == OpKind::Immediate32to64
                && w[0].immediate32to64() == -1
                && w[1].mnemonic() == Mnemonic::Xadd
                && w[1].has_lock_prefix()
                && w[1].op1_register() == w[0].op0_register()
        });
        if takes_one_off {
            found.push(start);
        }
    }
    found.sort_unstable();
    found.dedup();
    one_of("AnyEntity's drop", &found)
}

/// `FieldState::text`, the entity map's lookup and the `App`'s map.
fn text(image: &Image<'_>) -> Result<(u64, u64, u32)> {
    let names: Vec<u64> = image.find(RICH_TEXT_STATE);
    let mut found = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (lea, _) in image.leas_into(&names) {
        let Some((start, end)) = image.function(lea) else {
            continue;
        };
        if end - start > 0x200 || !seen.insert(start) {
            continue;
        }
        let Some((_, insns)) = function_code(image, start) else {
            continue;
        };
        let head = &insns[..insns.len().min(16)];
        let keeps_out = head.iter().any(|i| {
            i.mnemonic() == Mnemonic::Mov
                && i.op0_register() == Register::RSI
                && i.op1_kind() == OpKind::Register
                && i.op1_register() == Register::RCX
        });
        let entities = head.iter().find_map(|i| {
            (i.mnemonic() == Mnemonic::Lea
                && i.op0_register() == Register::RCX
                && i.memory_base() == Register::R8
                && i.memory_index() == Register::None)
                .then(|| u32::try_from(i.memory_displacement64()).ok())
                .flatten()
        });
        let reads_entity = head.iter().any(|i| {
            i.op1_kind() == OpKind::Memory
                && i.memory_base() == Register::RDX
                && i.memory_displacement64() == 0
        });
        let returns_out = insns.windows(2).any(|w| {
            w[0].mnemonic() == Mnemonic::Mov
                && w[0].op0_register() == Register::RAX
                && w[0].op1_kind() == OpKind::Register
                && w[0].op1_register() == Register::RSI
        });
        // It writes the 24-byte `SharedString` into `out` itself: a store
        // at `[rsi + 0x10]`, none at `[rsi + 0x18]`. A neighbour with the
        // same head returns a 32-byte handle from a callee.
        let stores = |disp: u64| {
            insns.iter().any(|i| {
                i.op0_kind() == OpKind::Memory
                    && i.memory_base() == Register::RSI
                    && i.memory_displacement64() == disp
            })
        };
        let returns_out = returns_out && stores(0x10) && !stores(0x18);
        if let (true, Some(entities), true, true) = (keeps_out, entities, reads_entity, returns_out)
            && let Some(&get) = direct_calls(&insns).first()
        {
            found.push((start, get, entities));
        }
    }
    one_of("FieldState::text", &found)
}

/// `TextInput`'s conversion and its size.
fn input_into_any(image: &Image<'_>) -> Result<(u64, usize)> {
    let mut found = Vec::new();
    for (lea, _) in image.leas_into(&image.find(TEXT_INPUT)) {
        let Some((start, end)) = image.function(lea) else {
            continue;
        };
        if end - start >= 0x100 {
            continue;
        }
        let Some((_, insns)) = function_code(image, start) else {
            continue;
        };
        let sizes: Vec<usize> = insns
            .iter()
            .filter(|i| {
                i.mnemonic() == Mnemonic::Mov
                    && i.op0_kind() == OpKind::Register
                    && i.op0_register() == Register::R8D
                    && i.op1_kind() == OpKind::Immediate32
            })
            .map(|i| i.immediate32() as usize)
            .collect();
        if let [size] = sizes[..] {
            found.push((start, size));
        }
    }
    found.sort_unstable();
    found.dedup();
    one_of("AnyElement::new::<TextInput>", &found)
}

/// `TextInput::new`: of the functions that open with `add rcx, at; call
/// clone`, the one whose stores end at `size`.
fn input_new(image: &Image<'_>, clone: u64, size: usize) -> Result<u64> {
    let mut found = Vec::new();
    for (site, _) in image.calls_into(&[clone]) {
        let Some((start, _)) = image.function(site) else {
            continue;
        };
        if site - start > 0x10 {
            continue;
        }
        let Some((_, insns)) = function_code(image, start) else {
            continue;
        };
        let Some(k) = insns.iter().position(|i| i.ip() == site) else {
            continue;
        };
        let Some(add) = k.checked_sub(1).map(|p| &insns[p]) else {
            continue;
        };
        if add.mnemonic() != Mnemonic::Add || add.op0_register() != Register::RCX {
            continue;
        }
        let entity = add.immediate32to64() as usize;
        let extent = insns
            .iter()
            .filter(|i| {
                i.mnemonic() == Mnemonic::Mov
                    && i.op0_kind() == OpKind::Memory
                    && i.memory_base() == Register::RSI
            })
            .map(|i| i.memory_displacement64() as usize + i.memory_size().size())
            .max()
            .unwrap_or(0);
        if entity + ENTITY <= size && extent <= size && size - extent < 8 {
            found.push(start);
        }
    }
    found.sort_unstable();
    found.dedup();
    one_of("TextInput::new", &found)
}

/// Find the text field's functions in `image`.
pub fn resolve(image: &Image<'_>) -> Result<Field> {
    let thunk = field_state_new(image)?;
    let (cx_new, clone) = cx_new(image, thunk)?;
    let entity_drop = entity_drop(image)?;
    let (text, entity_get, entities_at) = text(image)?;
    let (input_into_any, input_size) = input_into_any(image)?;
    let input_new = input_new(image, clone, input_size)?;
    Ok(Field {
        cx_new,
        entity_drop,
        entity_get,
        entities_at,
        text,
        input_new,
        input_into_any,
        input_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against the client on disk, when it is there.
    #[test]
    fn resolves_a_build_on_disk() {
        let Ok(path) = std::env::var("REKINDLE_DESKTOP_EXE") else {
            eprintln!("REKINDLE_DESKTOP_EXE not set; skipped");
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        let image = Image::parse(&bytes).unwrap();
        let f = resolve(&image).unwrap();
        eprintln!("{f:#x?}");
        let rva = |va: u64| va - image.base();
        assert_eq!(rva(f.cx_new), 0x1905030);
        assert_eq!(rva(f.entity_drop), 0x878cdd0);
        assert_eq!(rva(f.entity_get), 0x876ffb0);
        assert_eq!(f.entities_at, 0x28);
        assert_eq!(rva(f.text), 0x9007610);
        assert_eq!(rva(f.input_new), 0x88c1850);
        assert_eq!(rva(f.input_into_any), 0x88febd0);
        assert_eq!(f.input_size, 0x2d8);
    }
}
