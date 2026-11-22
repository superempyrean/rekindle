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
