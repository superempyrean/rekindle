//! A masked byte signature and a wildcard-aware scanner.
//!
//! A signature is a run of bytes where some positions are *wildcards*: they
//! stand for bytes that differ between our locally-compiled reference copy of a
//! function and the same function in the target binary. The bytes that differ
//! are precisely the ones the linker patches (call/jump targets and data
//! references), which [`crate::extract`] reads from each object's relocation
//! table. Everything else (opcodes, register encodings, non-relocated
//! immediates) is identical when the same `rustc` and codegen flags produced
//! both, so matching on just those bytes locates the function regardless of
//! where it (or the things it references) landed.
//!
//! The textual form mirrors the convention IDA/sig tools use, so a signature is
//! legible and diffable: `48 8b ?? ?? ?? ?? e8 ?? ?? ?? ??`.

use std::fmt::Write;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// A byte pattern with wildcards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    /// One entry per byte. Wildcard bytes carry `None`; fixed bytes `Some(b)`.
    bytes: Vec<Option<u8>>,
}

impl Signature {
    /// A signature from one entry per byte, `None` for a wildcard.
    pub fn new(bytes: Vec<Option<u8>>) -> Self {
        Self { bytes }
    }

    /// Total length in bytes, wildcards included.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Number of fixed (non-wildcard) bytes. This, not [`len`](Self::len), is
    /// what makes a match meaningful: a signature that is mostly wildcards can
    /// match by chance, so callers gate on this.
    pub fn significant(&self) -> usize {
        self.bytes.iter().filter(|b| b.is_some()).count()
    }

    /// The longest run of consecutive fixed bytes. A scan can anchor on this
    /// run and only then check the wildcards, which keeps scanning cheap, and a
    /// long fixed run is itself a sign the signature is specific.
    pub fn longest_fixed_run(&self) -> usize {
        let mut best = 0;
        let mut cur = 0;
        for b in &self.bytes {
            if b.is_some() {
                cur += 1;
                best = best.max(cur);
            } else {
                cur = 0;
            }
        }
        best
    }

    /// Whether the signature matches the haystack starting exactly at `pos`.
    pub fn matches_at(&self, hay: &[u8], pos: usize) -> bool {
        let Some(end) = pos.checked_add(self.len()) else {
            return false;
        };
        if end > hay.len() {
            return false;
        }
        self.bytes
            .iter()
            .zip(&hay[pos..end])
            .all(|(sig, &got)| sig.is_none_or(|want| want == got))
    }

    /// Every offset in `hay` where the signature matches. Anchors on the first
    /// fixed byte so fully-wildcard prefixes don't cost a full compare at every
    /// position; an all-wildcard signature would match everywhere, so we refuse
    /// to build one (see [`Self::parse`]) and treat an empty one as no matches.
    pub fn scan(&self, hay: &[u8]) -> Vec<usize> {
        let mut out = Vec::new();
        let Some(anchor_off) = self.bytes.iter().position(std::option::Option::is_some) else {
            return out;
        };
        let anchor = self.bytes[anchor_off].expect("position found a Some");
        // The anchor sits `anchor_off` bytes into the signature, so a match that
        // begins at `pos` has its anchor at `pos + anchor_off`; iterate anchor
        // positions and step back.
        let last_start = hay.len().saturating_sub(self.len());
        let mut i = anchor_off;
        while i <= last_start + anchor_off && i < hay.len() {
            if hay[i] == anchor {
                let start = i - anchor_off;
                if self.matches_at(hay, start) {
                    out.push(start);
                }
            }
            i += 1;
        }
        out
    }

    /// The pattern, one entry per byte (`None` = wildcard).
    pub fn bytes(&self) -> &[Option<u8>] {
        &self.bytes
    }

    /// Offsets `o` where bytes `o..o+3` are all fixed (candidate index keys).
    fn trigram_offsets(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.bytes.len().saturating_sub(2)).filter(|&o| {
            self.bytes[o].is_some() && self.bytes[o + 1].is_some() && self.bytes[o + 2].is_some()
        })
    }

    /// Render as space-separated lowercase hex with `??` for wildcards.
    pub fn to_pattern(&self) -> String {
        let mut s = String::with_capacity(self.bytes.len() * 3);
        for (i, b) in self.bytes.iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            match b {
                Some(v) => {
                    write!(s, "{v:02x}").expect("writing to a String can't fail");
                }
                None => s.push_str("??"),
            }
        }
        s
    }

    /// Parse the `48 8b ?? e8 ..` form. Rejects an all-wildcard pattern, which
    /// would match at every offset and means nothing.
    pub fn parse(pattern: &str) -> Result<Self> {
        let mut bytes = Vec::new();
        for tok in pattern.split_whitespace() {
            if tok == "??" || tok == "?" {
                bytes.push(None);
            } else if tok.len() == 2 && tok.bytes().all(|c| c.is_ascii_hexdigit()) {
                bytes.push(Some(u8::from_str_radix(tok, 16)?));
            } else {
                bail!("bad signature token {tok:?}");
            }
        }
        if bytes.is_empty() {
            bail!("empty signature");
        }
        if !bytes.iter().any(std::option::Option::is_some) {
            bail!("signature is all wildcards");
        }
        Ok(Self { bytes })
    }
}

/// Every position of every 3-byte sequence in a haystack, so each signature
/// is checked only where its rarest fixed 3-byte run occurs instead of at
/// every position its first fixed byte does. Built once per haystack; for
/// many signatures over a large binary this turns a full pass per signature
/// into a handful of verifications. About 4 bytes per haystack byte plus a
/// 64 MiB table.
pub struct TrigramIndex<'a> {
    hay: &'a [u8],
    /// `heads[k]..heads[k + 1]` indexes `positions` for trigram key `k`.
    heads: Vec<u32>,
    positions: Vec<u32>,
}

impl<'a> TrigramIndex<'a> {
    fn key(b: &[u8]) -> usize {
        (b[0] as usize) << 16 | (b[1] as usize) << 8 | b[2] as usize
    }

    pub fn new(hay: &'a [u8]) -> Self {
        const KEYS: usize = 1 << 24;
        let n = hay.len().saturating_sub(2);
        let mut heads = vec![0u32; KEYS + 1];
        for i in 0..n {
            heads[Self::key(&hay[i..]) + 1] += 1;
        }
        for k in 0..KEYS {
            heads[k + 1] += heads[k];
        }
        let mut fill = heads.clone();
        let mut positions = vec![0u32; n];
        for i in 0..n {
            let k = Self::key(&hay[i..]);
            positions[fill[k] as usize] = i as u32;
            fill[k] += 1;
        }
        Self {
            hay,
            heads,
            positions,
        }
    }

    fn bucket(&self, key: usize) -> &[u32] {
        &self.positions[self.heads[key] as usize..self.heads[key + 1] as usize]
    }

    /// Same result as `sig.scan(hay)` (sorted offsets).
    pub fn find(&self, sig: &Signature) -> Vec<usize> {
        let best = sig
            .trigram_offsets()
            .map(|o| {
                let k = (sig.bytes[o].unwrap() as usize) << 16
                    | (sig.bytes[o + 1].unwrap() as usize) << 8
                    | sig.bytes[o + 2].unwrap() as usize;
                (self.bucket(k).len(), o, k)
            })
            .min();
        let Some((_, o, k)) = best else {
            return sig.scan(self.hay);
        };
        let mut out: Vec<usize> = self
            .bucket(k)
            .iter()
            .filter_map(|&p| (p as usize).checked_sub(o))
            .filter(|&start| sig.matches_at(self.hay, start))
            .collect();
        out.sort_unstable();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(bytes: &[Option<u8>]) -> Signature {
        Signature::new(bytes.to_vec())
    }

    #[test]
    fn pattern_round_trips() {
        let p = "48 8b ?? ?? ?? ?? e8 ?? ?? ?? ??";
        let s = Signature::parse(p).unwrap();
        assert_eq!(s.to_pattern(), p);
        assert_eq!(s.len(), 11);
        assert_eq!(s.significant(), 3);
        assert_eq!(s.longest_fixed_run(), 2);
    }

    #[test]
    fn rejects_empty_and_all_wildcard() {
        assert!(Signature::parse("").is_err());
        assert!(Signature::parse("?? ??").is_err());
        assert!(Signature::parse("zz").is_err());
        assert!(Signature::parse("8").is_err());
    }

    #[test]
    fn matches_at_respects_wildcards_and_bounds() {
        let s = sig(&[Some(0x90), None, Some(0xc3)]);
        assert!(s.matches_at(&[0x90, 0xff, 0xc3], 0));
        assert!(s.matches_at(&[0x00, 0x90, 0x11, 0xc3], 1));
        assert!(!s.matches_at(&[0x90, 0xff, 0xc4], 0));
        // Runs off the end.
        assert!(!s.matches_at(&[0x90, 0xff], 0));
        assert!(!s.matches_at(&[0x90, 0xff, 0xc3], 1));
    }

    #[test]
    fn scan_finds_all_occurrences() {
        let s = sig(&[Some(0xe8), None, None, None, None]);
        let hay = [0xe8, 1, 2, 3, 4, 0x00, 0xe8, 9, 9, 9, 9];
        assert_eq!(s.scan(&hay), vec![0, 6]);
    }

    #[test]
    fn scan_anchors_past_leading_wildcards() {
        // Leading wildcards must not shift the reported start offset.
        let s = sig(&[None, None, Some(0xc3)]);
        let hay = [0x11, 0x22, 0xc3, 0x33];
        assert_eq!(s.scan(&hay), vec![0]);
        // No false hit when the anchor is too close to the start to fit.
        let hay2 = [0xc3, 0x33];
        assert_eq!(s.scan(&hay2), Vec::<usize>::new());
    }

    #[test]
    fn scan_empty_haystack_is_empty() {
        let s = sig(&[Some(0x90)]);
        assert_eq!(s.scan(&[]), Vec::<usize>::new());
    }
}
