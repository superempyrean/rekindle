//! Timers. No capability is needed.
//!
//! A timer calls [`Plugin::on_timer`](crate::Plugin::on_timer) after a
//! delay, once or on an interval. The host owns the clock, allows 16 active
//! timers per plugin with a 50 ms floor, and clears them when the plugin
//! unloads. A timer is never a user gesture.
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! # fn f() -> Result<(), timer::TimerError> {
//! timer::repeat("tick", 90)?; // on_timer("tick") every 90 ms
//! timer::clear("tick");
//! # Ok(())
//! # }
//! ```

use crate::raw::rekindle::plugin::host as raw;
pub use raw::TimerError;

/// Calls `on_timer(id)` once, `delay_ms` from now. Reusing `id` replaces
/// the timer.
pub fn set(id: &str, delay_ms: u64) -> Result<(), TimerError> {
    raw::set_timer(id, delay_ms, false)
}

/// Calls `on_timer(id)` every `delay_ms` until [`clear`]. Reusing `id`
/// replaces the timer.
pub fn repeat(id: &str, delay_ms: u64) -> Result<(), TimerError> {
    raw::set_timer(id, delay_ms, true)
}

/// Cancels a timer.
pub fn clear(id: &str) {
    raw::clear_timer(id);
}
