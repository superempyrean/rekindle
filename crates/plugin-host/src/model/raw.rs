//! Engine work a plugin with `wire-raw` writes itself (`wire-raw` in the
//! WIT), checked before a backend keeps it.

use rekindle_wire_inject::{Handler, PressAction, RESERVED, Work};

/// A plugin's own engine work, as `rekindle-wire-inject` reads it.
pub type RawWork = Work;

/// The largest work, as JSON, a plugin may set.
pub const MAX_RAW_BYTES: usize = 256 * 1024;
/// How many node ids a plugin's mounts may use, from
/// [`RESERVED`](rekindle_wire_inject::RESERVED) up. The host moves them to
/// a block of the plugin's own.
pub const RAW_IDS: u32 = 0x1_0000;
/// The handler surface of a press a plugin's work keeps: the plugin's
/// `on-ui` receives it as `clicked` on this surface.
pub const RAW_SURFACE: &str = "wire-raw";

/// `json` as `plugin`'s work: read, checked, and with every handler and
/// press rule made `plugin`'s, on [`RAW_SURFACE`]. An error says what's
/// wrong.
///
/// The work may not take the system back, and its mounts' ids are
/// [`RESERVED`] to `RESERVED + RAW_IDS`.
pub fn raw_work(plugin: &str, json: &str) -> Result<Work, String> {
    if json.len() > MAX_RAW_BYTES {
        return Err(format!(
            "the work is {} bytes; the limit is {MAX_RAW_BYTES}",
            json.len()
        ));
    }
    let mut work = Work::from_json(json).map_err(|e| e.to_string())?;
    if work.back.is_some() {
        return Err("a plugin's work can't take the system back".into());
    }
    if let Some(n) = work
        .mounts
        .iter()
        .flat_map(|m| &m.nodes)
        .find(|n| n.id >= RESERVED + RAW_IDS)
    {
        return Err(format!(
            "mount node {:#x} is past the plugin's ids ({RESERVED:#x} to {:#x})",
            n.id,
            RESERVED + RAW_IDS - 1
        ));
    }
    let own = |h: &mut Handler| {
        if let Handler::Plugin {
            plugin: p, surface, ..
        } = h
        {
            *p = plugin.to_string();
            *surface = RAW_SURFACE.to_string();
        }
    };
    for n in work.mounts.iter_mut().flat_map(|m| &mut m.nodes) {
        if let Some(h) = &mut n.on_press {
            own(h);
        }
    }
    for r in &mut work.presses {
        if let PressAction::Plugin {
            plugin: p, surface, ..
        } = &mut r.action
        {
            *p = plugin.to_string();
            *surface = RAW_SURFACE.to_string();
        }
    }
    Ok(work)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handlers_become_the_plugins_own() {
        let json = r#"{
            "mounts": [{
                "under": {"cell": {"prop": "accessibilityId", "value": {"str": "composer"}}},
                "root": 4026531840,
                "nodes": [{"id": 4026531840, "kind": "view",
                    "on_press": {"plugin": {"plugin": "someone.else", "surface": "chat-bar.button", "element": "go"}}}]
            }],
            "presses": [{"select": {"label": "Gift"},
                "action": {"plugin": {"plugin": "x", "surface": "y", "element": "gift"}}}]
        }"#;
        let w = raw_work("dev.me.p", json).unwrap();
        assert_eq!(
            w.mounts[0].nodes[0].on_press,
            Some(Handler::Plugin {
                plugin: "dev.me.p".into(),
                surface: RAW_SURFACE.into(),
                element: "go".into(),
            })
        );
        assert_eq!(
            w.presses[0].action,
            PressAction::Plugin {
                plugin: "dev.me.p".into(),
                surface: RAW_SURFACE.into(),
                element: "gift".into(),
                forward: false,
            }
        );
    }

    #[test]
    fn refuses_the_back_ids_past_its_block_and_bad_json() {
        let back = r#"{"back": {"plugin": {"plugin": "p", "surface": "s", "element": "e"}}}"#;
        assert!(raw_work("p", back).unwrap_err().contains("system back"));
        let far = format!(
            r#"{{"mounts": [{{"under": {{"kind": "view"}}, "root": {id}, "nodes": [{{"id": {id}, "kind": "view"}}]}}]}}"#,
            id = RESERVED + RAW_IDS
        );
        assert!(
            raw_work("p", &far)
                .unwrap_err()
                .contains("past the plugin's ids")
        );
        assert!(raw_work("p", "{\"rows\": 3}").is_err());
        assert!(raw_work("p", &" ".repeat(MAX_RAW_BYTES + 1)).is_err());
        assert_eq!(raw_work("p", "{}").unwrap(), Work::default());
    }
}
