//! Hooking a client from inside its process: the attach gate, reversible x64
//! inline detours, and reversible pointer swaps in vtable and GOT slots.
//!
//! - [`guard`] decides whether rekindle may attach to an image at all, by its
//!   SHA-256. An empty allowlist refuses everything.
//! - [`hook`] replaces a function's entry with a jump to a hook, and puts it
//!   back when the [`Installed`] hook is dropped.
//! - [`slot`] replaces the pointer in a vtable or GOT slot, which needs no
//!   code patched and works on any architecture, and puts it back when the
//!   [`SlotSwap`] is dropped. [`elf`] finds an import's GOT slots.
//!
//! The host finds the functions it hooks with `rekindle-anchor`.
//!
//! ```no_run
//! use rekindle_hook::{Detour, InlineDetour, guard};
//!
//! extern "C" fn hooked(value: u64) -> u64 {
//!     value + 1
//! }
//!
//! # fn run(client: &std::path::Path, target: usize) -> anyhow::Result<()> {
//! guard::verify(client, &guard::AllowList::from_env())?;
//! // SAFETY: `target` is the entry of a function with `hooked`'s ABI.
//! let installed = unsafe { InlineDetour.install(target, hooked as *const () as usize)? };
//! // SAFETY: the trampoline runs the original function, of the same type.
//! let original: extern "C" fn(u64) -> u64 =
//!     unsafe { std::mem::transmute(installed.trampoline()) };
//! # Ok(()) }
//! ```

pub mod elf;
pub mod guard;
pub mod hook;
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
pub mod slot;

pub use guard::{AllowList, ImageFingerprint};
pub use hook::{Detour, InlineDetour, Installed};
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
pub use slot::SlotSwap;
