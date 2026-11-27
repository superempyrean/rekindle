//! Frames the client sends to Discord's gateway, as plugin events.
//!
//! A platform hands each text frame the client sends to [`event`], which
//! reads the presence (op 3) and voice state (op 4) updates and nothing
//! else. Identify (op 2) and Resume (op 6) carry the session's token and
//! id, and [`event`] returns `None` for them before it parses anything, as
//! it does for heartbeats and every op it doesn't read. When an
//! `event-filter` replaced the event, [`frame`] writes it back into the
//! frame, keeping every field it doesn't change.

use serde_json::{Map, Value, json};

use crate::api::{self, Event};
use crate::runtime::redact;

/// The ops rekindle reads.
pub const PRESENCE: u64 = 3;
pub const VOICE_STATE: u64 = 4;
/// The ops that are never read or passed on.
pub const IDENTIFY: u64 = 2;
pub const RESUME: u64 = 6;

/// The `"op"` of a frame, read without parsing the rest.
#[must_use]
pub fn opcode(frame: &str) -> Option<u64> {
    let at = frame.find("\"op\"")? + 4;
    let rest = frame[at..].trim_start().strip_prefix(':')?.trim_start();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Whether plugins may see a frame with this op.
#[must_use]
pub fn readable(op: u64) -> bool {
    op == PRESENCE || op == VOICE_STATE
}

fn snowflake(v: &Value) -> Option<u64> {
    match v {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

/// The event a frame the client sends stands for, if plugins may see it.
#[must_use]
pub fn event(frame: &str) -> Option<Event> {
    let op = opcode(frame).filter(|&op| readable(op))?;
    let v: Value = serde_json::from_str(frame).ok()?;
    let d = v.get("d")?;
    match op {
        PRESENCE => {
            let mut activities = d.get("activities").cloned().unwrap_or(json!([]));
            redact(&mut activities);
            Some(Event::PresenceSent(api::OutgoingPresence {
                status: d.get("status")?.as_str()?.to_string(),
                afk: d.get("afk").and_then(Value::as_bool).unwrap_or(false),
                activities: activities.to_string(),
            }))
        }
        VOICE_STATE => Some(Event::VoiceStateSent(api::OutgoingVoiceState {
            guild: d.get("guild_id").and_then(snowflake),
            channel: d.get("channel_id").and_then(snowflake),
            self_mute: d.get("self_mute").and_then(Value::as_bool).unwrap_or(false),
            self_deaf: d.get("self_deaf").and_then(Value::as_bool).unwrap_or(false),
            self_video: d
                .get("self_video")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })),
        _ => None,
    }
}

fn id(n: Option<u64>) -> Value {
    n.map_or(Value::Null, |n| Value::String(n.to_string()))
}

/// An activity's `name` and `type`, which match a filter's activity to the
/// original whose `secrets` it keeps.
fn activity_key(a: &Value) -> (Option<&Value>, Option<&Value>) {
    (a.get("name"), a.get("type"))
}

/// `frame` with `ev`, an event of the same op that a filter returned,
/// written into its `d`. `None` if `ev` is of another op or its activities
/// aren't a JSON array.
#[must_use]
pub fn frame(frame: &str, ev: &Event) -> Option<String> {
    let op = opcode(frame).filter(|&op| readable(op))?;
    let mut v: Value = serde_json::from_str(frame).ok()?;
    let d = v.get_mut("d")?.as_object_mut()?;
    match (op, ev) {
        (PRESENCE, Event::PresenceSent(p)) => {
            let Value::Array(mut new) = serde_json::from_str(&p.activities).ok()? else {
                return None;
            };
            let old = d.get("activities").and_then(Value::as_array).cloned();
            for a in &mut new {
                let secrets = old.iter().flatten().find_map(|o| {
                    (activity_key(o) == activity_key(a))
                        .then(|| o.get("secrets").cloned())
                        .flatten()
                });
                if let (Some(s), Some(a)) = (secrets, a.as_object_mut()) {
                    a.insert("secrets".into(), s);
                }
            }
            d.insert("status".into(), Value::String(p.status.clone()));
            d.insert("afk".into(), Value::Bool(p.afk));
            d.insert("activities".into(), Value::Array(new));
        }
        (VOICE_STATE, Event::VoiceStateSent(s)) => {
            let fields: Map<String, Value> = [
                ("guild_id", id(s.guild)),
                ("channel_id", id(s.channel)),
                ("self_mute", Value::Bool(s.self_mute)),
                ("self_deaf", Value::Bool(s.self_deaf)),
                ("self_video", Value::Bool(s.self_video)),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
            d.extend(fields);
        }
        _ => return None,
    }
    Some(v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRESENCE_FRAME: &str = r#"{"op":3,"d":{"status":"online","since":0,"activities":[{"name":"Game","type":0,"secrets":{"join":"s3cr3t"}}],"afk":false}}"#;
    const VOICE_FRAME: &str = r#"{"op":4,"d":{"guild_id":"1","channel_id":"2","self_mute":false,"self_deaf":true,"self_video":false,"flags":2}}"#;

    #[test]
    fn identify_and_resume_are_never_read() {
        assert_eq!(
            event(r#"{"op":2,"d":{"token":"t","status":"online"}}"#),
            None
        );
        assert_eq!(
            event(r#"{"op":6,"d":{"token":"t","session_id":"s"}}"#),
            None
        );
        assert_eq!(event(r#"{"op":1,"d":42}"#), None);
        assert!(!readable(IDENTIFY) && !readable(RESUME));
    }

    #[test]
    fn a_presence_update_without_its_secrets() {
        let Some(Event::PresenceSent(p)) = event(PRESENCE_FRAME) else {
            panic!("no presence");
        };
        assert_eq!(p.status, "online");
        assert!(!p.afk);
        assert!(!p.activities.contains("s3cr3t"));
        assert!(p.activities.contains("Game"));
    }

    #[test]
    fn a_replaced_presence_keeps_the_secrets_and_other_fields() {
        let Some(Event::PresenceSent(mut p)) = event(PRESENCE_FRAME) else {
            panic!("no presence");
        };
        p.status = "dnd".into();
        let out: Value =
            serde_json::from_str(&frame(PRESENCE_FRAME, &Event::PresenceSent(p)).unwrap()).unwrap();
        assert_eq!(out["op"], 3);
        assert_eq!(out["d"]["status"], "dnd");
        assert_eq!(out["d"]["since"], 0);
        assert_eq!(out["d"]["activities"][0]["secrets"]["join"], "s3cr3t");

        let hidden = Event::PresenceSent(api::OutgoingPresence {
            status: "invisible".into(),
            afk: false,
            activities: "[]".into(),
        });
        let out: Value = serde_json::from_str(&frame(PRESENCE_FRAME, &hidden).unwrap()).unwrap();
        assert_eq!(out["d"]["activities"], json!([]));
    }

    #[test]
    fn a_voice_state_update() {
        let Some(Event::VoiceStateSent(mut s)) = event(VOICE_FRAME) else {
            panic!("no voice state");
        };
        assert_eq!((s.guild, s.channel), (Some(1), Some(2)));
        assert!(s.self_deaf && !s.self_mute);
        s.self_mute = true;
        s.channel = None;
        let out: Value =
            serde_json::from_str(&frame(VOICE_FRAME, &Event::VoiceStateSent(s)).unwrap()).unwrap();
        assert_eq!(out["d"]["self_mute"], true);
        assert_eq!(out["d"]["channel_id"], Value::Null);
        assert_eq!(out["d"]["flags"], 2);
    }

    #[test]
    fn another_ops_event_is_refused() {
        let Some(voice) = event(VOICE_FRAME) else {
            panic!("no voice state");
        };
        assert_eq!(frame(PRESENCE_FRAME, &voice), None);
        let bad = Event::PresenceSent(api::OutgoingPresence {
            status: "online".into(),
            afk: false,
            activities: "{}".into(),
        });
        assert_eq!(frame(PRESENCE_FRAME, &bad), None);
    }
}
