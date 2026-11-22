//! Message content the client has received (requires `read-messages`).
//!
//! Queries see only what the client has loaded: scrolling up loads more,
//! and plugins can't. [`resolve_link`] is the exception, and makes the fetch
//! the client makes when the user clicks a message link that's on screen.

use crate::MessageRef;
use crate::raw::rekindle::plugin::messages as raw;
pub use crate::records::{Attachment, Embed, EmbedMedia, Forwarded, Message, Poll, Reaction};
pub use raw::LinkError;

/// The prefixes of a message link, one per Discord domain.
const LINK_PREFIXES: [&str; 3] = [
    "https://discord.com/channels/",
    "https://ptb.discord.com/channels/",
    "https://canary.discord.com/channels/",
];

/// A loaded message.
pub fn get(channel: u64, id: u64) -> Option<Message> {
    raw::get_message(channel, id)
}

/// Loaded messages in `channel`, newest first, older than `before` if
/// given; at most `limit`, up to 100.
pub fn loaded(channel: u64, before: Option<u64>, limit: u32) -> Vec<Message> {
    raw::loaded_messages(channel, before, limit)
}

/// The messages on screen, top to bottom.
pub fn visible() -> Vec<MessageRef> {
    raw::visible_messages()
}

/// The pinned messages the client has loaded for `channel`.
pub fn pins(channel: u64) -> Vec<MessageRef> {
    raw::pins(channel)
}

/// The message a link (`https://discord.com/channels/g/c/m`) points to, for
/// a link preview. Works only while a message on screen contains the link,
/// and is rate-limited.
pub fn resolve_link(url: &str) -> Result<Message, LinkError> {
    raw::resolve_message_link(url)
}

/// The message links in `text`, in order. A link ends at its last digit, so
/// punctuation after it in a sentence isn't part of it.
pub fn links_in(text: &str) -> Vec<&str> {
    text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '(' | ')'))
        .map(|w| w.trim_end_matches(|c: char| !c.is_ascii_digit()))
        .filter(|w| LINK_PREFIXES.iter().any(|p| w.starts_with(p)))
        .collect()
}

/// The reference to `m` that actions and accessories take.
pub fn reference(m: &Message) -> MessageRef {
    MessageRef {
        id: m.id,
        channel: m.channel,
        guild: m.guild,
        author: m.author.id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_message_links_in_brackets_and_markdown() {
        let text = "<https://ptb.discord.com/channels/1/2/4> and \
                    [x](https://canary.discord.com/channels/@me/2/5), not https://example.com/channels/1";
        assert_eq!(
            links_in(text),
            [
                "https://ptb.discord.com/channels/1/2/4",
                "https://canary.discord.com/channels/@me/2/5"
            ]
        );
    }

    #[test]
    fn punctuation_after_a_link_isnt_part_of_it() {
        let text = "see https://discord.com/channels/1/2/3, or https://discord.com/channels/1/2/4.";
        assert_eq!(
            links_in(text),
            [
                "https://discord.com/channels/1/2/3",
                "https://discord.com/channels/1/2/4"
            ]
        );
        assert!(links_in("https://discord.com/channels/").is_empty());
    }
}
