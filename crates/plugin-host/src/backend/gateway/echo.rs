//! A message the client dispatches twice.
//!
//! When the user sends a message, the desktop client dispatches
//! `MESSAGE_CREATE` once from the REST reply to its `POST`, then again when
//! the gateway's copy arrives. Both carry the same message `id`. Plugins
//! receive one `message-created` for it, and their event filters see it
//! once: the second copy takes the first one's verdict. A dropped message
//! is dropped both times, and fields a filter changed in the first are
//! written over the second.
//!
//! A client that dispatches each message once never meets a repeated id,
//! so this changes nothing there.

use std::collections::VecDeque;

use serde_json::{Map, Value};

/// How many recent messages are remembered.
const KEPT: usize = 64;

/// The dispatch that comes twice.
const TWICE: &str = "MESSAGE_CREATE";

/// What the filters made of a message's first copy.
struct Verdict {
    id: String,
    /// `None`: dropped. Otherwise the top-level fields the filters changed,
    /// with their new values.
    changed: Option<Map<String, Value>>,
}

/// Recent `MESSAGE_CREATE`s, by id.
#[derive(Default)]
pub struct Echoes {
    filtered: VecDeque<Verdict>,
    delivered: VecDeque<String>,
}

fn id(t: &str, d: &Value) -> Option<String> {
    (t == TWICE)
        .then(|| d.get("id")?.as_str().map(str::to_string))
        .flatten()
}

fn keep<T>(q: &mut VecDeque<T>, item: T) {
    if q.len() >= KEPT {
        q.pop_front();
    }
    q.push_back(item);
}

impl Echoes {
    /// The verdict for a second copy of `d`, if this is one: `Some(None)` to
    /// drop it, `Some(Some(d))` to let it through as `d`. `None` for a
    /// dispatch the filters haven't seen, which goes to [`Echoes::filtered`]
    /// once they have.
    pub fn repeat(&self, t: &str, d: &Value) -> Option<Option<Value>> {
        let id = id(t, d)?;
        let v = self.filtered.iter().find(|v| v.id == id)?;
        Some(v.changed.as_ref().map(|changed| {
            let mut d = d.clone();
            if let Some(o) = d.as_object_mut() {
                o.extend(changed.clone());
            }
            d
        }))
    }

    /// The filters turned `before` into `after` (`None`: dropped).
    pub fn filtered(&mut self, t: &str, before: &Value, after: Option<&Value>) {
        let Some(id) = id(t, before) else { return };
        let changed = after.map(|after| {
            let mut changed = Map::new();
            if let (Some(a), Some(b)) = (after.as_object(), before.as_object()) {
                for (k, v) in a {
                    if b.get(k) != Some(v) {
                        changed.insert(k.clone(), v.clone());
                    }
                }
            }
            changed
        });
        keep(&mut self.filtered, Verdict { id, changed });
    }

    /// Whether the events of `d` reach the plugins: false for a second copy
    /// of a message whose events were delivered.
    pub fn deliver(&mut self, t: &str, d: &Value) -> bool {
        let Some(id) = id(t, d) else { return true };
        if self.delivered.contains(&id) {
            return false;
        }
        keep(&mut self.delivered, id);
        true
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_message_is_delivered_once() {
        let mut e = Echoes::default();
        let rest = json!({"id": "7", "content": "hi", "position": null});
        let gateway = json!({"id": "7", "content": "hi", "channel_type": 0});
        assert!(e.deliver("MESSAGE_CREATE", &rest));
        assert!(!e.deliver("MESSAGE_CREATE", &gateway));
        assert!(e.deliver("MESSAGE_CREATE", &json!({"id": "8"})));
        assert!(e.deliver("MESSAGE_UPDATE", &rest));
        assert!(e.deliver("MESSAGE_UPDATE", &rest));
    }

    #[test]
    fn the_second_copy_takes_the_first_ones_verdict() {
        let mut e = Echoes::default();
        let first = json!({"id": "7", "content": "hi", "position": null});
        let second = json!({"id": "7", "content": "hi", "channel_type": 0});
        assert_eq!(e.repeat("MESSAGE_CREATE", &first), None);
        let mut after = first.clone();
        after["content"] = json!("HI");
        e.filtered("MESSAGE_CREATE", &first, Some(&after));
        assert_eq!(
            e.repeat("MESSAGE_CREATE", &second),
            Some(Some(json!({"id": "7", "content": "HI", "channel_type": 0})))
        );

        let dropped = json!({"id": "9"});
        e.filtered("MESSAGE_CREATE", &dropped, None);
        assert_eq!(e.repeat("MESSAGE_CREATE", &dropped), Some(None));
    }

    #[test]
    fn only_recent_messages_are_kept() {
        let mut e = Echoes::default();
        for n in 0..=KEPT {
            assert!(e.deliver("MESSAGE_CREATE", &json!({"id": n.to_string()})));
        }
        assert!(e.deliver("MESSAGE_CREATE", &json!({"id": "0"})));
    }
}
