//! Dates and times without a wall clock: the host formats them in the
//! user's locale and timezone, and the plugin never learns either.
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! # use rekindle_sdk::time::TimeStyle;
//! # fn f(joined_ms: u64, user_id: u64) {
//! let joined = time::format(joined_ms, TimeStyle::Relative); // "3 years ago"
//! let created = time::snowflake_ms(user_id);
//! # }
//! ```

use crate::raw::rekindle::plugin::host as raw;
pub use raw::TimeStyle;

/// When a snowflake (a user, message or guild id, ...) was created, in
/// milliseconds since the Unix epoch.
pub fn snowflake_ms(id: u64) -> u64 {
    raw::snowflake_ms(id)
}

/// Formats an epoch-milliseconds time in one of Discord's timestamp styles.
pub fn format(epoch_ms: u64, style: TimeStyle) -> String {
    raw::format_time(epoch_ms, style)
}

/// Formats a duration: "0:42", "1:02:03", "3 days".
pub fn duration(millis: u64) -> String {
    raw::format_duration(millis)
}
