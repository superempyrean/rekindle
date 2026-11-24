//! The System V `va_list`, read from Rust, and the naked functions that
//! make one for the library's variadic imports.
//!
//! On `x86_64` System V a `va_list` is a pointer to this record. The first
//! six integer arguments and eight vector arguments arrive in registers,
//! which the callee spills into a 176-byte register save area; the rest are
//! on the stack.

/// `__va_list_tag`.
#[repr(C)]
pub(crate) struct VaList {
    gp_offset: u32,
    fp_offset: u32,
    overflow: *mut u8,
    reg_save: *mut u8,
}

/// The arguments a printf or scanf conversion takes, in order.
#[cfg(any(windows, test))]
pub(crate) trait Args {
    /// An integer or pointer argument, as 64 bits.
    fn int(&mut self) -> u64;
    /// A `double`.
    fn double(&mut self) -> f64;
    /// A `long double`: binary128 on Android `x86_64`, as its bits.
    fn f128(&mut self) -> u128;
}

/// Reads arguments through a System V `va_list` the library passed.
#[cfg(windows)]
pub(crate) struct SysvArgs(pub(crate) *mut VaList);

#[cfg(windows)]
impl Args for SysvArgs {
    fn int(&mut self) -> u64 {
        // SAFETY: the caller of the variadic function passed this va_list,
        // and asks for no more arguments than its format names.
        unsafe {
            let v = &mut *self.0;
            if v.gp_offset < 48 {
                let x =
                    ((v.reg_save as usize + v.gp_offset as usize) as *const u64).read_unaligned();
                v.gp_offset += 8;
                x
            } else {
                let x = v.overflow.cast::<u64>().read_unaligned();
                v.overflow = v.overflow.add(8);
                x
            }
        }
    }

    fn double(&mut self) -> f64 {
        // SAFETY: as in `int`.
        unsafe {
            let v = &mut *self.0;
            if v.fp_offset < 176 {
                let x = v
                    .reg_save
                    .add(v.fp_offset as usize)
                    .cast::<f64>()
                    .read_unaligned();
                v.fp_offset += 16;
                x
            } else {
                let x = v.overflow.cast::<f64>().read_unaligned();
                v.overflow = v.overflow.add(8);
                x
            }
        }
    }

    fn f128(&mut self) -> u128 {
        // SAFETY: as in `int`. A binary128 takes a whole vector register, or
        // 16 aligned bytes on the stack.
        unsafe {
            let v = &mut *self.0;
            if v.fp_offset <= 160 {
                let x = v
                    .reg_save
                    .add(v.fp_offset as usize)
                    .cast::<u128>()
                    .read_unaligned();
                v.fp_offset += 16;
                x
            } else {
                let p = (v.overflow as usize + 15) & !15;
                let x = (p as *const u128).read_unaligned();
                v.overflow = (p + 16) as *mut u8;
                x
            }
        }
    }
}

/// Defines a System V variadic function `$name` that spills its registers
/// into a `va_list` and calls `$imp` with the `$named` fixed arguments and
/// a pointer to that `va_list` after them. `$reg` is the register the
/// pointer goes in: the one for argument number `$named`.
macro_rules! variadic {
    ($name:ident, $named:literal, $reg:literal, $imp:path) => {
        #[unsafe(naked)]
        pub(crate) extern "sysv64" fn $name() {
            core::arch::naked_asm!(
                "push rbp",
                "mov rbp, rsp",
                "sub rsp, 208",
                "mov [rsp], rdi",
                "mov [rsp + 8], rsi",
                "mov [rsp + 16], rdx",
                "mov [rsp + 24], rcx",
                "mov [rsp + 32], r8",
                "mov [rsp + 40], r9",
                "movaps [rsp + 48], xmm0",
                "movaps [rsp + 64], xmm1",
                "movaps [rsp + 80], xmm2",
                "movaps [rsp + 96], xmm3",
                "movaps [rsp + 112], xmm4",
                "movaps [rsp + 128], xmm5",
                "movaps [rsp + 144], xmm6",
                "movaps [rsp + 160], xmm7",
                "mov dword ptr [rsp + 176], {gp}",
                "mov dword ptr [rsp + 180], 48",
                "lea rax, [rbp + 16]",
                "mov [rsp + 184], rax",
                "mov [rsp + 192], rsp",
                concat!("lea ", $reg, ", [rsp + 176]"),
                "call {imp}",
                "leave",
                "ret",
                gp = const $named * 8,
                imp = sym $imp,
            )
        }
    };
}
pub(crate) use variadic;

/// Defines a System V function `$name` that calls `$imp`, which returns a
/// binary128's bits in `rax:rdx`, and returns them in `xmm0`, where a
/// `long double` is returned on Android `x86_64`.
macro_rules! returns_f128 {
    ($name:ident, $imp:path) => {
        #[unsafe(naked)]
        pub(crate) extern "sysv64" fn $name() {
            core::arch::naked_asm!(
                "sub rsp, 8",
                "call {imp}",
                "add rsp, 8",
                "movq xmm0, rax",
                "movq xmm1, rdx",
                "punpcklqdq xmm0, xmm1",
                "ret",
                imp = sym $imp,
            )
        }
    };
}
pub(crate) use returns_f128;

/// An `f64` as binary128 bits.
pub(crate) fn f64_to_f128(v: f64) -> u128 {
    let b = v.to_bits();
    let sign = u128::from(b >> 63) << 127;
    let exp = ((b >> 52) & 0x7ff) as i32;
    let mut mant = b & ((1 << 52) - 1);
    if exp == 0x7ff {
        return sign | (0x7fff << 112) | (u128::from(mant) << 60);
    }
    if exp == 0 {
        if mant == 0 {
            return sign;
        }
        // Subnormal: normalise into binary128's wider exponent.
        let mut e = -1022;
        while mant & (1 << 52) == 0 {
            mant <<= 1;
            e -= 1;
        }
        mant &= (1 << 52) - 1;
        return sign | (((e + 16383) as u128) << 112) | (u128::from(mant) << 60);
    }
    sign | (((exp - 1023 + 16383) as u128) << 112) | (u128::from(mant) << 60)
}

/// Binary128 bits as the nearest `f64`, truncating the extra precision.
#[cfg(any(windows, test))]
pub(crate) fn f128_to_f64(b: u128) -> f64 {
    let sign = ((b >> 127) as u64) << 63;
    let exp = ((b >> 112) & 0x7fff) as i32;
    let mant = ((b >> 60) & ((1 << 52) - 1)) as u64;
    if exp == 0x7fff {
        let nan = b & ((1 << 112) - 1) != 0;
        return f64::from_bits(sign | (0x7ff << 52) | if nan { mant | 1 << 51 } else { 0 });
    }
    let e = exp - 16383 + 1023;
    if exp == 0 || e <= -52 {
        return f64::from_bits(sign);
    }
    if e <= 0 {
        return f64::from_bits(sign | ((1 << 52 | mant) >> (1 - e)));
    }
    if e >= 0x7ff {
        return f64::from_bits(sign | (0x7ff << 52));
    }
    f64::from_bits(sign | ((e as u64) << 52) | mant)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f128_round_trip() {
        for v in [0.0, -0.0, 1.0, -2.5, 1e300, 5e-324, f64::INFINITY, f64::MAX] {
            assert_eq!(f128_to_f64(f64_to_f128(v)).to_bits(), v.to_bits(), "{v}");
        }
        assert!(f128_to_f64(f64_to_f128(f64::NAN)).is_nan());
        // 1.0 is 0x3fff << 112.
        assert_eq!(f64_to_f128(1.0), 0x3fff << 112);
    }
}
