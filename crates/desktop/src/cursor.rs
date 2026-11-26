//! The client's mouse cursors, read from its platform's `set_cursor_style`.
//!
//! An element shows a cursor while the mouse is over it when its style's
//! `mouse_cursor` is `Some(style)`. In paint, GPUI records the style against
//! the element's hitbox. After each frame and each mouse event, the window
//! takes the last recorded style whose hitbox is hovered and passes it to
//! `Platform::set_cursor_style`. On Windows that function maps the style to
//! a system cursor with `LoadCursorW` and posts it to the client's windows,
//! which set it on the next `WM_SETCURSOR`.
//!
//! The mapping is a `match` on the style byte, compiled to a jump table:
//!
//! ```text
//! movzx ecx, dl              ; the style
//! mov   eax, 32512           ; IDC_ARROW, the default
//! dec   ecx
//! cmp   ecx, 21
//! ja    join
//! lea   rdx, [table]
//! movsxd rcx, dword [rdx + rcx*4]
//! add   rcx, rdx
//! jmp   rcx
//! case: mov eax, 32649       ; IDC_HAND
//!       jmp join
//! ```
//!
//! [`resolve`] finds the function that holds `mov eax, IDC_HAND` and such a
//! switch, and reads the system cursor each style byte maps to. That byte is
//! the variant's own value in `CursorStyle`, and an `Option<CursorStyle>`
//! stores it unchanged for `Some`.

use anyhow::{Context, Result};
use iced_x86::{
    Code, Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic, OpKind, Register,
};
use serde::Serialize;

use crate::image::{Image, one_of};

/// `IDC_ARROW`.
pub const IDC_ARROW: u16 = 32512;
/// `IDC_HAND`.
pub const IDC_HAND: u16 = 32649;
/// The `IDC_*` values are `MAKEINTRESOURCE` ids from 32512 up.
const IDC: std::ops::RangeInclusive<u32> = 0x7f00..=0x7fff;
/// `mov eax, IDC_HAND`.
const MOV_EAX_HAND: [u8; 5] = [0xb8, 0x89, 0x7f, 0x00, 0x00];
/// How much of a function is decoded.
const FUNCTION_MAX: u64 = 0x1000;
/// How many instructions before the bounds check hold the style's
/// `movzx`, the default cursor and the bias.
const PREAMBLE: usize = 6;
/// How many instructions a case runs before its branch to the join.
const CASE_MAX: usize = 6;
/// The most cases a switch on a byte can have.
const CASES_MAX: usize = 256;

/// The platform's cursor mapping in one build.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Cursors {
    /// `Platform::set_cursor_style(&self, style)`'s entry.
    pub set_cursor_style: u64,
    /// The system cursor for each style byte from 0, `None` for a style
    /// that hides the cursor. A byte past the end maps to the default.
    pub idc: Vec<Option<u16>>,
}

impl Cursors {
    /// `CursorStyle::PointingHand`: the first style that maps to `IDC_HAND`.
    /// GPUI maps `PointingHand` and `DragLink` to it and declares
    /// `PointingHand` first.
    #[must_use]
    pub fn pointing_hand(&self) -> Option<u8> {
        let at = self.idc.iter().position(|&c| c == Some(IDC_HAND))?;
        u8::try_from(at).ok()
    }

    /// The system cursor `style` maps to, if the style shows one.
    #[must_use]
    pub fn idc_of(&self, style: u8) -> Option<u16> {
        self.idc
            .get(usize::from(style))
            .copied()
            .unwrap_or_else(|| self.idc.first().copied().flatten())
    }
}

/// The name of a system cursor, for logs.
#[must_use]
pub fn idc_name(idc: u16) -> &'static str {
    match idc {
        32512 => "IDC_ARROW",
        32513 => "IDC_IBEAM",
        32514 => "IDC_WAIT",
        32515 => "IDC_CROSS",
        32516 => "IDC_UPARROW",
        32642 => "IDC_SIZENWSE",
        32643 => "IDC_SIZENESW",
        32644 => "IDC_SIZEWE",
        32645 => "IDC_SIZENS",
        32646 => "IDC_SIZEALL",
        32648 => "IDC_NO",
        32649 => "IDC_HAND",
        32650 => "IDC_APPSTARTING",
        32651 => "IDC_HELP",
        _ => "IDC_?",
    }
}

fn decode(code: &[u8], ip: u64) -> Vec<Instruction> {
    Decoder::with_ip(64, code, ip, DecoderOptions::NONE)
        .into_iter()
        .take_while(|i| !i.is_invalid())
        .collect()
}

/// The `IDC_*` value `i` loads into a register, if it is `mov r32, imm32`.
fn loads_idc(i: &Instruction) -> Option<u16> {
    (i.code() == Code::Mov_r32_imm32 && IDC.contains(&i.immediate32()))
        .then(|| u16::try_from(i.immediate32()).ok())
        .flatten()
}

/// The 32-bit register a `movzx r32, dl` fills.
fn style_register(i: &Instruction) -> Option<Register> {
    (i.mnemonic() == Mnemonic::Movzx
        && i.op1_kind() == OpKind::Register
        && i.op1_register() == Register::DL)
        .then(|| i.op0_register())
}

/// The cursor each style maps to, from the switch in `code`. `table(at, n)`
/// reads `n` jump-table entries at `at`.
fn switch(
    code: &[Instruction],
    table: impl Fn(u64, usize) -> Option<Vec<i32>>,
) -> Option<Vec<Option<u16>>> {
    for (k, cmp) in code.iter().enumerate() {
        if cmp.mnemonic() != Mnemonic::Cmp
            || cmp.op0_kind() != OpKind::Register
            || !matches!(cmp.op1_kind(), OpKind::Immediate8to32 | OpKind::Immediate32)
        {
            continue;
        }
        let Some(ja) = code.get(k + 1) else { continue };
        if ja.mnemonic() != Mnemonic::Ja {
            continue;
        }
        let index = cmp.op0_register();
        let before = &code[k.saturating_sub(PREAMBLE)..k];
        if !before.iter().any(|i| style_register(i) == Some(index)) {
            continue;
        }
        let bias: u32 = match before
            .iter()
            .rev()
            .find(|i| i.op0_kind() == OpKind::Register && i.op0_register() == index)
        {
            Some(i) if i.mnemonic() == Mnemonic::Dec => 1,
            Some(i) if i.mnemonic() == Mnemonic::Sub && i.op1_kind() == OpKind::Immediate8to32 => {
                i.immediate32()
            }
            _ => 0,
        };
        let default = before.iter().rev().find_map(loads_idc);
        let join = ja.near_branch_target();
        let Some(base) = code[k + 2..]
            .iter()
            .take(3)
            .find(|i| i.mnemonic() == Mnemonic::Lea && i.is_ip_rel_memory_operand())
            .map(Instruction::ip_rel_memory_address)
        else {
            continue;
        };
        let cases = usize::try_from(cmp.immediate32()).ok()? + 1;
        if cases > CASES_MAX {
            continue;
        }
        let entries = table(base, cases)?;
        let mut idc = vec![default; usize::try_from(bias).ok()?];
        for entry in entries {
            let target = base.wrapping_add_signed(i64::from(entry));
            idc.push(case(code, target, join, default));
        }
        return Some(idc);
    }
    None
}

/// The cursor the case at `target` loads before it branches, or the
/// default when it is the join itself.
fn case(code: &[Instruction], target: u64, join: u64, default: Option<u16>) -> Option<u16> {
    if target == join {
        return default;
    }
    let start = code.partition_point(|i| i.ip() < target);
    for i in code.get(start..)?.iter().take(CASE_MAX) {
        if let Some(idc) = loads_idc(i) {
            return Some(idc);
        }
        if i.flow_control() != FlowControl::Next {
            return None;
        }
    }
    None
}

/// Find `set_cursor_style` in `image` and read its mapping.
pub fn resolve(image: &Image<'_>) -> Result<Cursors> {
    let mut functions: Vec<(u64, u64)> = image
        .find(&MOV_EAX_HAND)
        .into_iter()
        .filter_map(|hit| image.function(hit))
        .collect();
    functions.sort_unstable();
    functions.dedup();
    let mut found = Vec::new();
    for (start, end) in functions {
        let len = usize::try_from((end - start).min(FUNCTION_MAX))?;
        let Some(bytes) = image.read(start, len) else {
            continue;
        };
        let code = decode(bytes, start);
        let table = |at: u64, n: usize| {
            let raw = image.read(at, n * 4)?;
            Some(
                raw.chunks_exact(4)
                    .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
                    .collect(),
            )
        };
        if let Some(idc) = switch(&code, table)
            && idc.contains(&Some(IDC_HAND))
            && idc.first() == Some(&Some(IDC_ARROW))
        {
            found.push(Cursors {
                set_cursor_style: start,
                idc,
            });
        }
    }
    let starts: Vec<u64> = found.iter().map(|c| c.set_cursor_style).collect();
    let one = one_of("set_cursor_style", &starts).context("the platform's cursor mapping")?;
    Ok(found
        .into_iter()
        .find(|c| c.set_cursor_style == one)
        .unwrap())
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

    /// The first 0x104 bytes of build 2208's `set_cursor_style`, through the
    /// last case, and its jump table.
    const FUNCTION: (u64, &str) = (
        0xf1244c0,
        "5541574156415541545657534883ec68488d6c246048c74500feffffff4889ce0fb6ca488d3d461e
         d708b8007f0000ffc983f9150f87bb000000488d15dfbd140848630c8a4801d1ffe1488d3d5f1ed7
         08b8847f0000e99a000000488d3d3e1ed708b8857f0000e989000000488d3d1d1ed708b8017f0000
         eb7b488d3ddf1dd708b8897f0000eb6d488d3d311ed708b8837f0000eb5f488d3db31dd708b8887f
         0000eb51488d3dd51dd708b8037f0000eb4331d2488b868001000083b8280100000174584883fa01
         0f85bc010000488955f048894de84889cfeb64488d3d5e1dd708b88a7f0000eb0c488d3de01dd708
         b8827f0000488945c88b470885c00f8597010000",
    );
    const TABLE: (u64, &str) = (
        0x172702e0,
        "4c42ebf78442ebf7d542ebf7d542ebf75a42ebf72a42ebf72a42ebf72a42ebf73b42ebf73b42ebf7
         3b42ebf7c942ebf76842ebf72a42ebf73b42ebf74c42ebf77642ebf75a42ebf7d542ebf7d542ebf7
         bb42ebf79242ebf7",
    );

    fn build_2208() -> Vec<Option<u16>> {
        let code = decode(&hex(FUNCTION.1), FUNCTION.0);
        let table = hex(TABLE.1);
        switch(&code, |at, n| {
            let from = usize::try_from(at - TABLE.0).ok()?;
            let raw = table.get(from..from + n * 4)?;
            Some(
                raw.chunks_exact(4)
                    .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
                    .collect(),
            )
        })
        .unwrap()
    }

    /// GPUI's `CursorStyle` order, with the fork's one extra variant before
    /// `None` mapping to `IDC_APPSTARTING`.
    #[test]
    fn build_2208_maps_gpuis_order() {
        let idc = build_2208();
        let expect = [
            Some(32512), // Arrow
            Some(32513), // IBeam
            Some(32515), // Crosshair
            Some(32512), // ClosedHand
            Some(32512), // OpenHand
            Some(32649), // PointingHand
            Some(32644), // ResizeLeft
            Some(32644), // ResizeRight
            Some(32644), // ResizeLeftRight
            Some(32645), // ResizeUp
            Some(32645), // ResizeDown
            Some(32645), // ResizeUpDown
            Some(32642), // ResizeUpLeftDownRight
            Some(32643), // ResizeUpRightDownLeft
            Some(32644), // ResizeColumn
            Some(32645), // ResizeRow
            Some(32513), // IBeamCursorForVerticalLayout
            Some(32648), // OperationNotAllowed
            Some(32649), // DragLink
            Some(32512), // DragCopy
            Some(32512), // ContextualMenu
            Some(32650), // the fork's own
            None,        // None
        ];
        assert_eq!(idc, expect);
        let cursors = Cursors {
            set_cursor_style: FUNCTION.0,
            idc,
        };
        assert_eq!(cursors.pointing_hand(), Some(5));
        assert_eq!(cursors.idc_of(5), Some(IDC_HAND));
        assert_eq!(cursors.idc_of(22), None);
        assert_eq!(cursors.idc_of(200), Some(IDC_ARROW));
        assert_eq!(idc_name(IDC_HAND), "IDC_HAND");
    }

    /// A switch whose index isn't the second argument's byte is not this one.
    #[test]
    fn a_switch_on_another_register_is_skipped() {
        let mut bytes = hex(FUNCTION.1);
        // movzx ecx, dl -> movzx ecx, al
        let at = bytes
            .windows(3)
            .position(|w| w == [0x0f, 0xb6, 0xca])
            .unwrap();
        bytes[at + 2] = 0xc8;
        let code = decode(&bytes, FUNCTION.0);
        assert_eq!(switch(&code, |_, n| Some(vec![0; n])), None);
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
        let c = resolve(&image).unwrap();
        eprintln!(
            "set_cursor_style {:#x}: {:?}",
            c.set_cursor_style - image.base(),
            c.idc
        );
        assert_eq!(c.pointing_hand(), Some(5));
        // The entry is movable by an inline detour: the eight-push prologue.
        assert_eq!(image.read(c.set_cursor_style, 2), Some(&[0x55, 0x41][..]));
    }
}
