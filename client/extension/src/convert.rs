//! Plugin UI from the runtime's model, as the items `rekindle-ui-hook` draws:
//! panel elements as widgets, icons as glyphs.

use rekindle_plugin_host::model::{Element, Icon};
use rekindle_ui_hook::Widget;

/// The text an element reads as, on surfaces that show text only.
fn text_of(e: &Element) -> Option<String> {
    match e {
        Element::Heading { text } | Element::Text { text } => Some(text.clone()),
        Element::RichText { spans } => Some(spans.iter().map(|s| s.text.as_str()).collect()),
        Element::Field { key, value } => Some(format!("{key}: {value}")),
        Element::User { id } => Some(format!("user {id}")),
        _ => None,
    }
}

/// The text lines of a panel's elements, for a profile section.
pub(crate) fn lines_of(elements: &[Element]) -> Vec<String> {
    elements.iter().filter_map(text_of).collect()
}

/// A panel's elements as interactive widgets.
pub(crate) fn widgets_of(elements: &[Element]) -> Vec<Widget> {
    elements.iter().map(widget).collect()
}

fn widget(e: &Element) -> Widget {
    match e.clone() {
        Element::Heading { text } => Widget::Heading(text),
        Element::Text { text } => Widget::Text(text),
        Element::Separator => Widget::Separator,
        Element::Toggle { id, label, value } => Widget::Toggle { id, label, value },
        Element::Button { id, label, .. } => Widget::Button { id, label },
        Element::TextInput {
            id,
            label,
            value,
            placeholder,
            ..
        } => Widget::TextInput {
            id,
            label,
            value,
            placeholder,
        },
        Element::Select {
            id,
            label,
            options,
            selected,
        } => Widget::Select {
            id,
            label,
            options,
            selected,
        },
        Element::Slider {
            id,
            label,
            min,
            max,
            step,
            value,
        } => Widget::Slider {
            id,
            label,
            min,
            max,
            step,
            value,
        },
        Element::ColorPicker { id, label, value } => {
            let hex = if value.a == 255 {
                format!("#{:02x}{:02x}{:02x}", value.r, value.g, value.b)
            } else {
                format!(
                    "#{:02x}{:02x}{:02x}{:02x}",
                    value.r, value.g, value.b, value.a
                )
            };
            Widget::Color { id, label, hex }
        }
        Element::Tabs { id, tabs, selected } => Widget::Tabs { id, tabs, selected },
        Element::Header {
            back,
            title,
            meta,
            toggle,
            action,
        } => Widget::Header {
            back,
            title,
            meta,
            toggle: toggle.map(|t| (t.id, t.label, t.value)),
            action: action.map(|a| (a.id, a.label)),
        },
        Element::Cards { cards } => Widget::Cards(
            cards
                .into_iter()
                .map(|c| rekindle_ui_hook::CardItem {
                    id: c.id,
                    title: c.title,
                    meta: c.meta,
                    text: c.text,
                    state: c.state,
                    action: c.action.map(|a| (a.id, a.label)),
                })
                .collect(),
        ),
        // No table, list or image widget yet: these show what they hold.
        Element::Table { label, rows, .. } => Widget::Text(format!("{label}: {} rows", rows.len())),
        Element::StringList { label, items, .. } => {
            Widget::Text(format!("{label}: {}", items.join(", ")))
        }
        Element::Image { image, .. } => Widget::Text(format!("[{image}]")),
        other => Widget::Text(text_of(&other).unwrap_or_default()),
    }
}

/// A short glyph for an icon. rekindle ships no icon assets, so a chat-bar
/// button shows one of these.
pub(crate) fn glyph(icon: Icon) -> &'static str {
    match icon {
        Icon::Star => "★",
        Icon::Bookmark => "🔖",
        Icon::Note => "🗒",
        Icon::Palette => "🎨",
        Icon::Bolt => "⚡",
        Icon::Heart => "♥",
        Icon::Bell => "🔔",
        Icon::Gear => "⚙",
        Icon::Link => "🔗",
        Icon::Search => "🔍",
        Icon::Sparkle => "✨",
        Icon::Eye => "👁",
        Icon::Clock => "🕑",
        Icon::Pin => "📌",
        Icon::Puzzle => "🧩",
        Icon::EyeOff => "🙈",
        Icon::Translate => "🌐",
        Icon::Microphone => "🎤",
        Icon::Headphones => "🎧",
        Icon::Camera => "📷",
        Icon::Shield => "🛡",
        Icon::Lock => "🔒",
        Icon::User => "👤",
        Icon::Users => "👥",
        Icon::Chat => "💬",
        Icon::Reply => "↩",
        Icon::Copy => "⧉",
        Icon::Trash => "🗑",
        Icon::Edit => "✎",
        Icon::Check => "✓",
        Icon::Cross => "✕",
        Icon::Info => "ℹ",
        Icon::Warning => "⚠",
        Icon::Download => "⬇",
        Icon::Upload => "⬆",
        Icon::Globe => "🌍",
        Icon::Code => "⌨",
        Icon::Hash => "#",
        Icon::At => "@",
        Icon::Smile => "☺",
        Icon::Image => "🖼",
        Icon::Music => "♪",
        Icon::Play => "▶",
        Icon::Pause => "⏸",
    }
}

/// An icon's name in the contract, which a host maps to the client's own
/// icon set.
pub(crate) fn icon_name(icon: Icon) -> &'static str {
    match icon {
        Icon::Star => "star",
        Icon::Bookmark => "bookmark",
        Icon::Note => "note",
        Icon::Palette => "palette",
        Icon::Bolt => "bolt",
        Icon::Heart => "heart",
        Icon::Bell => "bell",
        Icon::Gear => "gear",
        Icon::Link => "link",
        Icon::Search => "search",
        Icon::Sparkle => "sparkle",
        Icon::Eye => "eye",
        Icon::Clock => "clock",
        Icon::Pin => "pin",
        Icon::Puzzle => "puzzle",
        Icon::EyeOff => "eye-off",
        Icon::Translate => "translate",
        Icon::Microphone => "microphone",
        Icon::Headphones => "headphones",
        Icon::Camera => "camera",
        Icon::Shield => "shield",
        Icon::Lock => "lock",
        Icon::User => "user",
        Icon::Users => "users",
        Icon::Chat => "chat",
        Icon::Reply => "reply",
        Icon::Copy => "copy",
        Icon::Trash => "trash",
        Icon::Edit => "edit",
        Icon::Check => "check",
        Icon::Cross => "cross",
        Icon::Info => "info",
        Icon::Warning => "warning",
        Icon::Download => "download",
        Icon::Upload => "upload",
        Icon::Globe => "globe",
        Icon::Code => "code",
        Icon::Hash => "hash",
        Icon::At => "at",
        Icon::Smile => "smile",
        Icon::Image => "image",
        Icon::Music => "music",
        Icon::Play => "play",
        Icon::Pause => "pause",
    }
}

/// What a renderer is told about a row the client drew.
pub(crate) fn render_input(
    row: &rekindle_ui_hook::render::RenderRequest,
) -> rekindle_plugin_host::api::RenderInput {
    use rekindle_plugin_host::api::{Message, RenderInput, UserRef};
    let m = &row.message;
    let name = row
        .author
        .as_ref()
        .map_or_else(String::new, |u| u.display.clone());
    RenderInput {
        message: Message {
            id: m.id,
            channel: row.channel,
            guild: row.guild,
            author: UserRef { id: m.author, name },
            bot: row.author.as_ref().is_some_and(|u| u.bot),
            webhook: false,
            kind: 0,
            flag_bits: 0,
            content: m.body.clone(),
            // The client keeps that a message was edited, not when.
            edited_ms: m.edited.then_some(0),
            pinned: m.pinned,
            tts: false,
            mentions_everyone: false,
            mentions: vec![],
            mention_roles: vec![],
            reply_to: None,
            attachments: vec![],
            embeds: vec![],
            reactions: vec![],
            stickers: vec![],
            poll: None,
            thread: None,
            forwarded: None,
            ephemeral: false,
            sending: false,
            failed: false,
            ..Default::default()
        },
        author_roles: row.author_roles.clone(),
        grouped: row.grouped,
        dark: true,
    }
}

/// The plugins' transforms for one row, as the client's look. `name` gives
/// a plugin's name for the row's "changed by" line, and `token` a theme
/// token's colour now. A transform the client has no place for is left out.
pub(crate) fn look(
    per: &[rekindle_plugin_host::runtime::Rendered],
    name: impl Fn(&str) -> String,
    token: impl Fn(&str) -> Option<u32>,
) -> rekindle_ui_hook::render::Look {
    use rekindle_plugin_host::api::{SpanColor, Transform};
    use rekindle_ui_hook::render::{Look, LookBadge};
    let rgba = |c: &rekindle_plugin_host::api::WitColor| u32::from_be_bytes([c.r, c.g, c.b, c.a]);
    let span_color = |c: &SpanColor| match c {
        SpanColor::Token(t) => token(t).unwrap_or(0),
        SpanColor::Rgba(c) => rgba(c),
    };
    let text = |spans: &[rekindle_plugin_host::api::Span]| {
        spans.iter().map(|s| s.text.as_str()).collect::<String>()
    };
    let mut look = Look::default();
    for r in per {
        let mut noticed = false;
        for t in &r.transforms {
            match t {
                Transform::AuthorBadge(b) => look.badges.push(LookBadge {
                    text: b.text.clone().unwrap_or_else(|| "\u{2022}".into()),
                    tooltip: b.tooltip.clone(),
                    color: b.color.as_ref().map_or(0, rgba),
                }),
                Transform::AuthorColor(c) => look.name_color = rgba(c),
                Transform::Background(c) => look.tint = span_color(c),
                Transform::Highlight(hs) => look.highlights.extend(hs.iter().map(|h| {
                    (
                        h.start as usize,
                        h.end as usize,
                        h.color.as_ref().map_or(0, span_color),
                    )
                })),
                Transform::Append(spans) => {
                    if !look.append.is_empty() {
                        look.append.push(' ');
                    }
                    look.append.push_str(&text(spans));
                }
                Transform::ReplaceContent(spans) => {
                    look.replace = text(spans);
                    noticed = true;
                }
                Transform::Collapse(why) => {
                    look.collapse.clone_from(why);
                    noticed = true;
                }
                Transform::Blur => {
                    look.blur = true;
                    noticed = true;
                }
                _ => {}
            }
        }
        if noticed {
            look.by.push(name(&r.plugin));
        }
    }
    look
}

#[cfg(test)]
mod tests {
    use rekindle_plugin_host::api::{Badge, Highlight, SpanColor, Transform, WitColor};
    use rekindle_plugin_host::runtime::Rendered;

    #[test]
    fn transforms_become_the_rows_look() {
        let gold = WitColor {
            r: 0xf0,
            g: 0xb2,
            b: 0x32,
            a: 0xff,
        };
        let per = vec![
            Rendered {
                plugin: "dev.a.staff".into(),
                transforms: vec![Transform::AuthorBadge(Badge {
                    icon: None,
                    image: None,
                    text: Some("Owner".into()),
                    tooltip: "Server owner".into(),
                    color: Some(gold),
                })],
            },
            Rendered {
                plugin: "dev.a.words".into(),
                transforms: vec![
                    Transform::Highlight(vec![Highlight {
                        start: 4,
                        end: 12,
                        color: Some(SpanColor::Token("text-feedback-warning".into())),
                        bold: true,
                        tooltip: None,
                    }]),
                    Transform::Background(SpanColor::Rgba(gold)),
                    Transform::Collapse("muted word".into()),
                ],
            },
        ];
        let look = super::look(
            &per,
            |id| format!("name of {id}"),
            |t| (t == "text-feedback-warning").then_some(0x1122_33ff),
        );
        assert_eq!(look.badges.len(), 1);
        assert_eq!(look.badges[0].text, "Owner");
        assert_eq!(look.badges[0].color, 0xf0b2_32ff);
        assert_eq!(look.highlights, vec![(4, 12, 0x1122_33ff)]);
        assert_eq!(look.tint, 0xf0b2_32ff);
        assert_eq!(look.collapse, "muted word");
        // Only a change a reader notices names its plugin.
        assert_eq!(look.by, vec!["name of dev.a.words".to_string()]);
    }
}
