//! Gateway dispatches in the host frames a split client's guest receives.
//!
//! The guest opens the gateway through the platform's `WebSocket` host
//! module, and every message on a socket reaches it as an EVENT frame,
//! `@ws` (378), on its event method. The frame's record names the socket
//! (`id`, 417) and what happened (`event`, 443: `open`, `text`, `binary`,
//! `close` or `error`), and the message is the frame's body. A gateway
//! message is the JSON `{"op", "d", "s", "t"}`, and a dispatch is op 0 with
//! its name in `t`. The platform may send these frames on their own or in a
//! BATCH with UI events and other frames.
//!
//! [`filter`] finds the dispatches in what the platform dispatches, runs
//! the ones a plugin intercepts past a decision, and rewrites the buffer
//! when one is dropped or replaced. A dispatch that is never forwarded
//! ([`gateway::forwarded`]) is read only for its name, so its `d`, which
//! holds the session and the user's credentials, is never parsed.

use rekindle_plugin_host::backend::gateway;
use rekindle_wire::frame::{self, Frame};
use rekindle_wire::{PropId, Value, event};
use serde::Deserialize;

/// `@ws`, a socket's events.
pub(crate) const WS_EVENT: u16 = 378;
/// The record's `id`: which socket.
const ID: PropId = PropId(417);
/// The record's `event`: what happened on it.
const EVENT: PropId = PropId(443);

/// What a plugin decided about a dispatch.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Decision {
    Pass,
    /// The guest receives the dispatch with this `d`.
    Replace(serde_json::Value),
    /// The guest never receives it.
    Drop,
}

/// A dispatch the guest receives, for the events it causes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Passed {
    pub(crate) t: String,
    /// The gateway message, as the guest receives it.
    pub(crate) message: Vec<u8>,
}

impl Passed {
    /// The dispatch's `d`, or null for one that is never forwarded.
    pub(crate) fn d(&self) -> serde_json::Value {
        #[derive(Deserialize)]
        struct D {
            #[serde(default)]
            d: serde_json::Value,
        }
        if !gateway::forwarded(&self.t) {
            return serde_json::Value::Null;
        }
        serde_json::from_slice::<D>(&self.message).map_or(serde_json::Value::Null, |m| m.d)
    }
}

/// The socket the gateway's dispatches arrive on, from the last one seen:
/// its `id` and the last sequence number.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Socket {
    pub(crate) id: Value,
    pub(crate) seq: Option<i64>,
}

/// What [`filter`] made of one buffer.
#[derive(Debug, Default)]
pub(crate) struct Filtered {
    pub(crate) change: Change,
    /// The dispatches the guest receives, in order.
    pub(crate) passed: Vec<Passed>,
    /// The names of the dispatches the plugins dropped.
    pub(crate) dropped: Vec<String>,
    pub(crate) replaced: usize,
    pub(crate) socket: Option<Socket>,
    /// Socket messages whose body isn't a gateway message, by name of the
    /// socket event: another socket's, or a gateway compressed in a way the
    /// platform didn't inflate.
    pub(crate) unread: Vec<(String, usize)>,
}

/// What happens to a buffer.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) enum Change {
    /// The guest receives it as it is.
    #[default]
    Keep,
    /// The guest receives this instead.
    Bytes(Vec<u8>),
    /// The guest receives nothing.
    Drop,
}

/// Find the dispatches in `buf`, and ask `decide` about each one `wants`
/// names. `decide` gets the dispatch's name and `d`.
pub(crate) fn filter(
    buf: &[u8],
    wants: &dyn Fn(&str) -> bool,
    decide: &mut dyn FnMut(&str, &serde_json::Value) -> Decision,
) -> Filtered {
    let mut out = Filtered::default();
    out.change = walk(buf, wants, decide, &mut out);
    out
}

fn walk(
    buf: &[u8],
    wants: &dyn Fn(&str) -> bool,
    decide: &mut dyn FnMut(&str, &serde_json::Value) -> Decision,
    out: &mut Filtered,
) -> Change {
    if event::is_ui(buf) {
        return Change::Keep;
    }
    let Ok(f) = Frame::from_bytes(buf) else {
        return Change::Keep;
    };
    match f.kind {
        frame::BATCH => {
            let Ok(entries) = f.batch() else {
                return Change::Keep;
            };
            let mut kept: Vec<Vec<u8>> = Vec::with_capacity(entries.len());
            let mut changed = false;
            for e in entries {
                match walk(e, wants, decide, out) {
                    Change::Keep => kept.push(e.to_vec()),
                    Change::Bytes(b) => {
                        kept.push(b);
                        changed = true;
                    }
                    Change::Drop => changed = true,
                }
            }
            if !changed {
                Change::Keep
            } else if kept.is_empty() {
                Change::Drop
            } else {
                let body = frame::batch_body(kept.iter().map(Vec::as_slice));
                Change::Bytes(Frame { body, ..f }.to_bytes())
            }
        }
        frame::EVENT if f.method == WS_EVENT => socket_message(f, wants, decide, out),
        _ => Change::Keep,
    }
}

/// The head of a gateway message: everything but `d`, which is skipped.
#[derive(Deserialize)]
struct Head {
    op: u8,
    #[serde(default)]
    t: Option<String>,
    #[serde(default)]
    s: Option<i64>,
}

fn socket_message(
    f: Frame,
    wants: &dyn Fn(&str) -> bool,
    decide: &mut dyn FnMut(&str, &serde_json::Value) -> Decision,
    out: &mut Filtered,
) -> Change {
    let Ok(Value::Object(record)) = f.record_value() else {
        return Change::Keep;
    };
    let field = |p: PropId| record.iter().find(|(k, _)| *k == p).map(|(_, v)| v);
    let Some(Value::Str(name)) = field(EVENT) else {
        return Change::Keep;
    };
    if name != "text" && name != "binary" {
        return Change::Keep;
    }
    let Ok(head) = serde_json::from_slice::<Head>(&f.body) else {
        out.unread.push((name.clone(), f.body.len()));
        return Change::Keep;
    };
    let (0, Some(t)) = (head.op, head.t) else {
        return Change::Keep;
    };
    if let Some(id) = field(ID) {
        let seq = head.s.or(out.socket.as_ref().and_then(|s| s.seq));
        out.socket = Some(Socket {
            id: id.clone(),
            seq,
        });
    }
    let mut replaced = None;
    if gateway::forwarded(&t)
        && wants(&t)
        && let Ok(mut m) = serde_json::from_slice::<serde_json::Map<_, _>>(&f.body)
    {
        let d = m.remove("d").unwrap_or_default();
        match decide(&t, &d) {
            Decision::Pass => {}
            Decision::Drop => {
                out.dropped.push(t);
                return Change::Drop;
            }
            Decision::Replace(d) => {
                m.insert("d".into(), d);
                replaced = serde_json::to_vec(&m).ok();
            }
        }
    }
    let Some(message) = replaced else {
        out.passed.push(Passed { t, message: f.body });
        return Change::Keep;
    };
    out.replaced += 1;
    out.passed.push(Passed {
        t,
        message: message.clone(),
    });
    Change::Bytes(Frame { body: message, ..f }.to_bytes())
}

/// A gateway message on `socket` as the platform hands one to the guest:
/// an `@ws` text event.
pub(crate) fn socket_event(socket: &Socket, message: &[u8]) -> Vec<u8> {
    let record = Value::Object(vec![
        (ID, socket.id.clone()),
        (EVENT, Value::Str("text".into())),
    ]);
    Frame {
        body: message.to_vec(),
        ..Frame::event(WS_EVENT, &record)
    }
    .to_bytes()
}

/// The gateway message of dispatch `t` with `d`, numbered `seq`.
pub(crate) fn dispatch_message(t: &str, d: &serde_json::Value, seq: Option<i64>) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({ "op": 0, "t": t, "s": seq, "d": d }))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn socket() -> Socket {
        Socket {
            id: Value::I64(7),
            seq: Some(41),
        }
    }

    fn message(content: &str) -> Vec<u8> {
        dispatch_message(
            "MESSAGE_CREATE",
            &json!({"id": "1", "channel_id": "2", "content": content}),
            Some(42),
        )
    }

    fn mute(word: &'static str) -> impl FnMut(&str, &serde_json::Value) -> Decision {
        move |_, d| {
            if d["content"].as_str().is_some_and(|c| c.contains(word)) {
                Decision::Drop
            } else {
                Decision::Pass
            }
        }
    }

    #[test]
    fn a_dropped_dispatch_leaves_the_rest_of_the_batch() {
        let press = rekindle_wire::event::Event::press(5).to_bytes();
        let muted = socket_event(&socket(), &message("a spoiler"));
        let fine = socket_event(&socket(), &message("hello"));
        let batch = Frame::batch_of([press.as_slice(), &muted, &fine]).to_bytes();
        let f = filter(&batch, &|t| t == "MESSAGE_CREATE", &mut mute("spoiler"));
        assert_eq!(f.dropped, ["MESSAGE_CREATE"]);
        let Change::Bytes(b) = &f.change else {
            panic!("{:?}", f.change)
        };
        let entries: Vec<Vec<u8>> = Frame::from_bytes(b)
            .unwrap()
            .batch()
            .unwrap()
            .into_iter()
            .map(<[u8]>::to_vec)
            .collect();
        assert_eq!(entries, [press, fine]);
        assert_eq!(f.passed.len(), 1);
        assert_eq!(f.passed[0].d()["content"], "hello");
        assert_eq!(
            f.socket,
            Some(Socket {
                id: Value::I64(7),
                seq: Some(42)
            })
        );
        // One on its own goes entirely.
        let alone = filter(&muted, &|_| true, &mut mute("spoiler"));
        assert_eq!(alone.change, Change::Drop);
    }

    #[test]
    fn a_replaced_dispatch_keeps_the_rest_of_the_message() {
        let ev = socket_event(&socket(), &message("teh cat"));
        let f = filter(&ev, &|_| true, &mut |_, d| {
            let mut d = d.clone();
            d["content"] = json!("the cat");
            Decision::Replace(d)
        });
        let Change::Bytes(b) = f.change else { panic!() };
        let frame = Frame::from_bytes(&b).unwrap();
        let m: serde_json::Value = serde_json::from_slice(&frame.body).unwrap();
        assert_eq!(m["d"]["content"], "the cat");
        assert_eq!((m["op"].as_u64(), m["s"].as_i64()), (Some(0), Some(42)));
        assert_eq!(frame.record, Frame::from_bytes(&ev).unwrap().record);
        assert_eq!(f.replaced, 1);
    }

    #[test]
    fn what_no_plugin_intercepts_is_only_read_for_its_name() {
        let ready = socket_event(
            &socket(),
            &dispatch_message("READY", &json!({"session_id": "SECRET"}), Some(1)),
        );
        let mut asked = Vec::new();
        let f = filter(&ready, &|_| true, &mut |t, _| {
            asked.push(t.to_string());
            Decision::Drop
        });
        assert!(asked.is_empty(), "READY is never forwarded");
        assert_eq!(f.change, Change::Keep);
        assert_eq!(f.passed[0].t, "READY");
        assert_eq!(f.passed[0].d(), serde_json::Value::Null);
        // A kind nobody wants passes without a decision.
        let ev = socket_event(&socket(), &message("x"));
        let f = filter(&ev, &|_| false, &mut |_, _| Decision::Drop);
        assert_eq!((f.change, f.passed.len()), (Change::Keep, 1));
    }

    #[test]
    fn other_socket_messages_pass() {
        let hello = socket_event(&socket(), br#"{"op":10,"d":{"heartbeat_interval":41250}}"#);
        let binary = Frame {
            body: vec![0x28, 0xb5, 0x2f, 0xfd],
            ..Frame::event(
                WS_EVENT,
                &Value::Object(vec![
                    (ID, Value::I64(7)),
                    (EVENT, Value::Str("binary".into())),
                ]),
            )
        }
        .to_bytes();
        for buf in [hello, binary] {
            let f = filter(&buf, &|_| true, &mut |_, _| Decision::Drop);
            assert_eq!(f.change, Change::Keep);
            assert!(f.passed.is_empty());
        }
    }
}
