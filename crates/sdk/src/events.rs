//! Client events, delivered to [`Plugin::on_event`](crate::Plugin::on_event)
//! one host tick at a time.
//!
//! Each kind needs a capability: client structure and the user's own state
//! need `read-client`, other people `read-users`, message content
//! `read-messages`, and the cursor `pointer`. The plugin receives every kind
//! it has the capability for, until [`subscribe`] narrows that to a list.

pub use crate::raw::rekindle::plugin::events::{
    ConnectionState, DeletedMessage, Event, EventKind, EventPatch, GuildLeftReason, ReactionChange,
    RelationshipChange, SelectionChange,
};
use crate::raw::rekindle::plugin::host_events as raw;
pub use raw::SubscribeError;
/// What [`Plugin::on_intercept`](crate::Plugin::on_intercept) returns.
pub use rekindle_plugin_api::exports::rekindle::plugin::event_filter::Verdict;

/// Delivers only these kinds from now on. Returns the kinds that were
/// refused, each with the reason.
pub fn subscribe(kinds: &[EventKind]) -> Vec<(EventKind, SubscribeError)> {
    refused(raw::subscribe(kinds))
}

/// Has [`Plugin::on_intercept`](crate::Plugin::on_intercept) see these
/// kinds before the client processes them. Needs `intercept`, and each
/// kind's own capability. Returns the kinds that were refused: not granted,
/// or `unavailable` for kinds that don't come from Discord.
pub fn intercept(kinds: &[EventKind]) -> Vec<(EventKind, SubscribeError)> {
    refused(raw::intercept(kinds))
}

fn refused(
    results: Vec<(EventKind, Result<(), SubscribeError>)>,
) -> Vec<(EventKind, SubscribeError)> {
    results
        .into_iter()
        .filter_map(|(k, r)| r.err().map(|e| (k, e)))
        .collect()
}
