//! Message Link Embeds, after Vencord's `MessageLinkEmbeds`: when a message
//! links to another message, show a preview of it underneath.
//!
//! `messages::resolve_link` (owner decision 2026-11-07) works only while a
//! message containing the link is on screen, and makes the same fetch the
//! client makes when the user clicks it, so it can't be used to crawl.

use rekindle_sdk::decorations;
use rekindle_sdk::messages::{self, Message};
use rekindle_sdk::prelude::*;
use rekindle_sdk::time::TimeStyle;

struct LinkEmbeds;

fn preview(m: &Message) {
    let mut els: Vec<ui::Element> = Vec::new();
    for url in messages::links_in(&m.content) {
        match messages::resolve_link(url) {
            Ok(linked) => {
                let where_ = state::channel(linked.channel)
                    .map(|c| format!("#{}", c.name))
                    .unwrap_or_default();
                let when = time::format(time::snowflake_ms(linked.id), TimeStyle::ShortDateTime);
                els = els
                    .rich_text(vec![
                        span(&linked.author.name).bold(),
                        span(&format!("  {where_} · {when}")).color_token("text-muted"),
                    ])
                    .text(&linked.content);
            }
            Err(e) => debug!("couldn't resolve {url}: {e:?}"),
        }
    }
    if !els.is_empty()
        && let Err(e) = decorations::set_accessory(&messages::reference(m), Some(&els))
    {
        warn!("couldn't show a preview: {e:?}");
    }
}

impl Plugin for LinkEmbeds {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        events::subscribe(&[EventKind::Message]);
        for r in messages::visible() {
            if let Some(m) = messages::get(r.channel, r.id) {
                preview(&m);
            }
        }
        Ok(LinkEmbeds)
    }

    fn on_event(&mut self, event: Event) {
        if let Event::MessageCreated(m) | Event::MessageEdited((m, _)) = event {
            preview(&m);
        }
    }
}

rekindle_sdk::plugin!(LinkEmbeds);
