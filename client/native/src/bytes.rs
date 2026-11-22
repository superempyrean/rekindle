//! Values of the client's types as bytes: a buffer rekindle owns for one it
//! builds or receives by value, and writes and reads at recovered offsets.
//! Indexing is checked, so a write past a value panics instead of landing
//! in whatever follows it.

use std::ops::{Deref, DerefMut};

/// Zeroed bytes that rekindle owns, 16-aligned, as any of the client's
/// types may need.
pub(crate) struct Bytes(Vec<u128>);

impl Bytes {
    /// `len` zero bytes (rounded up to a multiple of 16).
    pub(crate) fn zeroed(len: usize) -> Self {
        Bytes(vec![0; len.div_ceil(16)])
    }

    /// A copy of `src`, then zeros up to `len` bytes.
    pub(crate) fn copied(src: &[u8], len: usize) -> Self {
        let mut b = Bytes::zeroed(len.max(src.len()));
        b[..src.len()].copy_from_slice(src);
        b
    }
}

impl Deref for Bytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        // SAFETY: the `u128`s are initialized and have no padding, every
        // byte pattern is a valid `u8`, and the slice covers exactly them.
        unsafe { std::slice::from_raw_parts(self.0.as_ptr().cast::<u8>(), self.0.len() * 16) }
    }
}

impl DerefMut for Bytes {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: as in `deref`; every byte pattern is also a valid `u128`,
        // so any write through the slice leaves the `Vec` valid.
        unsafe {
            std::slice::from_raw_parts_mut(self.0.as_mut_ptr().cast::<u8>(), self.0.len() * 16)
        }
    }
}

/// Writes `bytes` into `buf` at `at`.
pub(crate) fn put(buf: &mut [u8], at: usize, bytes: &[u8]) {
    buf[at..at + bytes.len()].copy_from_slice(bytes);
}

/// Writes the machine word `v` into `buf` at `at`.
pub(crate) fn put_word(buf: &mut [u8], at: usize, v: usize) {
    put(buf, at, &v.to_ne_bytes());
}

/// The machine word in `buf` at `at`.
pub(crate) fn word(buf: &[u8], at: usize) -> usize {
    usize::from_ne_bytes(*buf[at..].first_chunk().expect("a word inside the value"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_aligned_and_zeroed() {
        let b = Bytes::zeroed(20);
        assert_eq!(b.len(), 32);
        assert!(b.iter().all(|&x| x == 0));
        assert!(b.as_ptr().addr().is_multiple_of(16));
        let c = Bytes::copied(&[1, 2, 3], 8);
        assert_eq!(&c[..4], &[1, 2, 3, 0]);
    }

    #[test]
    fn words_go_where_they_are_put() {
        let mut b = Bytes::zeroed(24);
        put_word(&mut b, 9, 0xdead_beef);
        assert_eq!(word(&b, 9), 0xdead_beef);
        assert_eq!(b[8], 0);
    }

    #[test]
    #[should_panic = "out of range"]
    fn a_write_past_the_value_panics() {
        let mut b = Bytes::zeroed(16);
        put_word(&mut b, 12, 1);
    }
}
