//! `strtol`, `strtoul` and `strtod`, as C parses them, for Windows.

fn skip_space(s: &[u8]) -> usize {
    s.iter()
        .take_while(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r'))
        .count()
}

fn digit(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some(u32::from(c - b'0')),
        b'a'..=b'z' => Some(u32::from(c - b'a') + 10),
        b'A'..=b'Z' => Some(u32::from(c - b'A') + 10),
        _ => None,
    }
}

/// What `strtol` and `strtoul` read.
pub(crate) struct Int {
    /// The value, already clamped when it overflowed.
    pub(crate) value: u64,
    /// Bytes consumed, 0 if there was no number.
    pub(crate) used: usize,
    pub(crate) overflow: bool,
}

/// `strtoll` (`signed`) or `strtoull` in `base` (0, or 2 to 36).
pub(crate) fn strtoint(s: &[u8], base: u32, signed: bool) -> Int {
    let none = Int {
        value: 0,
        used: 0,
        overflow: false,
    };
    if base == 1 || base > 36 {
        return none;
    }
    let mut i = skip_space(s);
    let neg = match s.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let hex_prefix = |i: usize| {
        s.get(i) == Some(&b'0')
            && matches!(s.get(i + 1), Some(b'x' | b'X'))
            && s.get(i + 2).and_then(|&c| digit(c)).is_some_and(|d| d < 16)
    };
    let mut base = base;
    if (base == 0 || base == 16) && hex_prefix(i) {
        base = 16;
        i += 2;
    } else if base == 0 {
        base = if s.get(i) == Some(&b'0') { 8 } else { 10 };
    }
    let start = i;
    let mut v: u64 = 0;
    let mut overflow = false;
    while let Some(d) = s.get(i).and_then(|&c| digit(c)).filter(|&d| d < base) {
        match v
            .checked_mul(u64::from(base))
            .and_then(|x| x.checked_add(u64::from(d)))
        {
            Some(x) => v = x,
            None => overflow = true,
        }
        i += 1;
    }
    if i == start {
        return none;
    }
    let value = if signed {
        let limit = if neg { 1u64 << 63 } else { (1u64 << 63) - 1 };
        if overflow || v > limit {
            overflow = true;
            if neg {
                i64::MIN as u64
            } else {
                i64::MAX as u64
            }
        } else if neg {
            v.wrapping_neg()
        } else {
            v
        }
    } else if overflow {
        u64::MAX
    } else if neg {
        v.wrapping_neg()
    } else {
        v
    };
    Int {
        value,
        used: i,
        overflow,
    }
}

/// `strtod`: the value and the bytes consumed (0 if there was no number).
pub(crate) fn strtod(s: &[u8]) -> (f64, usize) {
    let lead = skip_space(s);
    let mut i = lead;
    let neg = match s.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let sign = |v: f64| if neg { -v } else { v };
    let rest = &s[i..];
    let starts = |w: &[u8]| rest.len() >= w.len() && rest[..w.len()].eq_ignore_ascii_case(w);
    if starts(b"infinity") {
        return (sign(f64::INFINITY), i + 8);
    }
    if starts(b"inf") {
        return (sign(f64::INFINITY), i + 3);
    }
    if starts(b"nan") {
        let mut n = i + 3;
        if s.get(n) == Some(&b'(')
            && let Some(k) = s[n..].iter().position(|&c| c == b')')
            && s[n + 1..n + k]
                .iter()
                .all(|c| c.is_ascii_alphanumeric() || *c == b'_')
        {
            n += k + 1;
        }
        return (sign(f64::NAN), n);
    }
    if rest.len() > 2 && rest[0] == b'0' && matches!(rest[1], b'x' | b'X') {
        if let Some((v, n)) = hex_float(&rest[2..]) {
            return (sign(v), i + 2 + n);
        }
        return (sign(0.0), i + 1);
    }
    let mut j = i;
    let digits = |j: &mut usize| {
        let s0 = *j;
        while s.get(*j).is_some_and(u8::is_ascii_digit) {
            *j += 1;
        }
        *j - s0
    };
    let mut n = digits(&mut j);
    if s.get(j) == Some(&b'.') {
        j += 1;
        n += digits(&mut j);
    }
    if n == 0 {
        return (0.0, 0);
    }
    if matches!(s.get(j), Some(b'e' | b'E')) {
        let mut k = j + 1;
        if matches!(s.get(k), Some(b'+' | b'-')) {
            k += 1;
        }
        if digits(&mut k) > 0 {
            j = k;
        }
    }
    let text = std::str::from_utf8(&s[i..j]).unwrap();
    let text = text.strip_suffix('.').unwrap_or(text);
    let v: f64 = text.parse().unwrap_or(0.0);
    (sign(v), j)
}

/// The digits after `0x`, with an optional binary exponent.
fn hex_float(s: &[u8]) -> Option<(f64, usize)> {
    let mut mant: u64 = 0;
    let mut exp: i32 = 0;
    let mut i = 0;
    let mut any = false;
    let mut seen_dot = false;
    while let Some(&c) = s.get(i) {
        if c == b'.' && !seen_dot {
            seen_dot = true;
            i += 1;
            continue;
        }
        let Some(d) = digit(c).filter(|&d| d < 16) else {
            break;
        };
        any = true;
        if mant >> 60 == 0 {
            mant = mant << 4 | u64::from(d);
            if seen_dot {
                exp -= 4;
            }
        } else if !seen_dot {
            exp += 4;
        }
        i += 1;
    }
    if !any {
        return None;
    }
    if matches!(s.get(i), Some(b'p' | b'P')) {
        let r = strtoint(&s[i + 1..], 10, true);
        if r.used > 0 && !matches!(s.get(i + 1), Some(b' ' | b'\t')) {
            exp = exp.saturating_add(r.value as i64 as i32);
            i += 1 + r.used;
        }
    }
    Some((mant as f64 * 2f64.powi(exp), i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ints() {
        let r = strtoint(b"  -42xyz", 10, true);
        assert_eq!((r.value as i64, r.used, r.overflow), (-42, 5, false));
        let r = strtoint(b"0x1F", 0, false);
        assert_eq!((r.value, r.used), (31, 4));
        let r = strtoint(b"0755", 0, false);
        assert_eq!((r.value, r.used), (0o755, 4));
        let r = strtoint(b"99999999999999999999", 10, true);
        assert_eq!((r.value as i64, r.overflow), (i64::MAX, true));
        let r = strtoint(b"-99999999999999999999", 10, true);
        assert_eq!((r.value as i64, r.overflow), (i64::MIN, true));
        let r = strtoint(b"0x", 16, false);
        assert_eq!((r.value, r.used), (0, 1));
        assert_eq!(strtoint(b"zz", 10, false).used, 0);
        assert_eq!(strtoint(b"-1", 10, false).value, u64::MAX);
    }

    #[test]
    fn doubles() {
        assert_eq!(strtod(b" 1.5e3x"), (1500.0, 6));
        assert_eq!(strtod(b"-.25"), (-0.25, 4));
        assert_eq!(strtod(b"7."), (7.0, 2));
        assert_eq!(strtod(b"1e"), (1.0, 1));
        assert_eq!(strtod(b"0x1.8p1"), (3.0, 7));
        assert_eq!(strtod(b"Infinity"), (f64::INFINITY, 8));
        assert_eq!(strtod(b"abc"), (0.0, 0));
        let (v, n) = strtod(b"nan(x1)!");
        assert!(v.is_nan());
        assert_eq!(n, 7);
    }
}
