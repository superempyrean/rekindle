//! The pointers a position-independent ELF fills in at load time.
//!
//! A PIE executable or a shared object stores 0 in each pointer slot of its
//! data and keeps the value in a `R_*_RELATIVE` relocation: at load time the
//! dynamic linker writes base + addend into the slot. To read such a pointer
//! from the file, look up the slot's address here and take the addend.
//!
//! Most ELF files keep these relocations in `.rela.dyn` as plain `Elf64_Rela`
//! records. Android's linker can pack them instead (`--pack-dyn-relocs=android`):
//! the section starts with `APS2` and holds groups of SLEB128 deltas, which
//! `object` doesn't decode. Both forms are read here. RELR (`.relr.dyn`) isn't
//! yet.

use std::collections::HashMap;

use object::elf::{R_AARCH64_RELATIVE, R_X86_64_RELATIVE, RelocationType};
use object::{Object, ObjectSection, RelocationFlags};

/// The address each relative relocation writes, mapped to its addend: the
/// value the pointer at that address holds once loaded at base 0. Empty for
/// a file that isn't ELF.
pub fn relative(obj: &object::File) -> HashMap<u64, u64> {
    let mut out = HashMap::new();
    if obj.format() != object::BinaryFormat::Elf {
        return out;
    }
    if let Some(it) = obj.dynamic_relocations() {
        for (va, r) in it {
            if let RelocationFlags::Elf { r_type } = r.flags()
                && is_relative(r_type)
            {
                out.insert(va, r.addend() as u64);
            }
        }
    }
    for sec in obj.sections() {
        let Ok(data) = sec.data() else { continue };
        if let Some(packed) = data.strip_prefix(b"APS2") {
            unpack_aps2(packed, obj.is_64(), &mut out);
        }
    }
    out
}

fn is_relative(r_type: RelocationType) -> bool {
    r_type == R_AARCH64_RELATIVE || r_type == R_X86_64_RELATIVE
}

const GROUPED_BY_INFO: i64 = 1;
const GROUPED_BY_OFFSET_DELTA: i64 = 2;
const GROUPED_BY_ADDEND: i64 = 4;
const GROUP_HAS_ADDEND: i64 = 8;

/// Decode Android's packed relocations (the bytes after `APS2`), the format
/// bionic's linker reads: a count and a start offset, then groups that share
/// any of the offset delta, `r_info` and addend. Stops at the first malformed
/// number, keeping what it decoded.
fn unpack_aps2(bytes: &[u8], is_64: bool, out: &mut HashMap<u64, u64>) {
    let mut s = Sleb { bytes, pos: 0 };
    let mut next = || s.next();
    let Some(count) = next() else { return };
    let Some(mut offset) = next() else { return };
    let mut info = 0i64;
    let mut addend = 0i64;
    let mut done = 0i64;
    while done < count {
        let (Some(size), Some(flags)) = (next(), next()) else {
            return;
        };
        if size <= 0 {
            return;
        }
        let delta = if flags & GROUPED_BY_OFFSET_DELTA != 0 {
            let Some(d) = next() else { return };
            Some(d)
        } else {
            None
        };
        if flags & GROUPED_BY_INFO != 0 {
            let Some(i) = next() else { return };
            info = i;
        }
        let has_addend = flags & GROUP_HAS_ADDEND != 0;
        if has_addend && flags & GROUPED_BY_ADDEND != 0 {
            let Some(a) = next() else { return };
            addend = addend.wrapping_add(a);
        } else if !has_addend {
            addend = 0;
        }
        for _ in 0..size.min(count - done) {
            let Some(d) = delta.or_else(&mut next) else {
                return;
            };
            offset = offset.wrapping_add(d);
            if flags & GROUPED_BY_INFO == 0 {
                let Some(i) = next() else { return };
                info = i;
            }
            if has_addend && flags & GROUPED_BY_ADDEND == 0 {
                let Some(a) = next() else { return };
                addend = addend.wrapping_add(a);
            }
            let r_type = if is_64 {
                info as u32
            } else {
                info as u32 & 0xff
            };
            if is_relative(RelocationType(r_type)) {
                out.insert(offset as u64, addend as u64);
            }
        }
        done += size;
    }
}

struct Sleb<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Sleb<'_> {
    fn next(&mut self) -> Option<i64> {
        let mut value = 0i64;
        let mut shift = 0u32;
        loop {
            let b = *self.bytes.get(self.pos)?;
            self.pos += 1;
            if shift >= 64 {
                return None;
            }
            value |= i64::from(b & 0x7f) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                if shift < 64 && b & 0x40 != 0 {
                    value |= -1i64 << shift;
                }
                return Some(value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn pack(nums: &[i64]) -> Vec<u8> {
        let mut out = Vec::new();
        for &n in nums {
            sleb(n, &mut out);
        }
        out
    }

    #[test]
    fn sleb_round_trips() {
        for v in [
            0,
            1,
            -1,
            63,
            64,
            -64,
            -65,
            0x1234_5678,
            -0x1234_5678,
            i64::MAX,
            i64::MIN,
        ] {
            let bytes = pack(&[v]);
            assert_eq!(
                Sleb {
                    bytes: &bytes,
                    pos: 0
                }
                .next(),
                Some(v),
                "{v}"
            );
        }
    }

    #[test]
    fn grouped_offset_info_and_addend() {
        let rel = i64::from(R_AARCH64_RELATIVE.0);
        // Three relocations 8 bytes apart from 0x1000, one shared r_info,
        // addends 0x500, 0x510, 0x520 given one by one.
        let flags = GROUPED_BY_OFFSET_DELTA | GROUPED_BY_INFO | GROUP_HAS_ADDEND;
        let bytes = pack(&[3, 0x1000 - 8, 3, flags, 8, rel, 0x500, 0x10, 0x10]);
        let mut out = HashMap::new();
        unpack_aps2(&bytes, true, &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[&0x1000], 0x500);
        assert_eq!(out[&0x1008], 0x510);
        assert_eq!(out[&0x1010], 0x520);
    }

    #[test]
    fn addend_resets_in_a_group_without_one() {
        let rel = i64::from(R_AARCH64_RELATIVE.0);
        let glob_dat = 1025; // R_AARCH64_GLOB_DAT, not relative
        let by_all = GROUPED_BY_OFFSET_DELTA | GROUPED_BY_INFO;
        let bytes = pack(&[
            3,
            0,
            // one relative with a shared addend of 0x40
            1,
            by_all | GROUP_HAS_ADDEND | GROUPED_BY_ADDEND,
            0x10,
            rel,
            0x40,
            // two with no addend: per-relocation offsets and infos
            2,
            0,
            0x10,
            glob_dat,
            0x10,
            rel,
        ]);
        let mut out = HashMap::new();
        unpack_aps2(&bytes, true, &mut out);
        assert_eq!(out.get(&0x10), Some(&0x40));
        assert_eq!(out.get(&0x20), None);
        assert_eq!(out.get(&0x30), Some(&0));
    }

    #[test]
    fn truncated_input_keeps_what_it_decoded() {
        let rel = i64::from(R_AARCH64_RELATIVE.0);
        let flags = GROUPED_BY_OFFSET_DELTA | GROUPED_BY_INFO | GROUP_HAS_ADDEND;
        let mut bytes = pack(&[3, 0, 3, flags, 8, rel, 0x500]);
        bytes.push(0x80); // the second addend never ends
        let mut out = HashMap::new();
        unpack_aps2(&bytes, true, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[&8], 0x500);
    }
}
