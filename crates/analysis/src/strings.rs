//! Extract printable ASCII runs from a byte buffer.
//!
//! A located string carries its file offset so later stages can report where a
//! match came from (useful when turning a find into a durable anchor).

/// A printable run found in the binary.
#[derive(Debug, Clone)]
pub struct Located {
    /// Byte offset of the first character in the file.
    pub offset: usize,
    /// The decoded run (ASCII, no interior NULs).
    pub text: String,
}

/// Scan `bytes` for printable ASCII runs of at least `min_len` characters.
///
/// We treat tab as printable but stop runs at any other control byte or at
/// 0x7f+. Rust's embedded paths and panic messages are plain ASCII, so this is
/// sufficient and avoids UTF-8 guesswork.
pub fn extract(bytes: &[u8], min_len: usize) -> Vec<Located> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut run_len = 0usize;

    let flush = |out: &mut Vec<Located>, start: usize, run: &[u8]| {
        if run.len() >= min_len {
            // Printable ASCII is UTF-8, so nothing is lost.
            out.push(Located {
                offset: start,
                text: String::from_utf8_lossy(run).into_owned(),
            });
        }
    };

    for (i, &b) in bytes.iter().enumerate() {
        let printable = b == b'\t' || (0x20..0x7f).contains(&b);
        if printable {
            if run_len == 0 {
                start = i;
            }
            run_len += 1;
        } else if run_len > 0 {
            flush(&mut out, start, &bytes[start..i]);
            run_len = 0;
        }
    }
    if run_len > 0 {
        flush(&mut out, start, &bytes[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_runs_and_offsets() {
        let data = b"\x00\x01hello\x00wo\x02rldxyz";
        let got = extract(data, 3);
        let texts: Vec<_> = got.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["hello", "rldxyz"]);
        // "hello" starts at index 2.
        assert_eq!(got[0].offset, 2);
    }

    #[test]
    fn respects_min_len() {
        let data = b"ab\x00abcde\x00xy";
        let got = extract(data, 4);
        let texts: Vec<_> = got.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["abcde"]);
    }
}
