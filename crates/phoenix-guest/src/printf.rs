//! bionic's printf, over a System V `va_list`, for Windows, where the
//! library's variadic calls can't reach the C runtime. `long` is 64 bits,
//! `wchar_t` 32, and `long double` binary128.

use std::ffi::{CStr, c_char};

use crate::valist::{Args, f128_to_f64};

#[derive(Clone, Copy, PartialEq)]
enum Len {
    None,
    Hh,
    H,
    L,
    Ll,
    BigL,
}

#[derive(Default)]
struct Spec {
    left: bool,
    plus: bool,
    space: bool,
    alt: bool,
    zero: bool,
    width: usize,
    prec: Option<usize>,
}

/// Formats `fmt` with `args` into `out`. `errno` is the value `%m` prints.
pub(crate) fn format(out: &mut Vec<u8>, fmt: &[u8], args: &mut dyn Args, errno: i32) {
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            let end = fmt[i..]
                .iter()
                .position(|&c| c == b'%')
                .map_or(fmt.len(), |n| i + n);
            out.extend_from_slice(&fmt[i..end]);
            i = end;
            continue;
        }
        let start = i;
        i += 1;
        if fmt.get(i) == Some(&b'%') {
            out.push(b'%');
            i += 1;
            continue;
        }
        let mut s = Spec::default();
        while let Some(&c) = fmt.get(i) {
            match c {
                b'-' => s.left = true,
                b'+' => s.plus = true,
                b' ' => s.space = true,
                b'#' => s.alt = true,
                b'0' => s.zero = true,
                b'\'' => {}
                _ => break,
            }
            i += 1;
        }
        if fmt.get(i) == Some(&b'*') {
            let w = args.int() as i32;
            if w < 0 {
                s.left = true;
            }
            s.width = w.unsigned_abs() as usize;
            i += 1;
        } else {
            while let Some(d) = fmt.get(i).filter(|c| c.is_ascii_digit()) {
                s.width = s.width * 10 + usize::from(d - b'0');
                i += 1;
            }
        }
        if fmt.get(i) == Some(&b'.') {
            i += 1;
            if fmt.get(i) == Some(&b'*') {
                let p = args.int() as i32;
                s.prec = usize::try_from(p).ok();
                i += 1;
            } else {
                let mut p = 0;
                while let Some(d) = fmt.get(i).filter(|c| c.is_ascii_digit()) {
                    p = p * 10 + usize::from(d - b'0');
                    i += 1;
                }
                s.prec = Some(p);
            }
        }
        let mut len = Len::None;
        while let Some(&c) = fmt.get(i) {
            len = match (c, len) {
                (b'h', Len::H) => Len::Hh,
                (b'h', _) => Len::H,
                (b'l', Len::L) | (b'j' | b'q' | b'z' | b't', _) => Len::Ll,
                (b'l', _) => Len::L,
                (b'L', _) => Len::BigL,
                _ => break,
            };
            i += 1;
        }
        let Some(&conv) = fmt.get(i) else {
            out.extend_from_slice(&fmt[start..]);
            break;
        };
        i += 1;
        convert(out, conv, &s, len, args, errno, &fmt[start..i]);
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one arm per conversion, which reads best as one match"
)]
fn convert(
    out: &mut Vec<u8>,
    conv: u8,
    s: &Spec,
    len: Len,
    args: &mut dyn Args,
    errno: i32,
    raw: &[u8],
) {
    match conv {
        b'd' | b'i' => {
            let v = args.int();
            let v = match len {
                Len::Hh => i64::from(v as i8),
                Len::H => i64::from(v as i16),
                Len::None | Len::BigL => i64::from(v as i32),
                Len::L | Len::Ll => v as i64,
            };
            let sign = if v < 0 {
                "-"
            } else if s.plus {
                "+"
            } else if s.space {
                " "
            } else {
                ""
            };
            integer(out, s, sign, "", v.unsigned_abs().to_string().into_bytes());
        }
        b'o' | b'u' | b'x' | b'X' => {
            let v = args.int();
            let v = match len {
                Len::Hh => u64::from(v as u8),
                Len::H => u64::from(v as u16),
                Len::None | Len::BigL => u64::from(v as u32),
                Len::L | Len::Ll => v,
            };
            let mut digits = match conv {
                b'o' => format!("{v:o}"),
                b'u' => v.to_string(),
                b'x' => format!("{v:x}"),
                _ => format!("{v:X}"),
            }
            .into_bytes();
            let mut prefix = "";
            if s.alt && conv == b'o' && digits.first() != Some(&b'0') {
                digits.insert(0, b'0');
            }
            if s.alt && v != 0 && conv == b'x' {
                prefix = "0x";
            }
            if s.alt && v != 0 && conv == b'X' {
                prefix = "0X";
            }
            if s.alt && conv == b'o' && s.prec == Some(0) && v == 0 {
                // "%#.0o" of 0 still prints the 0.
                return pad(out, s, b"", b"0", false);
            }
            integer(out, s, "", prefix, digits);
        }
        b'c' => {
            let v = args.int() as u32;
            let mut b = Vec::new();
            if len == Len::L {
                utf8_push(&mut b, v);
            } else {
                b.push(v as u8);
            }
            pad(out, s, b"", &b, false);
        }
        b's' => {
            let p = args.int() as usize as *const u8;
            let mut b = Vec::new();
            if p.is_null() {
                b.extend_from_slice(b"(null)");
            } else if len == Len::L {
                // SAFETY: a NUL-terminated wide string, as %ls takes.
                unsafe {
                    let mut w = p as usize;
                    while (w as *const u32).read_unaligned() != 0 {
                        utf8_push(&mut b, (w as *const u32).read_unaligned());
                        w += 4;
                    }
                }
            } else {
                // SAFETY: a NUL-terminated string, as %s takes. A precision
                // bounds the read, as in C.
                unsafe {
                    match s.prec {
                        Some(n) => {
                            let mut k = 0;
                            while k < n && *p.add(k) != 0 {
                                k += 1;
                            }
                            b.extend_from_slice(std::slice::from_raw_parts(p, k));
                        }
                        None => b.extend_from_slice(CStr::from_ptr(p.cast::<c_char>()).to_bytes()),
                    }
                }
            }
            if let Some(n) = s.prec {
                b.truncate(n);
            }
            pad(out, s, b"", &b, false);
        }
        b'p' => {
            let v = args.int();
            pad(out, s, b"0x", format!("{v:x}").as_bytes(), false);
        }
        b'f' | b'F' | b'e' | b'E' | b'g' | b'G' | b'a' | b'A' => {
            let v = if len == Len::BigL {
                f128_to_f64(args.f128())
            } else {
                args.double()
            };
            float(out, s, conv, v);
        }
        b'n' => {
            let p = args.int() as usize;
            let n = out.len();
            // SAFETY: %n takes a pointer to an integer of the given length.
            unsafe {
                match len {
                    Len::Hh => *(p as *mut i8) = n as i8,
                    Len::H => *(p as *mut i16) = n as i16,
                    Len::L | Len::Ll => *(p as *mut i64) = n as i64,
                    Len::None | Len::BigL => *(p as *mut i32) = n as i32,
                }
            }
        }
        b'm' => out.extend_from_slice(crate::errno::message(errno).as_bytes()),
        _ => out.extend_from_slice(raw),
    }
}

fn integer(out: &mut Vec<u8>, s: &Spec, sign: &str, prefix: &str, mut digits: Vec<u8>) {
    if let Some(p) = s.prec {
        if p == 0 && digits == b"0" {
            digits.clear();
        }
        while digits.len() < p {
            digits.insert(0, b'0');
        }
    }
    let lead = [sign.as_bytes(), prefix.as_bytes()].concat();
    pad(out, s, &lead, &digits, s.prec.is_none());
}

/// Writes `lead` and `body` padded to the width. Zero padding goes between
/// them, when the `0` flag is set and `zero_ok`.
fn pad(out: &mut Vec<u8>, s: &Spec, lead: &[u8], body: &[u8], zero_ok: bool) {
    let n = lead.len() + body.len();
    let fill = s.width.saturating_sub(n);
    if s.left {
        out.extend_from_slice(lead);
        out.extend_from_slice(body);
        out.resize(out.len() + fill, b' ');
    } else if s.zero && zero_ok {
        out.extend_from_slice(lead);
        out.resize(out.len() + fill, b'0');
        out.extend_from_slice(body);
    } else {
        out.resize(out.len() + fill, b' ');
        out.extend_from_slice(lead);
        out.extend_from_slice(body);
    }
}

fn float(out: &mut Vec<u8>, s: &Spec, conv: u8, v: f64) {
    let upper = conv.is_ascii_uppercase();
    let sign = if v.is_sign_negative() && !v.is_nan() {
        "-"
    } else if s.plus {
        "+"
    } else if s.space {
        " "
    } else {
        ""
    };
    let a = v.abs();
    if !a.is_finite() {
        let t = match (a.is_nan(), upper) {
            (true, false) => "nan",
            (true, true) => "NAN",
            (false, false) => "inf",
            (false, true) => "INF",
        };
        return pad(out, s, sign.as_bytes(), t.as_bytes(), false);
    }
    let mut body = match conv.to_ascii_lowercase() {
        b'f' => fixed(a, s.prec.unwrap_or(6), s.alt),
        b'e' => exp(a, s.prec.unwrap_or(6), s.alt),
        b'g' => general(a, s.prec.unwrap_or(6), s.alt),
        _ => hex(a, s.prec),
    };
    let mut lead = sign.to_string();
    if conv.eq_ignore_ascii_case(&b'a') {
        lead.push_str("0x");
    }
    if upper {
        body.make_ascii_uppercase();
    }
    let lead = if upper {
        lead.to_ascii_uppercase()
    } else {
        lead
    };
    pad(out, s, lead.as_bytes(), body.as_bytes(), true);
}

fn fixed(a: f64, p: usize, alt: bool) -> String {
    let mut t = format!("{a:.p$}");
    if alt && p == 0 {
        t.push('.');
    }
    t
}

/// `%e`: the mantissa and an exponent of at least two digits.
fn exp(a: f64, p: usize, alt: bool) -> String {
    let (m, e) = split_exp(a, p);
    let mut t = m;
    if alt && p == 0 {
        t.push('.');
    }
    let sign = if e < 0 { '-' } else { '+' };
    format!("{t}e{sign}{:02}", e.unsigned_abs())
}

fn split_exp(a: f64, p: usize) -> (String, i32) {
    let t = format!("{a:.p$e}");
    let (m, e) = t.split_once('e').unwrap();
    (m.to_string(), e.parse().unwrap())
}

fn general(a: f64, p: usize, alt: bool) -> String {
    let p = p.max(1);
    let x = if a == 0.0 { 0 } else { split_exp(a, p - 1).1 };
    let mut t = if i64::from(x) < p as i64 && x >= -4 {
        fixed(a, (p as i64 - 1 - i64::from(x)) as usize, false)
    } else {
        exp(a, p - 1, false)
    };
    if alt {
        if !t.contains('.') {
            match t.find('e') {
                Some(e) => t.insert(e, '.'),
                None => t.push('.'),
            }
        }
        return t;
    }
    let (m, e) = match t.find('e') {
        Some(k) => (t[..k].to_string(), t[k..].to_string()),
        None => (t.clone(), String::new()),
    };
    let m = if m.contains('.') {
        m.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        m
    };
    t = m + &e;
    t
}

/// `%a` without the `0x`: `1.8p+1`.
fn hex(a: f64, prec: Option<usize>) -> String {
    let bits = a.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    let mut mant = bits & ((1 << 52) - 1);
    let (mut lead, e) = match (exp, mant) {
        (0, 0) => (0u64, 0),
        (0, _) => (0, -1022),
        _ => (1, exp - 1023),
    };
    let mut digits = 13;
    if let Some(p) = prec.filter(|&p| p < 13) {
        let drop = 4 * (13 - p) as u32;
        let rem = mant & ((1 << drop) - 1);
        let half = 1 << (drop - 1);
        mant >>= drop;
        if rem > half || (rem == half && mant & 1 == 1) {
            mant += 1;
            if mant >> (4 * p) != 0 {
                mant = 0;
                lead += 1;
            }
        }
        digits = p;
    }
    let mut frac = if digits == 0 {
        String::new()
    } else {
        format!("{mant:0digits$x}")
    };
    match prec {
        None => frac = frac.trim_end_matches('0').to_string(),
        Some(p) => {
            while frac.len() < p {
                frac.push('0');
            }
        }
    }
    let sign = if e < 0 { '-' } else { '+' };
    let e = e.unsigned_abs();
    if frac.is_empty() {
        format!("{lead}p{sign}{e}")
    } else {
        format!("{lead}.{frac}p{sign}{e}")
    }
}

pub(crate) fn utf8_push(out: &mut Vec<u8>, c: u32) {
    match c {
        0..0x80 => out.push(c as u8),
        0x80..0x800 => out.extend([0xc0 | (c >> 6) as u8, 0x80 | (c & 63) as u8]),
        0x800..0x10000 => out.extend([
            0xe0 | (c >> 12) as u8,
            0x80 | ((c >> 6) & 63) as u8,
            0x80 | (c & 63) as u8,
        ]),
        _ => out.extend([
            0xf0 | (c >> 18) as u8,
            0x80 | ((c >> 12) & 63) as u8,
            0x80 | ((c >> 6) & 63) as u8,
            0x80 | (c & 63) as u8,
        ]),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Arguments from a list, for tests.
    pub(crate) enum A {
        I(u64),
        D(f64),
    }
    pub(crate) struct List(pub(crate) Vec<A>);
    impl Args for List {
        fn int(&mut self) -> u64 {
            match self.0.remove(0) {
                A::I(v) => v,
                A::D(_) => panic!("an int was asked for"),
            }
        }
        fn double(&mut self) -> f64 {
            match self.0.remove(0) {
                A::D(v) => v,
                A::I(_) => panic!("a double was asked for"),
            }
        }
        fn f128(&mut self) -> u128 {
            crate::valist::f64_to_f128(self.double())
        }
    }

    fn f(fmt: &str, args: Vec<A>) -> String {
        let mut out = Vec::new();
        format(&mut out, fmt.as_bytes(), &mut List(args), 2);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn integers() {
        use A::I;
        assert_eq!(
            f(
                "%d|%5d|%-5d|%05d",
                vec![I(-3i64 as u64), I(42), I(42), I(-42i64 as u64)]
            ),
            "-3|   42|42   |-0042"
        );
        assert_eq!(
            f("%ld %lu %llx", vec![I(-1i64 as u64), I(u64::MAX), I(0xabc)]),
            "-1 18446744073709551615 abc"
        );
        assert_eq!(
            f("%x %X %#x %#o %o", vec![I(255), I(255), I(255), I(8), I(0)]),
            "ff FF 0xff 010 0"
        );
        assert_eq!(
            f("%.3d|%+d|% d|%.0d|", vec![I(7), I(7), I(7), I(0)]),
            "007|+7| 7||"
        );
        assert_eq!(
            f("%hhd %hd %zu", vec![I(0x1ff), I(0x1ffff), I(5)]),
            "-1 -1 5"
        );
        assert_eq!(f("%*d|%-*d|", vec![I(4), I(1), I(3), I(2)]), "   1|2  |");
    }

    #[test]
    fn strings() {
        use A::I;
        let s = c"hello";
        let p = s.as_ptr() as u64;
        assert_eq!(
            f("[%s] [%.3s] [%8s] [%-7s]", vec![I(p), I(p), I(p), I(p)]),
            "[hello] [hel] [   hello] [hello  ]"
        );
        assert_eq!(
            f(
                "%s %c%c %p %%",
                vec![I(0), I(u64::from(b'o')), I(u64::from(b'k')), I(0x1234)]
            ),
            "(null) ok 0x1234 %"
        );
        let w: [u32; 3] = [0x48, 0xe9, 0];
        assert_eq!(
            f("%ls %m", vec![I(w.as_ptr() as u64)]),
            "H\u{e9} No such file or directory"
        );
    }

    #[test]
    fn floats() {
        use A::D;
        assert_eq!(
            f(
                "%f %.2f %e %.0e",
                vec![D(1.5), D(2.345), D(1234.5), D(5e-7)]
            ),
            "1.500000 2.35 1.234500e+03 5e-07"
        );
        assert_eq!(
            f(
                "%g %g %g %g %.3g",
                vec![D(0.0001), D(100000.0), D(1e6), D(1.5e-5), D(1234.0)]
            ),
            "0.0001 100000 1e+06 1.5e-05 1.23e+03"
        );
        assert_eq!(
            f("%08.3f|%-8.1f|%+.1f", vec![D(-3.14158), D(2.0), D(2.0)]),
            "-003.142|2.0     |+2.0"
        );
        assert_eq!(
            f("%f %F %g", vec![D(f64::INFINITY), D(f64::NAN), D(-0.0)]),
            "inf NAN -0"
        );
        assert_eq!(
            f("%a %a %.1a %A", vec![D(1.0), D(3.0), D(1.0), D(0.5)]),
            "0x1p+0 0x1.8p+1 0x1.0p+0 0X1P-1"
        );
        assert_eq!(f("%#.0f %#g", vec![D(3.0), D(2.0)]), "3. 2.00000");
    }
}
