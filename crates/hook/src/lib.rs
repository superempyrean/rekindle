//! Hooking a client from inside its process: the attach gate, and reversible
//! x64 inline detours.
//!
//! - [`guard`] decides whether rekindle may attach to an image at all, by its
//!   SHA-256. An empty allowlist refuses everything.
//! - [`hook`] replaces a function's entry with a jump to a hook, and puts it
//!   back when the [`Installed`] hook is dropped.
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

pub mod guard;
pub mod hook;

pub use guard::{AllowList, ImageFingerprint};
pub use hook::{Detour, InlineDetour, Installed};
