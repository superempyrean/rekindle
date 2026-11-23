//! Anchor sets and binaries the tests share.

use crate::descriptor::{Anchor, AnchorSet, Locator};

pub(crate) fn set() -> AnchorSet {
    AnchorSet {
        target: "test".into(),
        anchors: vec![
            Anchor {
                name: "greeting".into(),
                note: "two locators".into(),
                function: false,
                any_of: false,
                within: None,
                operand: None,
                relative_to: None,
                plus: None,
                source: None,
                locators: vec![
                    Locator::String {
                        text: "hello world".into(),
                    },
                    Locator::String {
                        text: "needle".into(),
                    },
                ],
            },
            Anchor {
                name: "absent".into(),
                note: String::new(),
                function: false,
                any_of: false,
                within: None,
                operand: None,
                relative_to: None,
                plus: None,
                source: None,
                locators: vec![Locator::String {
                    text: "nowhere".into(),
                }],
            },
            Anchor {
                name: "dupe".into(),
                note: String::new(),
                function: false,
                any_of: false,
                within: None,
                operand: None,
                relative_to: None,
                plus: None,
                source: None,
                locators: vec![Locator::String { text: "ab".into() }],
            },
        ],
    }
}

/// A minimal PE32+: one section at RVA 0x1000 holding `code`, then a
/// `.pdata` table for `functions` (RVAs relative to the section start).
pub(crate) fn tiny_pe(code: &[u8], functions: &[(u32, u32)]) -> Vec<u8> {
    let mut section = code.to_vec();
    let pdata = section.len() as u32;
    for &(b, e) in functions {
        for v in [0x1000 + b, 0x1000 + e, 0] {
            section.extend(v.to_le_bytes());
        }
    }
    let mut pe = vec![0u8; 0x200];
    pe[..2].copy_from_slice(b"MZ");
    pe[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
    pe[0x40..0x44].copy_from_slice(b"PE\0\0");
    pe[0x44..0x46].copy_from_slice(&0x8664u16.to_le_bytes());
    pe[0x46..0x48].copy_from_slice(&1u16.to_le_bytes());
    pe[0x54..0x56].copy_from_slice(&240u16.to_le_bytes());
    let opt = 0x58;
    pe[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
    pe[opt + 24..opt + 32].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
    pe[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes());
    pe[opt + 136..opt + 140].copy_from_slice(&(0x1000 + pdata).to_le_bytes());
    pe[opt + 140..opt + 144].copy_from_slice(&(functions.len() as u32 * 12).to_le_bytes());
    let sh = opt + 240;
    pe[sh..sh + 5].copy_from_slice(b".text");
    let size = section.len() as u32;
    for (at, v) in [
        (8, size),
        (12, 0x1000),
        (16, size),
        (20, 0x200),
        (36, 0x6000_0020),
    ] {
        pe[sh + at..sh + at + 4].copy_from_slice(&v.to_le_bytes());
    }
    pe.extend(section);
    pe
}

/// A minimal x86-64 ELF shared object, loaded at VA 0 so VAs equal file
/// offsets, with one name table: `names[k]`'s entry is `{name ptr, name
/// len, handler ptr}` at `0x300 + 24 * k`, its handler a `ret` at
/// `0x100 + 0x10 * k`. The pointers are relative relocations in an
/// Android-packed (`APS2`) section, as `object` leaves them for
/// `rekindle-analysis` to decode.
pub(crate) fn tiny_elf_table(names: &[&str]) -> Vec<u8> {
    const TEXT: usize = 0x100;
    const RODATA: usize = 0x200;
    const TABLE: usize = 0x300;
    const RELOCS: usize = 0x500;
    let mut f = vec![0u8; 0x600];
    // Handlers.
    for k in 0..names.len() {
        f[TEXT + 0x10 * k] = 0xc3;
    }
    // Names back to back, and the table.
    let mut relocs: Vec<(u64, u64)> = Vec::new();
    let mut at = RODATA;
    for (k, n) in names.iter().enumerate() {
        f[at..at + n.len()].copy_from_slice(n.as_bytes());
        let entry = TABLE + 24 * k;
        f[entry + 8..entry + 16].copy_from_slice(&(n.len() as u64).to_le_bytes());
        relocs.push((entry as u64, at as u64));
        relocs.push((entry as u64 + 16, (TEXT + 0x10 * k) as u64));
        at += n.len();
    }
    // APS2: count, start offset, then one group of all relocations, grouped
    // by info (R_X86_64_RELATIVE) and carrying addends, each an offset delta
    // and an addend delta.
    let mut aps = b"APS2".to_vec();
    let sleb = |out: &mut Vec<u8>, mut v: i64| loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        let done = (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0);
        out.push(if done { b } else { b | 0x80 });
        if done {
            break;
        }
    };
    sleb(&mut aps, relocs.len() as i64);
    sleb(&mut aps, 0);
    sleb(&mut aps, relocs.len() as i64);
    sleb(&mut aps, 1 | 8);
    sleb(&mut aps, 8);
    let (mut off, mut addend) = (0i64, 0i64);
    for &(slot, value) in &relocs {
        sleb(&mut aps, slot as i64 - off);
        sleb(&mut aps, value as i64 - addend);
        off = slot as i64;
        addend = value as i64;
    }
    f[RELOCS..RELOCS + aps.len()].copy_from_slice(&aps);
    // Section names, then section headers.
    let shstr = b"\0.text\0.rodata\0.data.rel.ro\0.rela.android\0.shstrtab\0";
    let shstr_at = f.len();
    f.extend_from_slice(shstr);
    let name = |s: &str| {
        shstr
            .windows(s.len() + 1)
            .position(|w| &w[1..] == s.as_bytes() && w[0] == 0)
            .unwrap() as u32
            + 1
    };
    let sections: [(u32, u32, u64, usize, usize); 6] = [
        (0, 0, 0, 0, 0),
        (name(".text"), 1, 6, TEXT, 0x100),
        (name(".rodata"), 1, 2, RODATA, 0x100),
        (name(".data.rel.ro"), 1, 3, TABLE, 0x200),
        (name(".rela.android"), 0x6000_0002, 2, RELOCS, aps.len()),
        (name(".shstrtab"), 3, 0, shstr_at, shstr.len()),
    ];
    while !f.len().is_multiple_of(8) {
        f.push(0);
    }
    let shoff = f.len();
    for &(n, ty, flags, off, size) in &sections {
        let mut sh = [0u8; 64];
        sh[0..4].copy_from_slice(&n.to_le_bytes());
        sh[4..8].copy_from_slice(&ty.to_le_bytes());
        sh[8..16].copy_from_slice(&flags.to_le_bytes());
        sh[16..24].copy_from_slice(&(off as u64).to_le_bytes());
        sh[24..32].copy_from_slice(&(off as u64).to_le_bytes());
        sh[32..40].copy_from_slice(&(size as u64).to_le_bytes());
        sh[48..56].copy_from_slice(&1u64.to_le_bytes());
        f.extend_from_slice(&sh);
    }
    let total = f.len() as u64;
    // ELF header.
    f[0..4].copy_from_slice(b"\x7fELF");
    f[4] = 2; // 64-bit
    f[5] = 1; // little-endian
    f[6] = 1;
    f[16..18].copy_from_slice(&3u16.to_le_bytes()); // ET_DYN
    f[18..20].copy_from_slice(&62u16.to_le_bytes()); // x86-64
    f[20..24].copy_from_slice(&1u32.to_le_bytes());
    f[32..40].copy_from_slice(&0x40u64.to_le_bytes()); // phoff
    f[40..48].copy_from_slice(&(shoff as u64).to_le_bytes());
    f[52..54].copy_from_slice(&64u16.to_le_bytes());
    f[54..56].copy_from_slice(&56u16.to_le_bytes());
    f[56..58].copy_from_slice(&1u16.to_le_bytes());
    f[58..60].copy_from_slice(&64u16.to_le_bytes());
    f[60..62].copy_from_slice(&(sections.len() as u16).to_le_bytes());
    f[62..64].copy_from_slice(&5u16.to_le_bytes());
    // One PT_LOAD over the whole file at VA 0.
    let ph = 0x40;
    f[ph..ph + 4].copy_from_slice(&1u32.to_le_bytes());
    f[ph + 4..ph + 8].copy_from_slice(&7u32.to_le_bytes());
    f[ph + 32..ph + 40].copy_from_slice(&total.to_le_bytes());
    f[ph + 40..ph + 48].copy_from_slice(&total.to_le_bytes());
    f[ph + 48..ph + 56].copy_from_slice(&0x1000u64.to_le_bytes());
    f
}
