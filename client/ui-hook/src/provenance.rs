//! Where the client's own registry entries point, as a check of the
//! recovered layout before rekindle writes beside them.
//!
//! When the client calls `Registries::with_builtins`, its registries already
//! hold its own entries, and rekindle reads them through recovered offsets
//! before pushing its own. A wrong offset or stride would have rekindle
//! write into the wrong place. The client's entries say whether the layout
//! is right by where their pointers point:
//!
//! - a provider (`Box<dyn Trait>`): its data pointer is outside the exe's
//!   image (the heap, or dangling for a provider with no fields), and its
//!   vtable is inside it, starting with a drop function in the image, a
//!   size and a power-of-two alignment;
//! - a `String`: `len <= cap`, a short length, a readable buffer outside the
//!   image, and UTF-8 bytes in it.
//!
//! A registry whose entries don't look like that is left to the client.

use rekindle_native::mem;

/// A `String`'s three words, at their recovered offsets.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StringLayout {
    pub(crate) cap: usize,
    pub(crate) ptr: usize,
    pub(crate) len: usize,
}

/// `String`'s word order in this build, which the client shares (same
/// compiler).
pub(crate) fn std_string() -> StringLayout {
    let mut s = String::with_capacity(32);
    s.push('x');
    let (p, l, c) = (s.as_ptr() as usize, s.len(), s.capacity());
    // SAFETY: a `String` is three words; they are only compared, and `s`
    // still owns its buffer.
    let words: [usize; 3] = unsafe { std::mem::transmute_copy(&s) };
    let off = |v: usize| words.iter().position(|w| *w == v).unwrap_or(0) * 8;
    StringLayout {
        cap: off(c),
        ptr: off(p),
        len: off(l),
    }
}

/// The longest name a client entry is expected to carry.
const MAX_NAME: usize = 4096;

/// The exe's image: its base and end (`SizeOfImage` past it).
#[cfg(windows)]
fn image() -> Option<(usize, usize)> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut std::ffi::c_void;
    }
    // SAFETY: a null name asks for the exe's own module; nothing is freed.
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    if base == 0 {
        return None;
    }
    // SAFETY: the loader maps the PE headers at the module base:
    // `e_lfanew` at 0x3c, and in the PE32+ optional header (at +24 from the
    // NT headers) `SizeOfImage` at +56.
    let size = unsafe {
        let nt = base + mem::read::<u32>(base + 0x3c) as usize;
        mem::read::<u32>(nt + 24 + 56) as usize
    };
    Some((base, base + size))
}

/// Not checked off Windows yet: every check passes.
#[cfg(not(windows))]
fn image() -> Option<(usize, usize)> {
    None
}

fn in_image(image: (usize, usize), p: usize) -> bool {
    (image.0..image.1).contains(&p)
}

/// Whether a `dyn` fat pointer the client keeps, `(data, vtable)`, looks
/// like one. Every read is checked first.
pub(crate) fn check_dyn(fat: [usize; 2]) -> Result<(), String> {
    let Some(image) = image() else {
        return Ok(());
    };
    let [data, vtable] = fat;
    if data == 0 {
        return Err("a null data pointer".into());
    }
    if in_image(image, data) && !mem::readable(data, 1) {
        return Err(format!("data pointer {data:#x} isn't readable"));
    }
    if !in_image(image, vtable) || !vtable.is_multiple_of(8) || !mem::readable(vtable, 24) {
        return Err(format!("vtable {vtable:#x} isn't in the client's image"));
    }
    // SAFETY: checked readable just above; plain words.
    let [drop, size, align] = unsafe { mem::read::<[usize; 3]>(vtable) };
    if drop != 0 && !in_image(image, drop) {
        return Err(format!(
            "the vtable's drop function {drop:#x} isn't in the client's image"
        ));
    }
    if size > 1 << 20 || !align.is_power_of_two() || align > 4096 {
        return Err(format!(
            "the vtable's size and alignment ({size}, {align}) aren't a type's"
        ));
    }
    // A provider with fields is on the heap; one without is a dangling,
    // aligned pointer.
    if size > 0 && in_image(image, data) {
        return Err(format!(
            "data pointer {data:#x} is in the image, not on the heap"
        ));
    }
    Ok(())
}

/// Whether the `String` at `at`, with its words where `s` says, looks like
/// one.
///
/// # Safety
///
/// `at` must point at `s.cap.max(s.ptr).max(s.len) + 8` readable bytes.
pub(crate) unsafe fn check_string(at: usize, s: &StringLayout) -> Result<(), String> {
    // SAFETY: the caller's contract.
    let (cap, ptr, len) = unsafe {
        (
            mem::read::<usize>(at + s.cap),
            mem::read::<usize>(at + s.ptr),
            mem::read::<usize>(at + s.len),
        )
    };
    if len > cap || len > MAX_NAME {
        return Err(format!("a String of length {len} and capacity {cap}"));
    }
    if cap == 0 {
        return Ok(());
    }
    if ptr == 0 || image().is_some_and(|i| in_image(i, ptr)) || !mem::readable(ptr, len) {
        return Err(format!(
            "a String buffer at {ptr:#x} that isn't on the heap"
        ));
    }
    // SAFETY: checked readable just above.
    let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, len) };
    std::str::from_utf8(bytes)
        .map(drop)
        .map_err(|_| format!("a String at {ptr:#x} that isn't UTF-8"))
}
#[cfg(test)]
mod tests {
    use super::*;

    trait Provider {
        fn name(&self) -> &str;
    }
    struct Stateless;
    impl Provider for Stateless {
        fn name(&self) -> &'static str {
            "stateless"
        }
    }
    struct Stateful(String);
    impl Provider for Stateful {
        fn name(&self) -> &str {
            &self.0
        }
    }

    fn fat(b: Box<dyn Provider>) -> [usize; 2] {
        // SAFETY: a `Box<dyn _>` is two words; the test turns them back.
        unsafe { std::mem::transmute(b) }
    }

    #[test]
    fn real_providers_pass_and_shifted_words_fail() {
        let (stateless, stateful) = (Stateless, Stateful("x".into()));
        assert_eq!((stateless.name(), stateful.name()), ("stateless", "x"));
        let a = fat(Box::new(stateless));
        let b = fat(Box::new(stateful));
        assert_eq!(check_dyn(a), Ok(()));
        assert_eq!(check_dyn(b), Ok(()));
        // Read one word off: the vtable slot holds a heap pointer, or the
        // data slot the next entry's vtable.
        assert!(check_dyn([b[1], a[0]]).is_err());
        assert!(check_dyn([b[0], b[0]]).is_err());
        assert!(check_dyn([0, b[1]]).is_err());
        // SAFETY: the words `fat` made, turned back once each.
        unsafe {
            drop(std::mem::transmute::<[usize; 2], Box<dyn Provider>>(a));
            drop(std::mem::transmute::<[usize; 2], Box<dyn Provider>>(b));
        }
    }

    #[test]
    fn strings_pass_where_their_words_are() {
        let mut name = String::with_capacity(32);
        name.push_str("Appearance");
        let (p, l, c) = (name.as_ptr() as usize, name.len(), name.capacity());
        // SAFETY: a `String` is three words; turned back below.
        let words: [usize; 3] = unsafe { std::mem::transmute(name) };
        // This std's order, found from the String itself.
        let off = |v: usize| words.iter().position(|w| *w == v).unwrap() * 8;
        let real = StringLayout {
            cap: off(c),
            ptr: off(p),
            len: off(l),
        };
        let at = words.as_ptr() as usize;
        // SAFETY: `at` is the three words, and they are turned back once.
        unsafe {
            assert_eq!(check_string(at, &real), Ok(()));
            // The slots shifted by one: the buffer pointer is read as a length.
            let shifted = StringLayout {
                cap: real.ptr,
                ptr: real.len,
                len: real.cap,
            };
            assert!(check_string(at, &shifted).is_err());
            drop(std::mem::transmute::<[usize; 3], String>(words));
        }
    }
}
