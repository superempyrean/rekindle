//! AArch64 code, read as far as anchors need it: the addresses code loads
//! (`adrp` with the `add` or `ldr` that completes it, `adr`, literal loads),
//! the functions it calls (`bl`), and which bits of each instruction a
//! relocation or a moved target changes, so byte signatures can mask them.
//!
//! An `adrp` loads a 4 KiB page and a later instruction adds the low 12
//! bits. The two are often several instructions apart, so a page stays
//! known for [`PAGE_REACH`] instructions, until a call clobbers it, or until
//! the instruction completing it overwrites the register.

use std::collections::HashMap;

use rekindle_sigmatch::sig::Signature;

/// How many instructions after an `adrp` an `add` or `ldr` on its register
/// still counts as completing the address.
const PAGE_REACH: usize = 32;

/// The instructions anchors read. Everything else is [`Insn::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Insn {
    /// `adrp rd, page`.
    Adrp {
        rd: u8,
        page: u64,
    },
    /// `adr rd, target`.
    Adr {
        rd: u8,
        target: u64,
    },
    /// `add rd, rn, #imm` (64-bit, `imm` already shifted).
    AddImm {
        rd: u8,
        rn: u8,
        imm: u64,
    },
    /// `ldr{b,h,w,x} rt, [rn, #imm]` (unsigned offset, `imm` already
    /// scaled).
    LdrImm {
        rt: u8,
        rn: u8,
        imm: u64,
    },
    /// `ldr rt, target`: a PC-relative literal load.
    LdrLiteral {
        rt: u8,
        target: u64,
    },
    /// `bl target`.
    Bl {
        target: u64,
    },
    /// `b target`.
    B {
        target: u64,
    },
    /// `b.cond`, `cbz`, `cbnz`, `tbz`, `tbnz`: a PC-relative branch inside a
    /// function.
    CondBranch,
    Other,
}

fn sext(value: u32, bits: u32) -> i64 {
    let shift = 64 - bits;
    (i64::from(value) << shift) >> shift
}

fn rel(pc: u64, delta: i64) -> u64 {
    pc.wrapping_add_signed(delta)
}

/// Decode the instruction `w` at `pc`.
pub(crate) fn decode(w: u32, pc: u64) -> Insn {
    let rd = (w & 31) as u8;
    let rn = ((w >> 5) & 31) as u8;
    let imm21 = || sext(((w >> 29) & 3) | (((w >> 5) & 0x7ffff) << 2), 21);
    if w & 0x9f00_0000 == 0x9000_0000 {
        Insn::Adrp {
            rd,
            page: rel(pc & !0xfff, imm21() << 12),
        }
    } else if w & 0x9f00_0000 == 0x1000_0000 {
        Insn::Adr {
            rd,
            target: rel(pc, imm21()),
        }
    } else if w & 0xff80_0000 == 0x9100_0000 {
        let imm = u64::from((w >> 10) & 0xfff) << if w & (1 << 22) != 0 { 12 } else { 0 };
        Insn::AddImm { rd, rn, imm }
    } else if w & 0x3fc0_0000 == 0x3940_0000 {
        let scale = w >> 30;
        Insn::LdrImm {
            rt: rd,
            rn,
            imm: u64::from((w >> 10) & 0xfff) << scale,
        }
    } else if w & 0xbf00_0000 == 0x1800_0000 {
        Insn::LdrLiteral {
            rt: rd,
            target: rel(pc, sext((w >> 5) & 0x7ffff, 19) << 2),
        }
    } else if w & 0xfc00_0000 == 0x9400_0000 {
        Insn::Bl {
            target: rel(pc, sext(w & 0x3ff_ffff, 26) << 2),
        }
    } else if w & 0xfc00_0000 == 0x1400_0000 {
        Insn::B {
            target: rel(pc, sext(w & 0x3ff_ffff, 26) << 2),
        }
    } else if w & 0xff00_0010 == 0x5400_0000 || w & 0x7c00_0000 == 0x3400_0000 {
        // b.cond; cbz/cbnz (011010x) and tbz/tbnz (011011x).
        Insn::CondBranch
    } else {
        Insn::Other
    }
}

/// The instruction words of `code`, from its first whole word.
fn words(code: &[u8]) -> impl Iterator<Item = u32> + '_ {
    code.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
}

/// What code at a site refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ref {
    /// An address it computes or loads from: a string, a record, a slot.
    Data(u64),
    /// A function it calls with `bl`.
    Call(u64),
}

/// Every reference in `code`, which starts at `va`: `(site, ref)`, in code
/// order. An address built by `adrp` and `add`/`ldr` is reported at the
/// `adrp`.
pub(crate) fn refs(code: &[u8], va: u64) -> Vec<(u64, Ref)> {
    let mut out = Vec::new();
    // Register -> (page, site of the adrp, instructions since).
    let mut pages: HashMap<u8, (u64, u64, usize)> = HashMap::new();
    for (i, w) in words(code).enumerate() {
        let pc = va + 4 * i as u64;
        pages.retain(|_, p| {
            p.2 += 1;
            p.2 <= PAGE_REACH
        });
        match decode(w, pc) {
            Insn::Adrp { rd, page } => {
                pages.insert(rd, (page, pc, 0));
            }
            Insn::Adr { rd, target } => {
                pages.remove(&rd);
                out.push((pc, Ref::Data(target)));
            }
            Insn::LdrLiteral { rt, target } => {
                pages.remove(&rt);
                out.push((pc, Ref::Data(target)));
            }
            Insn::AddImm { rd, rn, imm } => {
                if let Some(&(page, site, _)) = pages.get(&rn) {
                    out.push((site, Ref::Data(page + imm)));
                }
                pages.remove(&rd);
            }
            Insn::LdrImm { rt, rn, imm } => {
                if let Some(&(page, site, _)) = pages.get(&rn) {
                    out.push((site, Ref::Data(page + imm)));
                }
                pages.remove(&rt);
            }
            Insn::Bl { target } => {
                out.push((pc, Ref::Call(target)));
                // x0-x18 are caller-saved.
                pages.retain(|&r, _| r > 18);
            }
            Insn::B { .. } | Insn::CondBranch | Insn::Other => {}
        }
    }
    out
}

/// The `&str` literals `code` (starting at `va`) loads: each address built
/// by `adrp` and `add`, with the length rustc moves into a register
/// (`movz`) within [`LEN_REACH`] instructions of the `add`, when there is
/// one of at most `max`.
pub(crate) fn str_loads(code: &[u8], va: u64, max: u64) -> Vec<(u64, Option<u64>)> {
    let ws: Vec<u32> = words(code).collect();
    let mut out = Vec::new();
    let mut pages: HashMap<u8, (u64, usize)> = HashMap::new();
    for (i, &w) in ws.iter().enumerate() {
        let pc = va + 4 * i as u64;
        pages.retain(|_, p| i - p.1 <= PAGE_REACH);
        match decode(w, pc) {
            Insn::Adrp { rd, page } => {
                pages.insert(rd, (page, i));
            }
            Insn::AddImm { rd, rn, imm } => {
                if let Some(&(page, _)) = pages.get(&rn) {
                    let near = ws[i.saturating_sub(LEN_REACH)..(i + 1 + LEN_REACH).min(ws.len())]
                        .iter()
                        .filter_map(|&n| movz(n))
                        .find(|&l| (4..=max).contains(&l));
                    out.push((page + imm, near));
                }
                pages.remove(&rd);
            }
            Insn::Bl { .. } => pages.retain(|&r, _| r > 18),
            _ => {}
        }
    }
    out
}

/// How many instructions either side of the `add` [`str_loads`] looks for a
/// literal's length.
const LEN_REACH: usize = 4;

/// The value of `movz rd, #imm16` (unshifted), if `w` is one.
fn movz(w: u32) -> Option<u64> {
    (w & 0x7fe0_0000 == 0x5280_0000).then(|| u64::from((w >> 5) & 0xffff))
}

/// `code`, which starts at `va`, as a signature with every field a rebuild
/// moves wildcarded: the targets of branches, calls and PC-relative loads,
/// and the low 12 bits an `add` or `ldr` puts on an `adrp`'s page. Field
/// offsets and immediates stay, as they do in an x64 signature that keeps
/// displacements.
pub(crate) fn masked(code: &[u8], va: u64) -> Signature {
    let mut out = Vec::with_capacity(code.len());
    let mut pages: HashMap<u8, usize> = HashMap::new();
    for (i, w) in words(code).enumerate() {
        let pc = va + 4 * i as u64;
        let b = w.to_le_bytes();
        pages.retain(|_, age| {
            *age += 1;
            *age <= PAGE_REACH
        });
        let keep = |mask: [bool; 4]| -> [Option<u8>; 4] {
            std::array::from_fn(|k| mask[k].then_some(b[k]))
        };
        let entry = match decode(w, pc) {
            Insn::Adrp { rd, .. } => {
                pages.insert(rd, 0);
                [None; 4]
            }
            Insn::Adr { rd, .. } => {
                pages.remove(&rd);
                [None; 4]
            }
            Insn::LdrLiteral { rt, .. } => {
                pages.remove(&rt);
                keep([false, false, false, true])
            }
            Insn::Bl { .. } => {
                pages.retain(|&r, _| r > 18);
                [None; 4]
            }
            Insn::B { .. } => [None; 4],
            Insn::CondBranch => keep([false, false, false, true]),
            Insn::AddImm { rd, rn, .. } | Insn::LdrImm { rt: rd, rn, .. } => {
                let low12 = pages.contains_key(&rn);
                pages.remove(&rd);
                if low12 {
                    keep([true, false, false, true])
                } else {
                    keep([true; 4])
                }
            }
            Insn::Other => keep([true; 4]),
        };
        out.extend(entry);
    }
    Signature::new(out)
}

/// How many bytes of a signature are fixed.
pub(crate) fn fixed(sig: &Signature) -> usize {
    sig.bytes().iter().filter(|b| b.is_some()).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    // Instructions from the Android host (arm64, 356207), with their VAs.

    #[test]
    fn decodes_adrp_add_and_calls_at_their_addresses() {
        // 0x5459cbc: bl 0x5439ea0
        assert_eq!(
            decode(0x97ff8079, 0x5459cbc),
            Insn::Bl { target: 0x5439ea0 }
        );
        // 0x5459d54: bl 0x182ac38 (backwards, far)
        assert_eq!(
            decode(0x970f43b9, 0x5459d54),
            Insn::Bl { target: 0x182ac38 }
        );
        // add x29, sp, #0x120
        assert_eq!(
            decode(0x910483fd, 0),
            Insn::AddImm {
                rd: 29,
                rn: 31,
                imm: 0x120
            }
        );
        // ldr x23, [x20, #0x20]
        assert_eq!(
            decode(0xf9401297, 0),
            Insn::LdrImm {
                rt: 23,
                rn: 20,
                imm: 0x20
            }
        );
        // b.eq, cbz
        assert_eq!(decode(0x540001a0, 0), Insn::CondBranch);
        assert_eq!(decode(0x340008c8, 0), Insn::CondBranch);
    }

    #[test]
    fn an_adrp_and_a_later_add_are_one_reference_at_the_adrp() {
        // adrp x8, 0x46a000 at 0x5459f58; mov x1, x0; add x8, x8, #0x9ff
        let adrp = 0x9000_0000 | encode_adrp(0x5459f58, 0x46a000) | 8;
        let code = le(&[adrp, 0xaa0003e1, 0x9127fd08]);
        assert_eq!(
            decode(adrp, 0x5459f58),
            Insn::Adrp {
                rd: 8,
                page: 0x46a000
            }
        );
        assert_eq!(
            refs(&code, 0x5459f58),
            vec![(0x5459f58, Ref::Data(0x46a9ff))]
        );
    }

    #[test]
    fn a_call_clobbers_a_pending_page_in_a_caller_saved_register() {
        let adrp = 0x9000_0000 | encode_adrp(0x1000, 0x5000) | 8;
        let bl = 0x9400_0010;
        let code = le(&[adrp, bl, 0x9127fd08]);
        assert_eq!(refs(&code, 0x1000), vec![(0x1004, Ref::Call(0x1044))]);
    }

    #[test]
    fn masking_hides_what_moves_and_keeps_offsets() {
        let adrp = 0x9000_0000 | encode_adrp(0x1000, 0x5000) | 8;
        // adrp x8; add x8, x8, #0x9ff; ldr x23, [x20, #0x20]; bl
        let code = le(&[adrp, 0x9127fd08, 0xf9401297, 0x97ff8079]);
        let sig = masked(&code, 0x1000);
        assert_eq!(
            sig.to_pattern(),
            "?? ?? ?? ?? 08 ?? ?? 91 97 12 40 f9 ?? ?? ?? ??"
        );
        assert_eq!(fixed(&sig), 6);
    }

    /// The immediate bits of an `adrp` at `pc` that loads `page`.
    fn encode_adrp(pc: u64, page: u64) -> u32 {
        let delta = ((page as i64 - (pc & !0xfff) as i64) >> 12) as u32 & 0x1f_ffff;
        ((delta & 3) << 29) | ((delta >> 2) << 5)
    }
}
