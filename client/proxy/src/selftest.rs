//! Headless self-test for the `dwmapi.dll` proxy shim.
//!
//! No client, no window: it loads the built proxy into *this* process and
//! checks the two things the shim must get right:
//!
//!  1. every export the client imports is present and forwards to the genuine
//!     system dwmapi (same HRESULT for the same call), and
//!  2. on load the shim kicks the host: we point it at a stand-in host (the
//!     proxy itself) via `REKINDLE_PROXY_*` and confirm the kick fired.
//!
//! Run with no arguments (finds `dwmapi.dll` beside this binary) or pass the
//! path to the built proxy.

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
include!("selftest_windows.rs");
