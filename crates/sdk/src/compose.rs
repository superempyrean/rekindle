//! The user's own outgoing messages (requires `compose`). See
//! [`Plugin::on_compose`](crate::Plugin::on_compose).
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! # struct Shrug;
//! # impl Plugin for Shrug {
//! #     fn activate(_: &Granted) -> Result<Self, String> { Ok(Shrug) }
//! fn on_compose(&mut self, d: &Draft) -> ComposeResult {
//!     if d.text.contains(":shrug:") {
//!         compose::replace(d.text.replace(":shrug:", "¯\\_(ツ)_/¯"))
//!     } else {
//!         ComposeResult::Keep
//!     }
//! }
//! # }
//! ```

pub use crate::raw::rekindle::plugin::compose_control::SplitError;
pub use crate::raw::rekindle::plugin::compose_types::{
    AttachmentInfo, ComposeKind, ComposeResult, Draft, ReplyInfo, Rewrite, Upload, UploadResult,
};

/// Sends the message as `parts`, in order, instead of as one: 2 to 10
/// parts, each a message the client would send. The reply, attachments and
/// stickers go with the first part. Works only within
/// [`Plugin::on_compose`](crate::Plugin::on_compose) for a send; return
/// [`ComposeResult::Keep`] after it, since a rewrite from the same call is
/// ignored.
pub fn split<S: AsRef<str>>(parts: &[S]) -> Result<(), SplitError> {
    let parts: Vec<String> = parts.iter().map(|p| p.as_ref().to_string()).collect();
    crate::raw::rekindle::plugin::compose_control::split(&parts)
}

/// Sends `text` in place of the draft's text.
pub fn replace(text: impl Into<String>) -> ComposeResult {
    ComposeResult::Replace(Rewrite {
        text: text.into(),
        mention: None,
        silent: None,
    })
}

/// Keeps the text and sets whether the reply pings its author.
pub fn set_mention(d: &Draft, mention: bool) -> ComposeResult {
    ComposeResult::Replace(Rewrite {
        text: d.text.clone(),
        mention: Some(mention),
        silent: None,
    })
}

/// Doesn't send: the draft stays in the composer, and the user is told
/// `reason`.
pub fn cancel(reason: &str) -> ComposeResult {
    ComposeResult::Cancel(reason.into())
}
