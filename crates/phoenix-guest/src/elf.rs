//! The parts of the ELF format the loader reads, Android's packed
//! relocations, and the stack-guard rewrite. Everything here works on byte
//! slices, so it is tested without the library.

use anyhow::{Result, bail, ensure};

pub(crate) const PT_LOAD: u32 = 1;
pub(crate) const PT_DYNAMIC: u32 = 2;
pub(crate) const PT_GNU_EH_FRAME: u32 = 0x6474_e550;
pub(crate) const PT_GNU_RELRO: u32 = 0x6474_e552;
pub(crate) const PF_X: u32 = 1;
pub(crate) const PF_W: u32 = 2;
pub(crate) const PF_R: u32 = 4;

pub(crate) const DT_PLTRELSZ: i64 = 2;
pub(crate) const DT_STRTAB: i64 = 5;
pub(crate) const DT_SYMTAB: i64 = 6;
pub(crate) const DT_RELA: i64 = 7;
pub(crate) const DT_RELASZ: i64 = 8;
pub(crate) const DT_JMPREL: i64 = 23;
pub(crate) const DT_INIT_ARRAY: i64 = 25;
pub(crate) const DT_INIT_ARRAYSZ: i64 = 27;
pub(crate) const DT_ANDROID_RELA: i64 = 0x6000_0011;
pub(crate) const DT_VERSYM: i64 = 0x6fff_fff0;

pub(crate) const R_X86_64_NONE: u32 = 0;
pub(crate) const R_X86_64_64: u32 = 1;
pub(crate) const R_X86_64_GLOB_DAT: u32 = 6;
pub(crate) const R_X86_64_JUMP_SLOT: u32 = 7;
pub(crate) const R_X86_64_RELATIVE: u32 = 8;

pub(crate) const STB_WEAK: u8 = 2;
pub(crate) const STT_NOTYPE: u8 = 0;
pub(crate) const STT_FUNC: u8 = 2;

pub(crate) fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}
pub(crate) fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
pub(crate) fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// `Elf64_Phdr`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Phdr {
    pub(crate) kind: u32,
    pub(crate) flags: u32,
    pub(crate) offset: u64,
    pub(crate) vaddr: u64,
    pub(crate) filesz: u64,
    pub(crate) memsz: u64,
}

/// The header fields the loader uses.
pub(crate) struct Header {
    pub(crate) phoff: u64,
    pub(crate) phnum: u16,
    pub(crate) phdrs: Vec<Phdr>,
}

pub(crate) fn header(file: &[u8]) -> Result<Header> {
    ensure!(file.len() >= 64, "too short for an ELF header");
    ensure!(&file[..4] == b"\x7fELF", "not an ELF file");
    ensure!(
        file[4] == 2 && file[5] == 1,
        "not a little-endian ELF64 file"
    );
    ensure!(u16_at(file, 18) == 62, "not an x86_64 ELF");
    let phoff = u64_at(file, 32);
    let phentsize = usize::from(u16_at(file, 54));
    let phnum = u16_at(file, 56);
    ensure!(
        phentsize == 56,
        "unexpected program header size {phentsize}"
    );
    let mut phdrs = Vec::new();
    for i in 0..usize::from(phnum) {
        let at = phoff as usize + i * 56;
        ensure!(at + 56 <= file.len(), "program header {i} past the end");
        phdrs.push(Phdr {
            kind: u32_at(file, at),
            flags: u32_at(file, at + 4),
            offset: u64_at(file, at + 8),
            vaddr: u64_at(file, at + 16),
            filesz: u64_at(file, at + 32),
            memsz: u64_at(file, at + 40),
        });
    }
    Ok(Header {
        phoff,
        phnum,
        phdrs,
    })
}

/// One relocation: offset, `r_info` and addend.
pub(crate) type Rela = (u64, u64, i64);

struct Sleb<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Sleb<'_> {
    fn next(&mut self) -> Result<i64> {
        let mut r = 0i64;
        let mut shift = 0;
        loop {
            let Some(&b) = self.bytes.get(self.pos) else {
                bail!("packed relocations end inside a number");
            };
            self.pos += 1;
            if shift < 64 {
                r |= i64::from(b & 0x7f) << shift;
            }
            shift += 7;
            if b & 0x80 == 0 {
                if shift < 64 && b & 0x40 != 0 {
                    r |= -1i64 << shift;
                }
                return Ok(r);
            }
        }
    }
}

const GROUPED_BY_INFO: i64 = 1;
const GROUPED_BY_OFFSET_DELTA: i64 = 2;
const GROUPED_BY_ADDEND: i64 = 4;
const GROUP_HAS_ADDEND: i64 = 8;

/// Decodes Android's packed relocations (`DT_ANDROID_RELA`, starting with
/// `APS2`) in order, as bionic's linker applies them.
pub(crate) fn aps2(bytes: &[u8], mut f: impl FnMut(Rela) -> Result<()>) -> Result<()> {
    let Some(rest) = bytes.strip_prefix(b"APS2") else {
        bail!("packed relocations: bad magic");
    };
    let mut s = Sleb {
        bytes: rest,
        pos: 0,
    };
    let count = s.next()?;
    let mut offset = s.next()?;
    let (mut info, mut addend, mut delta) = (0i64, 0i64, 0i64);
    let mut done = 0;
    while done < count {
        let size = s.next()?;
        let flags = s.next()?;
        ensure!(size > 0, "packed relocations: empty group");
        let by_info = flags & GROUPED_BY_INFO != 0;
        let by_offset = flags & GROUPED_BY_OFFSET_DELTA != 0;
        let by_addend = flags & GROUPED_BY_ADDEND != 0;
        let has_addend = flags & GROUP_HAS_ADDEND != 0;
        if by_offset {
            delta = s.next()?;
        }
        if by_info {
            info = s.next()?;
        }
        if has_addend && by_addend {
            addend = addend.wrapping_add(s.next()?);
        }
        if !has_addend {
            addend = 0;
        }
        for _ in 0..size {
            offset = offset.wrapping_add(if by_offset { delta } else { s.next()? });
            if !by_info {
                info = s.next()?;
            }
            if has_addend && !by_addend {
                addend = addend.wrapping_add(s.next()?);
            }
            f((offset as u64, info as u64, addend))?;
            done += 1;
        }
    }
    Ok(())
}

/// Plain `Elf64_Rela` records.
pub(crate) fn rela(bytes: &[u8]) -> impl Iterator<Item = Rela> + '_ {
    bytes
        .as_chunks::<24>()
        .0
        .iter()
        .map(|r| (u64_at(r, 0), u64_at(r, 8), u64_at(r, 16) as i64))
}

/// Rewrites every `mov r64, fs:[0x28]` in `text` to read the 8 bytes at
/// `slot` instead. `text` is mapped at `text_addr`. Windows keeps no thread
/// pointer in `fs`, so the stack guard reads fault there.
///
/// The read is nine bytes, `64 48|4c 8b modrm 25 28000000` with mod 00 and
/// rm 100. It becomes `mov r64, [rip+disp32]` (seven bytes, the same
/// register) and a two-byte nop. Returns the number of sites.
#[cfg(any(windows, test))]
pub(crate) fn patch_stack_guard(text: &mut [u8], text_addr: usize, slot: usize) -> Result<usize> {
    let mut n = 0;
    let mut i = 0;
    while i + 9 <= text.len() {
        let p = &text[i..i + 9];
        let hit = p[0] == 0x64
            && (p[1] == 0x48 || p[1] == 0x4c)
            && p[2] == 0x8b
            && p[3] & 0xc7 == 0x04
            && p[4] == 0x25
            && p[5..9] == [0x28, 0, 0, 0];
        if !hit {
            i += 1;
            continue;
        }
        let disp = slot as i64 - (text_addr + i + 7) as i64;
        let Ok(disp) = i32::try_from(disp) else {
            bail!("the stack guard slot is out of rip range");
        };
        let reg = (p[3] >> 3) & 7;
        let rex = p[1];
        let p = &mut text[i..i + 9];
        p[0] = rex;
        p[1] = 0x8b;
        p[2] = reg << 3 | 5;
        p[3..7].copy_from_slice(&disp.to_le_bytes());
        p[7] = 0x66;
        p[8] = 0x90;
        n += 1;
        i += 9;
    }
    Ok(n)
}

/// The `&str` the guest's client-access evaluator reads, the only function
/// that references it. Overwriting that function to return `Some(true)`
/// removes the staff-only rollout gate.
pub(crate) const ACCESS_EXPERIMENT: &[u8] = b"2026-11-phoenix-app-access";

/// Function start addresses from a `.eh_frame_hdr` binary-search table.
/// `bytes` is the table's mapped memory and `hdr_addr` its mapped address.
/// Only the encodings an Android `x86_64` build uses are read (a `datarel`
/// `sdata4` table, a `udata4` count, a `pcrel` `sdata4` frame pointer); any
/// other header returns an empty list, so the caller skips the patch.
pub(crate) fn eh_frame_starts(bytes: &[u8], hdr_addr: usize) -> Vec<usize> {
    if bytes.len() < 12 || bytes[0] != 1 || bytes[1] != 0x1b || bytes[2] != 0x03 || bytes[3] != 0x3b
    {
        return Vec::new();
    }
    let count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let mut starts = Vec::with_capacity(count);
    let mut off = 12;
    for _ in 0..count {
        let Some(cell) = bytes.get(off..off + 4) else {
            break;
        };
        let loc = i32::from_le_bytes(cell.try_into().unwrap());
        starts.push((hdr_addr as i64 + i64::from(loc)) as usize);
        off += 8;
    }
    starts.sort_unstable();
    starts
}

/// The start address of the function containing `ip`: the largest table entry
/// not past it.
pub(crate) fn function_of(starts: &[usize], ip: usize) -> Option<usize> {
    let i = starts.partition_point(|&s| s <= ip);
    (i > 0).then(|| starts[i - 1])
}

/// The address of every `lea r64, [rip+disp32]` in `text` (mapped at
/// `text_addr`) whose target is one of `targets` (sorted). That is how a
/// function materializes a `&'static` pointer on `x86_64`.
pub(crate) fn lea_rip_refs(text: &[u8], text_addr: usize, targets: &[usize]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 7 <= text.len() {
        // REX.W (0x48..=0x4f), opcode 8D, ModRM mod=00 rm=101 (rip-relative).
        if (0x48..=0x4f).contains(&text[i]) && text[i + 1] == 0x8d && text[i + 2] & 0xc7 == 0x05 {
            let disp = i32::from_le_bytes(text[i + 3..i + 7].try_into().unwrap());
            let target = (text_addr + i + 7) as i64 + i64::from(disp);
            if target >= 0 && targets.binary_search(&(target as usize)).is_ok() {
                out.push(text_addr + i);
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eh_frame_table() {
        let mut h = vec![1u8, 0x1b, 0x03, 0x3b]; // version, enc, enc, datarel sdata4
        h.extend(0i32.to_le_bytes()); // eh_frame_ptr, unused here
        h.extend(3u32.to_le_bytes()); // fde count
        // datarel locs relative to hdr_addr 0x1000, deliberately out of order.
        for (loc, fde) in [(0x300i32, 0i32), (0x100, 0), (0x200, 0)] {
            h.extend(loc.to_le_bytes());
            h.extend(fde.to_le_bytes());
        }
        let starts = eh_frame_starts(&h, 0x1000);
        assert_eq!(starts, vec![0x1100, 0x1200, 0x1300]);
        // A header with any other encoding is refused.
        h[3] = 0x3c;
        assert!(eh_frame_starts(&h, 0x1000).is_empty());
    }

    #[test]
    fn containing_function() {
        let starts = [0x10, 0x20, 0x30];
        assert_eq!(function_of(&starts, 0x25), Some(0x20));
        assert_eq!(function_of(&starts, 0x30), Some(0x30));
        assert_eq!(function_of(&starts, 0x2f), Some(0x20));
        assert_eq!(function_of(&starts, 0x0f), None);
    }

    #[test]
    fn lea_targets() {
        let text_addr = 0x2000usize;
        // lea rax, [rip+disp] at offset 0, aimed at 0x9000.
        let disp = (0x9000i64 - (text_addr as i64 + 7)) as i32;
        let mut text = vec![0x48, 0x8d, 0x05];
        text.extend(disp.to_le_bytes());
        // A second lea aimed elsewhere, which must not match.
        text.extend([0x48, 0x8d, 0x0d, 0, 0, 0, 0]);
        let refs = lea_rip_refs(&text, text_addr, &[0x9000]);
        assert_eq!(refs, vec![0x2000]);
    }

    /// Full load of the real host with the gate patch on, when
    /// `PHOENIX_TEST_SO` points at an extracted `liblibdiscord_wasm_host.so`.
    /// Checks that exactly one function is found and patched: the log says so,
    /// and the patched entry begins with `mov al, 1; ret` (past any endbr64).
    #[test]
    #[ignore = "needs PHOENIX_TEST_SO=<path to liblibdiscord_wasm_host.so>"]
    fn access_gate_on_real_host() {
        let Some(so) = std::env::var_os("PHOENIX_TEST_SO") else {
            return;
        };
        let log = std::env::temp_dir().join("phoenix-gate-test.log");
        let mut opts = crate::Options::new(std::path::PathBuf::from(so));
        opts.log = Some(log.clone());
        opts.patch_access_gate = true;
        crate::load(&opts).expect("the host loads");
        let text = std::fs::read_to_string(&log).unwrap();
        let line = text
            .lines()
            .find(|l| l.contains("access gate"))
            .expect("the loader logged the gate");
        assert!(
            line.contains("neutralized at 0x"),
            "gate not patched: {line}"
        );
        let at = usize::from_str_radix(line.rsplit("0x").next().unwrap().trim(), 16).unwrap();
        // The patched entry returns Some(true); an endbr64 may precede it.
        // SAFETY: `at` is the patched entry in the mapped, still-live image.
        let head = unsafe { std::slice::from_raw_parts(at as *const u8, 7) };
        let body = if head[..4] == [0xf3, 0x0f, 0x1e, 0xfa] {
            &head[4..7]
        } else {
            &head[..3]
        };
        assert_eq!(body, [0xb0, 0x01, 0xc3]);
    }

    fn sleb(mut v: i64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            let done = (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0);
            out.push(if done { b } else { b | 0x80 });
            if done {
                return;
            }
        }
    }

    #[test]
    fn aps2_groups() {
        let mut s = b"APS2".to_vec();
        // 5 relocations from offset 0x1000.
        for v in [5, 0x1000] {
            sleb(v, &mut s);
        }
        // 3 RELATIVE by offset delta 8 and info, addends each.
        for v in [3, 1 | 2 | 8, 8, 8, 0x100, 0x10, -0x8] {
            sleb(v, &mut s);
        }
        // 2 with their own offsets and infos, no addend.
        for v in [2, 0, 0x20, (7 << 32) | 7, 0x18, (9 << 32) | 6] {
            sleb(v, &mut s);
        }
        let mut got = Vec::new();
        aps2(&s, |r| {
            got.push(r);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            got,
            [
                (0x1008, 8, 0x100),
                (0x1010, 8, 0x110),
                (0x1018, 8, 0x108),
                (0x1038, (7 << 32) | 7, 0),
                (0x1050, (9 << 32) | 6, 0),
            ]
        );
    }

    #[test]
    fn aps2_refuses_bad_magic() {
        assert!(aps2(b"APS1\0\0", |_| Ok(())).is_err());
    }

    #[test]
    fn stack_guard_rewrite() {
        // mov rax, fs:[0x28]; nop; mov r9, fs:[0x28]
        let mut text = vec![0x64, 0x48, 0x8b, 0x04, 0x25, 0x28, 0, 0, 0, 0x90];
        text.extend([0x64, 0x4c, 0x8b, 0x0c, 0x25, 0x28, 0, 0, 0]);
        let n = patch_stack_guard(&mut text, 0x1000, 0x2000).unwrap();
        assert_eq!(n, 2);
        let d1 = (0x2000i32 - (0x1000 + 7)).to_le_bytes();
        assert_eq!(
            &text[..9],
            &[0x48, 0x8b, 0x05, d1[0], d1[1], d1[2], d1[3], 0x66, 0x90]
        );
        let d2 = (0x2000i32 - (0x1000 + 10 + 7)).to_le_bytes();
        assert_eq!(
            &text[10..],
            &[0x4c, 0x8b, 0x0d, d2[0], d2[1], d2[2], d2[3], 0x66, 0x90]
        );
    }

    #[test]
    fn stack_guard_leaves_other_fs_reads() {
        let mut text = vec![0x64, 0x48, 0x8b, 0x04, 0x25, 0x30, 0, 0, 0];
        let before = text.clone();
        assert_eq!(patch_stack_guard(&mut text, 0, 0x100).unwrap(), 0);
        assert_eq!(text, before);
    }
}
