//! Platform Indicators, after Vencord's `PlatformIndicators`: small icons
//! after names showing which devices someone is on, from presence.
//!
//! Keyed decorations (pushed from `presence` events) plus a setting from the
//! manifest's schema that picks where they appear.

use rekindle_sdk::decorations::{self, Decoration, MEMBER_LIST_TRAILING, NAME_BADGE};
use rekindle_sdk::prelude::*;
use rekindle_sdk::users::{ClientStatus, Presence};

struct PlatformIndicators;

/// "online" -> green, "idle" -> amber, "dnd" -> red.
fn status_color(s: &str) -> Color {
    match s {
        "idle" => Color::rgb(0xf0, 0xb2, 0x32),
        "dnd" => Color::rgb(0xf2, 0x3f, 0x43),
        _ => Color::rgb(0x23, 0xa5, 0x5a),
    }
}

/// One decoration summarising the platforms (the host draws one per key).
fn decoration(cs: &ClientStatus) -> Option<Decoration> {
    let on: Vec<(&str, &str, Icon)> = [
        ("Desktop", cs.desktop.as_str(), Icon::Code),
        ("Mobile", cs.mobile.as_str(), Icon::Smile),
        ("Web", cs.web.as_str(), Icon::Globe),
    ]
    .into_iter()
    .filter(|(_, s, _)| !s.is_empty() && *s != "offline")
    .collect();
    let (_, first_status, icon) = *on.first()?;
    let tooltip = on
        .iter()
        .map(|(n, s, _)| format!("{n}: {s}"))
        .collect::<Vec<_>>()
        .join(" · ");
    let mut d = Decoration::icon(icon)
        .tooltip(&tooltip)
        .color(status_color(first_status));
    if on.len() > 1 {
        d = d.with_text(&on.len().to_string());
    }
    Some(d)
}

fn kinds() -> Vec<&'static str> {
    match settings::selected("where") {
        0 => vec![NAME_BADGE],
        1 => vec![MEMBER_LIST_TRAILING],
        _ => vec![NAME_BADGE, MEMBER_LIST_TRAILING],
    }
}

fn apply(p: &Presence) {
    let d = decoration(&p.client_status);
    for k in kinds() {
        if let Err(e) = decorations::set(k, p.user, d.clone()) {
            debug!("couldn't decorate user {} in {k}: {e:?}", p.user);
        }
    }
}

fn refresh_all() {
    decorations::clear(NAME_BADGE);
    decorations::clear(MEMBER_LIST_TRAILING);
    // Everyone the client has loaded in any guild.
    let mut seen = std::collections::BTreeSet::new();
    for g in state::guild_ids() {
        for m in users::loaded_members(g, 1000).0 {
            if seen.insert(m.user)
                && let Some(p) = users::presence(m.user, None)
            {
                apply(&p);
            }
        }
    }
}

impl Plugin for PlatformIndicators {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        events::subscribe(&[EventKind::Presence]);
        refresh_all();
        Ok(PlatformIndicators)
    }

    fn on_event(&mut self, event: Event) {
        if let Event::Presence(p) = event {
            apply(&p);
        }
    }

    fn on_settings_changed(&mut self, key: &str) {
        if key == "where" {
            refresh_all();
        }
    }
}

rekindle_sdk::plugin!(PlatformIndicators);
