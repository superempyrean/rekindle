//! bionic's `sscanf`, over a System V `va_list`, for Windows. Wide and
//! `long double` conversions are refused.

use crate::num::{strtod, strtoint};
use crate::valist::Args;

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

#[derive(Clone, Copy, PartialEq)]
enum Len {
    None,
    Hh,
    H,
    L,
    Ll,
    BigL,
}

/// Scans `input` by `fmt`. Returns the number of conversions assigned, -1
/// if the input ended before the first one, or `None` for a format it
/// refuses.
#[expect(
    clippy::too_many_lines,
    reason = "one pass over the format, which reads best whole"
)]
pub(crate) fn scan(input: &[u8], fmt: &[u8], args: &mut dyn Args) -> Option<i32> {
    let mut at = 0;
    let mut assigned = 0;
    let mut f = 0;
    let eof = |at: usize, assigned: i32| {
        if at >= input.len() && assigned == 0 {
            -1
        } else {
            assigned
        }
    };
    while f < fmt.len() {
        let c = fmt[f];
        if is_space(c) {
            while f < fmt.len() && is_space(fmt[f]) {
                f += 1;
            }
            while at < input.len() && is_space(input[at]) {
                at += 1;
            }
            continue;
        }
        if c != b'%' || fmt.get(f + 1) == Some(&b'%') {
            if c == b'%' {
                f += 1;
                while at < input.len() && is_space(input[at]) {
                    at += 1;
                }
            }
            if input.get(at) != Some(&fmt[f]) {
                return Some(eof(at, assigned));
            }
            at += 1;
            f += 1;
            continue;
        }
        f += 1;
        let suppress = fmt.get(f) == Some(&b'*');
        if suppress {
            f += 1;
        }
        let mut width = 0usize;
        while let Some(d) = fmt.get(f).filter(|c| c.is_ascii_digit()) {
            width = width * 10 + usize::from(d - b'0');
            f += 1;
        }
        let mut len = Len::None;
        while let Some(&c) = fmt.get(f) {
            len = match (c, len) {
                (b'h', Len::H) => Len::Hh,
                (b'h', _) => Len::H,
                (b'l', Len::L) | (b'j' | b'q' | b'z' | b't', _) => Len::Ll,
                (b'l', _) => Len::L,
                (b'L', _) => Len::BigL,
                _ => break,
            };
            f += 1;
        }
        let conv = *fmt.get(f)?;
        f += 1;
        if !matches!(conv, b'[' | b'c' | b'n') {
            while at < input.len() && is_space(input[at]) {
                at += 1;
            }
        }
        let rest = &input[at..];
        let field = if width > 0 {
            &rest[..width.min(rest.len())]
        } else {
            rest
        };
        match conv {
            b'd' | b'i' | b'o' | b'u' | b'x' | b'X' | b'p' => {
                let base = match conv {
                    b'd' | b'u' => 10,
                    b'i' => 0,
                    b'o' => 8,
                    _ => 16,
                };
                let r = strtoint(field, base, conv == b'd' || conv == b'i');
                if r.used == 0 {
                    return Some(eof(at, assigned));
                }
                at += r.used;
                if !suppress {
                    let p = args.int() as usize;
                    let len = if conv == b'p' { Len::Ll } else { len };
                    // SAFETY: the conversion takes a pointer to an integer
                    // of its length.
                    unsafe { store_int(p, len, r.value) };
                    assigned += 1;
                }
            }
            b'a' | b'A' | b'e' | b'E' | b'f' | b'F' | b'g' | b'G' => {
                if len == Len::BigL {
                    return None;
                }
                let (v, n) = strtod(field);
                if n == 0 {
                    return Some(eof(at, assigned));
                }
                at += n;
                if !suppress {
                    let p = args.int() as usize;
                    // SAFETY: a pointer to a float, or a double with `l`.
                    unsafe {
                        if len == Len::L {
                            *(p as *mut f64) = v;
                        } else {
                            *(p as *mut f32) = v as f32;
                        }
                    }
                    assigned += 1;
                }
            }
            b's' | b'c' | b'[' => {
                if len == Len::L {
                    return None;
                }
                let n = match conv {
                    b's' => field.iter().take_while(|&&c| !is_space(c)).count(),
                    b'c' => {
                        let want = width.max(1);
                        if rest.len() < want {
                            return Some(eof(at, assigned));
                        }
                        want
                    }
                    _ => {
                        let (set, used) = scanset(&fmt[f..])?;
                        f += used;
                        field.iter().take_while(|&&c| set[usize::from(c)]).count()
                    }
                };
                if n == 0 {
                    return Some(eof(at, assigned));
                }
                if !suppress {
                    let p = args.int() as usize as *mut u8;
                    // SAFETY: a buffer the format sizes, with room for the
                    // terminator except for %c.
                    unsafe {
                        std::ptr::copy_nonoverlapping(input[at..].as_ptr(), p, n);
                        if conv != b'c' {
                            *p.add(n) = 0;
                        }
                    }
                    assigned += 1;
                }
                at += n;
            }
            b'n' => {
                if !suppress {
                    let p = args.int() as usize;
                    // SAFETY: as for the integer conversions.
                    unsafe { store_int(p, len, at as u64) };
                }
            }
            _ => return None,
        }
    }
    Some(assigned)
}

unsafe fn store_int(p: usize, len: Len, v: u64) {
    // SAFETY: the caller's pointer is to an integer of `len`'s size.
    unsafe {
        match len {
            Len::Hh => *(p as *mut u8) = v as u8,
            Len::H => *(p as *mut u16) = v as u16,
            Len::None | Len::BigL => *(p as *mut u32) = v as u32,
            Len::L | Len::Ll => *(p as *mut u64) = v,
        }
    }
}

/// The set a `%[` conversion matches, and how many format bytes after the
/// `[` it took.
fn scanset(fmt: &[u8]) -> Option<([bool; 256], usize)> {
    let mut i = 0;
    let negate = fmt.first() == Some(&b'^');
    if negate {
        i += 1;
    }
    let mut set = [false; 256];
    let mut first = true;
    loop {
        let &c = fmt.get(i)?;
        if c == b']' && !first {
            i += 1;
            break;
        }
        first = false;
        if fmt.get(i + 1) == Some(&b'-') && fmt.get(i + 2).is_some_and(|&e| e != b']') {
            for b in c..=fmt[i + 2] {
                set[usize::from(b)] = true;
            }
            i += 3;
        } else {
            set[usize::from(c)] = true;
            i += 1;
        }
    }
    if negate {
        for b in &mut set {
            *b = !*b;
        }
    }
    Some((set, i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printf::tests::{A, List};

    #[test]
    fn conversions() {
        let (mut a, mut b, mut n) = (0i32, 0u64, 0i32);
        let mut s = [0u8; 16];
        let mut d = 0f64;
        let args = vec![
            A::I(&raw mut a as u64),
            A::I(&raw mut b as u64),
            A::I(s.as_mut_ptr() as u64),
            A::I(&raw mut d as u64),
            A::I(&raw mut n as u64),
        ];
        let r = scan(
            b"  12 ff name 2.5 tail",
            b"%d %lx %15s %lf%n",
            &mut List(args),
        );
        assert_eq!(r, Some(4));
        assert_eq!((a, b, &s[..5], d, n), (12, 0xff, &b"name\0"[..], 2.5, 16));
    }

    #[test]
    fn sets_literals_and_eof() {
        let mut s = [0u8; 8];
        let mut v = 0u32;
        let args = vec![A::I(s.as_mut_ptr() as u64), A::I(&raw mut v as u64)];
        assert_eq!(scan(b"abc:7", b"%[a-c]:%u", &mut List(args)), Some(2));
        assert_eq!((&s[..4], v), (&b"abc\0"[..], 7));
        assert_eq!(scan(b"", b"%d", &mut List(vec![])), Some(-1));
        assert_eq!(scan(b"x", b"%d", &mut List(vec![])), Some(0));
        assert_eq!(scan(b"1", b"%Lf", &mut List(vec![])), None);
    }
}
