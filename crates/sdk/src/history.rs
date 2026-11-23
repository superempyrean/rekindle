//! A channel's history past what the client has loaded (`history`, and
//! each call needs `read-messages`). Pages hold the same message records as
//! [`crate::messages`].
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! # let channel = 1;
//! let mut before = None;
//! while let Ok(page) = history::before(channel, before, 500) {
//!     // ...
//!     match page.cursor {
//!         Some(c) => before = Some(c),
//!         None => break,
//!     }
//! }
//! ```

use crate::raw::rekindle::plugin::history as raw;
pub use raw::{HistoryError, HistoryQuery, Page};

/// Up to `limit` messages (at most 1000), newest first, older than
/// `before`, or the newest page with `None`.
pub fn before(channel: u64, before: Option<u64>, limit: u32) -> Result<Page, HistoryError> {
    raw::query(HistoryQuery {
        channel,
        before,
        after: None,
        limit,
    })
}

/// Up to `limit` messages (at most 1000), oldest first, newer than `after`.
pub fn after(channel: u64, after: u64, limit: u32) -> Result<Page, HistoryError> {
    raw::query(HistoryQuery {
        channel,
        before: None,
        after: Some(after),
        limit,
    })
}
