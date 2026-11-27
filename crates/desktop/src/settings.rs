//! The user settings view's pages: what lets rekindle take one over.
//!
//! The settings view's pages are a closed `u8` enum. The client draws a
//! page in the sidebar only when its visibility check passes, names it with
//! `Page::title`, and draws its content with the section render, which
//! switches on the page byte kept in the view's state. Pages 9 to 24 are
//! behind flags, and the last of them, `Phoenix`, has no content for
//! anyone outside the client's own team. rekindle takes that page over.
//!
//! - The section render is the one function that loads the string
//!   `account-loading`. Its jump table switches on
//!   `movzx eax, byte [state + page_at]`, where `state` is its second
//!   argument.
//! - `Page::title(out, page)` opens with a jump table on `dl`. The page
//!   taken over is the case that loads the text `Phoenix` with the length
//!   7 written after it.
//! - Each visibility check is `lea r32, [page - 9]; cmp r32, 0x0f; ja`,
//!   then a jump table over the gated pages. A page above the range is
//!   shown, so lowering the `0x0f` to `0x0e` shows page 24 to everyone.

use anyhow::{Context, Result, bail};
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};
use serde::Serialize;

use crate::image::Image;

const SECTION_ANCHOR: &[u8] = b"account-loading";
const TAKEN_TITLE: &[u8] = b"Phoenix";
/// The first page behind a flag.
const FIRST_GATED: i64 = 9;

/// What the settings view's pages are drawn with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Settings {
    /// The section render:
    /// `(sret: *mut AnyElement, state, data, vtable, window, cx) -> sret`.
    pub section: u64,
    /// The offset of the active page's byte in the section's state.
    pub page_at: u32,
    /// `Page::title(out: *mut SharedString, page: u8) -> out`.
    pub title: u64,
    /// The page rekindle takes over.
    pub page: u8,
    /// The `0x0f` immediate of each visibility check, by address.
    pub visibility: Vec<u64>,
}

fn decode(code: &[u8], ip: u64) -> Vec<Instruction> {
    Decoder::with_ip(64, code, ip, DecoderOptions::NONE)
        .into_iter()
        .take_while(|i| !i.is_invalid())
        .collect()
}

fn function_code(image: &Image<'_>, what: &str, va: u64) -> Result<(u64, Vec<Instruction>)> {
    let (start, end) = image.function_named(what, va)?;
    let code = image
        .read(start, usize::try_from(end - start)?)
        .with_context(|| format!("{what} at {start:#x} is unmapped"))?;
    Ok((start, decode(code, start)))
}

/// The jump table `insns[at..]` dispatches through, if it is
/// `lea r, [rip + table]; movsxd r, dword [r + idx*4]`.
fn jump_table(insns: &[Instruction]) -> Option<u64> {
    let [lea, load, ..] = insns else { return None };
    (lea.mnemonic() == Mnemonic::Lea
        && lea.memory_base() == Register::RIP
        && load.mnemonic() == Mnemonic::Movsxd
        && load.memory_base() == lea.op0_register()
        && load.memory_index_scale() == 4)
        .then(|| lea.ip_rel_memory_address())
}

/// Whether one of the next instructions is a jump table indexed by `reg`.
fn indexes_a_table(insns: &[Instruction], reg: Register) -> bool {
    (0..insns.len().min(8)).any(|k| {
        jump_table(&insns[k..]).is_some()
            && insns[k + 1].memory_index().full_register() == reg.full_register()
    })
}

/// The case `index` of the jump table at `table`.
fn case(image: &Image<'_>, table: u64, index: u64) -> Option<u64> {
    let rel = image.dword(table + 4 * index)? as i32;
    Some(table.wrapping_add_signed(i64::from(rel)))
}

/// The register the function copies its second argument into, from
/// `mov reg, rdx` among its first instructions.
fn second_argument(insns: &[Instruction]) -> Option<Register> {
    insns.iter().take(24).find_map(|i| {
        (i.mnemonic() == Mnemonic::Mov
            && i.op_count() == 2
            && i.op0_kind() == OpKind::Register
            && i.op1_kind() == OpKind::Register
            && i.op1_register() == Register::RDX)
            .then(|| i.op0_register())
    })
}

/// The section render and its page offset.
fn section(image: &Image<'_>) -> Result<(u64, u32)> {
    let mut functions: Vec<u64> = image
        .leas_into(&image.find(SECTION_ANCHOR))
        .into_iter()
        .filter_map(|(lea, _)| image.function(lea).map(|f| f.0))
        .collect();
    functions.sort_unstable();
    functions.dedup();
    let start = crate::image::one_of("the section render (`account-loading`)", &functions)?;
    let (_, insns) = function_code(image, "the section render", start)?;
    let state = second_argument(&insns).context("the section render keeps no copy of `state`")?;
    let mut offsets: Vec<u32> = insns
        .iter()
        .enumerate()
        .filter(|(_, i)| {
            i.mnemonic() == Mnemonic::Movzx
                && i.op1_kind() == OpKind::Memory
                && i.memory_base() == state
                && i.memory_index() == Register::None
                && i.memory_size() == iced_x86::MemorySize::UInt8
        })
        .filter(|(k, _)| jump_table(&insns[k + 1..]).is_some())
        .filter_map(|(_, i)| u32::try_from(i.memory_displacement64()).ok())
        .collect();
    offsets.sort_unstable();
    offsets.dedup();
    let page_at = crate::image::one_of("the section's page byte", &offsets)?;
    Ok((start, page_at))
}

/// `Page::title` and the page whose case loads [`TAKEN_TITLE`].
fn title(image: &Image<'_>) -> Result<(u64, u8)> {
    let mut found = Vec::new();
    for (lea, _) in image.leas_into(&image.find(TAKEN_TITLE)) {
        let Some(code) = image.read(lea, 0x30) else {
            continue;
        };
        // `mov qword [r + 8], 7`: the fallback text's length.
        let length = decode(code, lea).iter().take(5).any(|i| {
            i.mnemonic() == Mnemonic::Mov
                && i.op0_kind() == OpKind::Memory
                && i.memory_displacement64() == 8
                && i.op1_kind() == OpKind::Immediate32to64
                && i.immediate32to64() == TAKEN_TITLE.len() as i64
        });
        if !length {
            continue;
        }
        let Ok((start, insns)) = function_code(image, "Page::title", lea) else {
            continue;
        };
        let Some(k) = insns.iter().take(8).position(|i| {
            i.mnemonic() == Mnemonic::Movzx
                && i.op1_kind() == OpKind::Register
                && i.op1_register() == Register::DL
        }) else {
            continue;
        };
        let Some(table) = jump_table(&insns[k + 1..]) else {
            continue;
        };
        if let Some(page) = (0..=u8::MAX).find(|&p| case(image, table, u64::from(p)) == Some(lea)) {
            found.push((start, page));
        }
    }
    found.sort_unstable();
    found.dedup();
    crate::image::one_of("Page::title's `Phoenix` case", &found)
}

/// Where each visibility check `lea r32, [page - 9]; cmp r32, 0x0f; ja`
/// keeps its `0x0f`, for a check whose next instructions are its jump
/// table.
fn visibility(image: &Image<'_>, page: u8) -> Result<Vec<u64>> {
    let mut out = Vec::new();
    for at in image.find(&[0x0f, 0x77]) {
        // `[41] 83 /7 0f`: the cmp starts two or three bytes before.
        for cmp_at in [at - 2, at - 3] {
            let Some(code) = image.read(cmp_at - 4, 4 + 0x30) else {
                continue;
            };
            for lea_len in [3u64, 4] {
                let from = 4 - lea_len as usize;
                let insns = decode(&code[from..], cmp_at - lea_len);
                let [lea, cmp, ja, rest @ ..] = insns.as_slice() else {
                    continue;
                };
                let reg = lea.op0_register();
                if lea.mnemonic() == Mnemonic::Lea
                    && lea.len() == lea_len as usize
                    && lea.memory_displacement64() as i64 == -FIRST_GATED
                    && cmp.mnemonic() == Mnemonic::Cmp
                    && cmp.ip() == cmp_at
                    && cmp.op0_kind() == OpKind::Register
                    && cmp.op0_register() == reg
                    && cmp.immediate8() == 0x0f
                    && ja.mnemonic() == Mnemonic::Ja
                    && indexes_a_table(rest, reg)
                {
                    out.push(cmp.ip() + cmp.len() as u64 - 1);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    if out.is_empty() {
        bail!("no visibility check of the gated pages");
    }
    if i64::from(page) != FIRST_GATED + 0x0f {
        bail!("page {page} isn't the last gated page, so lowering the range doesn't show it");
    }
    Ok(out)
}

/// Find the settings view's section render, `Page::title`, the page taken
/// over and its visibility checks.
pub fn resolve(image: &Image<'_>) -> Result<Settings> {
    let (section, page_at) = section(image)?;
    let (title, page) = title(image)?;
    let visibility = visibility(image, page)?;
    Ok(Settings {
        section,
        page_at,
        title,
        page,
        visibility,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Build 2208's two visibility checks: the table right after the `ja`,
    /// and after the flags are loaded.
    #[test]
    fn a_visibility_check_is_a_gated_range_before_its_table() {
        let checks = [
            (
                "0fb6c9 b001 448d41f7 4183f80f 771d 488d0d21141e13 4e630481 4901c8",
                2,
            ),
            (
                "440fb602 b001 418d50f7 83fa0f 7730 488b09 488b09 448b01 0fb74904
                 48c1e120 4c09c1 4c8d05d92b2013 49631490 4c01c2",
                2,
            ),
        ];
        for (code, lea) in checks {
            let insns = decode(&hex(code), 0x1000);
            assert_eq!(insns[lea].mnemonic(), Mnemonic::Lea);
            assert_eq!(insns[lea].memory_displacement64() as i64, -FIRST_GATED);
            assert_eq!(insns[lea + 1].immediate8(), 0x0f);
            assert_eq!(insns[lea + 2].mnemonic(), Mnemonic::Ja);
            assert!(indexes_a_table(
                &insns[lea + 3..],
                insns[lea].op0_register()
            ));
            assert!(!indexes_a_table(&insns[lea + 3..], Register::EAX));
        }
    }

    /// Against the client on disk, when it is there.
    #[test]
    fn resolves_a_build_on_disk() {
        let Ok(path) = std::env::var("REKINDLE_DESKTOP_EXE") else {
            eprintln!("REKINDLE_DESKTOP_EXE not set; skipped");
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        let image = Image::parse(&bytes).unwrap();
        let s = resolve(&image).unwrap();
        eprintln!("{s:#x?}");
        let rva = |va: u64| va - image.base();
        assert_eq!(rva(s.section), 0x10e06a);
        assert_eq!(s.page_at, 0x61f);
        assert_eq!(rva(s.title), 0x8b77abe);
        assert_eq!(s.page, 24);
        assert_eq!(
            s.visibility.iter().map(|&v| rva(v)).collect::<Vec<_>>(),
            [0xdd66d, 0x107bbd]
        );
    }
}
