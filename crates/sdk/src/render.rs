//! Changing how messages look (requires `render-messages`). Implement
//! [`Plugin::render`](crate::Plugin::render): it runs in the *render
//! isolate*, a second instance of the plugin that sees message content but
//! can't keep or send it (no storage writes, no network, no timers, no UI).
//! Its only output is these transforms, which the host draws.
//!
//! The host enforces the output rules: no URLs (span targets can't be links
//! or media), images are the plugin's registered ones, and accessory buttons
//! may only use ids the main instance declared with [`declare_actions`].
//! Their clicks reach the main instance's
//! [`Plugin::on_ui`](crate::Plugin::on_ui) with the surface `message:<id>`.
//!
//! ```no_run
//! # use rekindle_sdk::prelude::*;
//! # use rekindle_sdk::SpanColor;
//! # struct Highlighter;
//! # impl Plugin for Highlighter {
//! #     fn activate(_: &Granted) -> Result<Self, String> { Ok(Highlighter) }
//! fn render(input: &RenderInput) -> Vec<Transform> {
//!     let brand = Some(SpanColor::Token("text-brand".into()));
//!     render::highlight_all(&input.message.content, "rekindle", brand, false)
//!         .into_iter()
//!         .collect()
//! }
//! # }
//! ```

use crate::raw::rekindle::plugin::render_control as raw;
pub use crate::raw::rekindle::plugin::render_types::{
    Badge, EmbedPatch, Highlight, RenderInput, Transform,
};
use crate::{Color, Icon, Span, SpanColor};

/// Declares the accessory button ids the isolate may use. Call it from the
/// main instance.
pub fn declare_actions(ids: &[&str]) {
    let ids: Vec<String> = ids.iter().map(|id| (*id).to_string()).collect();
    raw::declare_actions(&ids);
}

/// Renders again every message, or one channel's. A settings change
/// invalidates by itself.
pub fn invalidate(channel: Option<u64>) {
    raw::invalidate(channel);
}

/// Shows a bar saying the plugin hid the message, with `reason` and a Show
/// button, in place of the message.
pub fn collapse(reason: &str) -> Transform {
    Transform::Collapse(reason.into())
}

/// Styles the byte range `start..end` of the content.
pub fn highlight(start: usize, end: usize, color: Option<SpanColor>) -> Transform {
    Transform::Highlight(vec![Highlight {
        start: start as u32,
        end: end as u32,
        color,
        bold: false,
        tooltip: None,
    }])
}

/// Highlights every occurrence of `needle` in `content`, ignoring case, or
/// returns `None` if there is none.
#[expect(
    clippy::needless_pass_by_value,
    reason = "callers build the colour inline, and each range gets a clone"
)]
pub fn highlight_all(
    content: &str,
    needle: &str,
    color: Option<SpanColor>,
    bold: bool,
) -> Option<Transform> {
    let needle: String = needle.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return None;
    }
    // Lower-casing can change a character's length (`İ` is 2 bytes, `i̇`
    // 3), so each byte of `hay` keeps the range of the character of
    // `content` it came from.
    let mut hay = String::with_capacity(content.len());
    let mut origin = Vec::with_capacity(content.len());
    for (i, c) in content.char_indices() {
        for lower in c.to_lowercase() {
            hay.push(lower);
            origin.extend(std::iter::repeat_n((i, i + c.len_utf8()), lower.len_utf8()));
        }
    }
    let ranges: Vec<Highlight> = hay
        .match_indices(&needle)
        .map(|(at, found)| Highlight {
            start: origin[at].0 as u32,
            end: origin[at + found.len() - 1].1 as u32,
            color: color.clone(),
            bold,
            tooltip: None,
        })
        .collect();
    (!ranges.is_empty()).then_some(Transform::Highlight(ranges))
}

/// Inline spans after the content, such as "(translated)".
pub fn append(spans: Vec<Span>) -> Transform {
    Transform::Append(spans)
}

/// Replaces how the content reads; the original stays one click away.
pub fn replace_content(spans: Vec<Span>) -> Transform {
    Transform::ReplaceContent(spans)
}

/// Tints the message row.
pub fn background(color: SpanColor) -> Transform {
    Transform::Background(color)
}

/// A badge after the author's name.
pub fn author_badge(
    icon: Option<Icon>,
    text: Option<&str>,
    tooltip: &str,
    color: Option<Color>,
) -> Transform {
    Transform::AuthorBadge(Badge {
        icon,
        image: None,
        text: text.map(Into::into),
        tooltip: tooltip.into(),
        color,
    })
}

/// Changes the embed at index `n` by `patch`, which [`embed_patch`]
/// starts.
pub fn replace_embed(n: u32, patch: EmbedPatch) -> Transform {
    Transform::ReplaceEmbed((n, patch))
}

/// An embed patch that changes nothing yet: set the fields to change.
pub fn embed_patch() -> EmbedPatch {
    EmbedPatch {
        title: None,
        description: None,
        footer: None,
        color: None,
        image: None,
        hide_media: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(t: Option<Transform>) -> Vec<(u32, u32)> {
        match t {
            Some(Transform::Highlight(hs)) => hs.iter().map(|h| (h.start, h.end)).collect(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn highlights_every_occurrence_ignoring_case() {
        let t = highlight_all("Rekindle and REKINDLE again", "rekindle", None, true);
        assert_eq!(ranges(t), [(0, 8), (13, 21)]);
        assert_eq!(highlight_all("nothing here", "rekindle", None, true), None);
        assert_eq!(highlight_all("anything", "", None, true), None);
    }

    #[test]
    fn ranges_fit_the_content_when_lower_casing_changes_lengths() {
        // `İ` lower-cases to three bytes and `ẞ` to two, so the lower-cased
        // text has the same length but the words between them move.
        let content = "İ rekindle ẞ";
        let t = highlight_all(content, "REKINDLE", None, false);
        assert_eq!(ranges(t), [(3, 11)]);
        assert_eq!(&content[3..11], "rekindle");
        // A match inside a character's lower-case form covers the character.
        assert_eq!(
            ranges(highlight_all("İstanbul", "i", None, false)),
            [(0, 2)]
        );
    }
}
