//! Reversible x64 inline detours.
//!
//! [`InlineDetour`] decodes the prologue of the function it hooks, copies
//! whole instructions into a trampoline and writes a `jmp rel32` over them.
//! It refuses a prologue it can't move unchanged: one with a RIP-relative
//! operand or any control flow. A wrongly sized copy corrupts the function in
//! ways that only surface much later, so a refusal is the safe failure.
//!
//! [`Installed`] keeps the bytes it replaced and writes them back when it is
//! dropped.
//!
//! Detours run on x64 Windows and Linux, which share everything but the
//! memory operations. On other targets [`Detour::install`] returns an error.

use anyhow::Result;

#[cfg(all(target_arch = "x86_64", any(windows, target_os = "linux")))]
mod mem;
#[cfg(all(target_arch = "x86_64", any(windows, target_os = "linux")))]
mod x64;

#[cfg(all(target_arch = "x86_64", any(windows, target_os = "linux")))]
pub use x64::Installed;

#[cfg(not(all(target_arch = "x86_64", any(windows, target_os = "linux"))))]
pub use unsupported::Installed;

/// Installs a hook at a live address.
pub trait Detour {
    /// Replace the entry of the function at `target` with a jump to `hook`.
    /// The returned [`Installed`] calls the original through its trampoline
    /// and removes the hook when dropped.
    ///
    /// Fails, leaving the function untouched, if its first 5 bytes can't be
    /// moved into a trampoline unchanged or no executable memory is free
    /// within `rel32` reach of it.
    ///
    /// # Safety
    ///
    /// - `target` is the entry of a function in this process, followed by at
    ///   least 24 bytes of readable code, and `hook` is a function with the
    ///   same ABI.
    /// - No thread runs the first 5 bytes of `target` while the hook is
    ///   written or removed.
    /// - The returned [`Installed`] is not unhooked or dropped while a thread
    ///   may still call its trampoline.
    unsafe fn install(&self, target: usize, hook: usize) -> Result<Installed>;
}

/// The x64 inline detour described in the [module docs](self).
#[derive(Debug, Clone, Copy, Default)]
pub struct InlineDetour;

#[cfg(not(all(target_arch = "x86_64", any(windows, target_os = "linux"))))]
mod unsupported {
    use anyhow::{Result, bail};

    use super::{Detour, InlineDetour};

    impl Detour for InlineDetour {
        unsafe fn install(&self, _target: usize, _hook: usize) -> Result<Installed> {
            bail!("inline detours are implemented for x64 Windows and Linux only")
        }
    }

    /// An installed hook. Installs fail on this target, so there is never one.
    #[derive(Debug)]
    pub enum Installed {}

    impl Installed {
        /// The trampoline that calls the original function.
        pub fn trampoline(&self) -> usize {
            match *self {}
        }

        /// Restore the original bytes.
        pub fn unhook(&mut self) -> Result<()> {
            match *self {}
        }
    }

    impl Drop for Installed {
        fn drop(&mut self) {
            match *self {}
        }
    }
}
