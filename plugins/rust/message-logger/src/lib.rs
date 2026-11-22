//! Message Logger, after Vencord's `MessageLogger`: keeps what your client
//! received when it's deleted or edited, on this device, and shows it where
//! it was.
//!
//! - Deleted messages come back as a **ghost row** in the channel, in the
//!   client's own deleted style and labelled with this plugin's name (never
//!   mistakable for a live message).
//! - Edited messages get their **edit history** under them as an accessory.
//!
//! The host hands over its own cached copy with `message-deleted` and the
//! previous text with `message-edited` (owner decision 2026-11-05/07: local
//! logging of your own received view is in scope). Without `network` this
//! plugin can't send any of it anywhere. Settings come from the manifest's
//! schema, so the user can see and change them without the plugin running.

use rekindle_sdk::decorations;
use rekindle_sdk::messages::Message;
use rekindle_sdk::prelude::*;

const MAX_HISTORY: usize = 10;

struct MessageLogger;

fn ignored(m: &Message) -> bool {
    (settings::bool("ignore-bots") && (m.bot || m.webhook))
        || settings::ids("ignored-users").contains(&m.author.id)
}

fn deleted_key(channel: u64, id: u64) -> String {
    format!("del:{channel}:{id}")
}

fn edits_key(id: u64) -> String {
    format!("edits:{id}")
}

fn show_ghost(channel: u64, id: u64, author: UserRef, content: &str) {
    let spans = vec![span(content)];
    if let Err(e) = decorations::set_ghost(channel, id, author, spans, "deleted") {
        warn!("couldn't show a deleted message: {e:?}");
    }
}

fn on_deleted(channel: u64, id: u64, cached: Option<Message>) {
    let Some(m) = cached else { return };
    if !settings::bool("log-deletes") || ignored(&m) {
        return;
    }
    // author id \t name \t content
    let record = format!(
        "{}\t{}\t{}",
        m.author.id,
        m.author.name.replace('\t', " "),
        m.content
    );
    if let Err(e) = storage::set(&deleted_key(channel, id), &record) {
        warn!("couldn't keep a deleted message: {e:?}");
        return;
    }
    show_ghost(channel, id, m.author, &m.content);
}

fn on_edited(m: &Message, before: Option<String>) {
    let Some(before) = before else { return };
    if !settings::bool("log-edits") || ignored(m) || before == m.content {
        return;
    }
    let mut history: Vec<String> = storage::get(&edits_key(m.id))
        .map(|s| s.split('\u{1f}').map(str::to_string).collect())
        .unwrap_or_default();
    history.push(before);
    if history.len() > MAX_HISTORY {
        history.remove(0);
    }
    if let Err(e) = storage::set(&edits_key(m.id), history.join("\u{1f}")) {
        warn!("couldn't keep an edit: {e:?}");
    }
    let mut elements: Vec<ui::Element> =
        Vec::new().text(&format!("Edited {} time(s). Before:", history.len()));
    for old in history.iter().rev() {
        elements = elements.rich_text(vec![span(old).italic().color_token("text-muted")]);
    }
    if let Err(e) = decorations::set_accessory(&messages::reference(m), Some(&elements)) {
        warn!("couldn't show an edit history: {e:?}");
    }
}

/// Ghost rows for what was kept in this channel (after a restart).
fn restore(channel: u64) {
    for key in storage::keys_with_prefix(&format!("del:{channel}:")) {
        let Some(id) = key.rsplit(':').next().and_then(|i| i.parse().ok()) else {
            continue;
        };
        let Some(rec) = storage::get(&key) else {
            continue;
        };
        let mut f = rec.splitn(3, '\t');
        let (Some(aid), Some(name), Some(content)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let author = UserRef {
            id: aid.parse().unwrap_or(0),
            name: name.into(),
        };
        show_ghost(channel, id, author, content);
    }
}

impl Plugin for MessageLogger {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        events::subscribe(&[EventKind::Message]);
        if let Some(ch) = messages::visible().first().map(|m| m.channel) {
            restore(ch);
        }
        ui::register_command("clear-log", "Message Logger: forget everything kept");
        Ok(MessageLogger)
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::MessageDeleted(d) => on_deleted(d.channel, d.id, d.cached),
            Event::MessageEdited((m, before)) => on_edited(&m, before),
            _ => {}
        }
    }

    fn on_command(&mut self, id: &str) {
        if id == "clear-log" {
            for k in storage::keys() {
                if let Some(rest) = k.strip_prefix("del:") {
                    let mut p = rest.split(':').filter_map(|x| x.parse::<u64>().ok());
                    if let (Some(ch), Some(id)) = (p.next(), p.next()) {
                        decorations::remove_ghost(ch, id);
                    }
                }
                storage::delete(&k);
            }
            ui::toast_kind("Message log cleared", ToastKind::Success);
        }
    }
}

rekindle_sdk::plugin!(MessageLogger);
