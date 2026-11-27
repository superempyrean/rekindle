//! Plugins' `http-filter`s on the phones, at the host-call boundary.
//!
//! The guest makes a REST request with `Networking.request`, a host call
//! whose record has the request's id, method and URL and whose body is the
//! request's body. The platform answers later with an `@http` EVENT that
//! carries the same id, the status and the response's body. [`on_call`]
//! runs the request past the plugins through [`embedded::net`], the path
//! desktop's transport hook takes, and [`on_event`] runs the answer past
//! them, so a plugin sees the same requests and responses on every
//! platform.
//!
//! A dropped request never reaches the platform. The guest gets an
//! `@http` answer with status 0 and an error in its place, from
//! [`take_answers`], as for a request that failed before any connection.
//!
//! The guest sends a gateway frame with `WebSocket.sendText`, whose body is
//! the frame. [`on_send`] runs it past the plugins' `outgoing` filters
//! through [`embedded::net`] as desktop's socket hook does, and hands it
//! on for its event. Identify and Resume are never read.
//!
//! The ids are the Android platform's (build 356207), which iOS shares.

use std::collections::VecDeque;
use std::sync::Mutex;

use rekindle_wire::frame::{self, Frame};
use rekindle_wire::{PropId, Value};

use crate::embedded::net::{self, Decision};

/// `Networking.request`.
pub(crate) const NETWORKING_REQUEST: u16 = 168;
/// `@http`, a request's answer.
pub(crate) const HTTP_EVENT: u16 = 321;
/// `WebSocket.sendText`.
pub(crate) const WEBSOCKET_SEND_TEXT: u16 = 293;
const ERROR: PropId = PropId(403);
const ID: PropId = PropId(417);
const METHOD: PropId = PropId(421);
const URL: PropId = PropId(422);
const STATUS: PropId = PropId(430);

/// How many filtered requests are kept to match answers against.
const KEPT: usize = 64;

/// What the platform does with a host call.
#[derive(Debug, PartialEq, Eq)]
pub enum Call {
    /// Make it as the guest made it.
    Pass,
    /// Return without making it.
    Drop,
    /// Make this call instead.
    Replace(Vec<u8>),
}

/// The guest's ids of the requests the plugins filter, with the ids
/// [`net::http_response`] knows them by.
static FILTERED: Mutex<VecDeque<(i64, u64)>> = Mutex::new(VecDeque::new());
/// Answers to dropped requests, for the next dispatch.
static ANSWERS: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn field(r: &Value, k: PropId) -> Option<&Value> {
    match r {
        Value::Object(f) => f.iter().find(|(p, _)| *p == k).map(|(_, v)| v),
        _ => None,
    }
}

fn text(r: &Value, k: PropId) -> Option<&str> {
    match field(r, k)? {
        Value::Str(s) => Some(s),
        _ => None,
    }
}

/// A host call the guest made, run past the plugins' `http-filter`s. On
/// the guest's thread.
pub fn on_call(buf: &[u8]) -> Call {
    on_call_with(buf, net::http_request)
}

fn on_call_with(
    buf: &[u8],
    filter: impl FnOnce(&str, &str, Option<&str>) -> (Decision, Option<u64>),
) -> Call {
    let Ok(mut f) = Frame::from_bytes(buf) else {
        return Call::Pass;
    };
    if f.kind != frame::CALL || f.method != NETWORKING_REQUEST {
        return Call::Pass;
    }
    let Ok(r) = f.record_value() else {
        return Call::Pass;
    };
    let (Some(id), Some(url)) = (field(&r, ID).and_then(Value::as_int), text(&r, URL)) else {
        return Call::Pass;
    };
    let method = text(&r, METHOD).unwrap_or("GET").to_ascii_uppercase();
    let body = (!f.body.is_empty())
        .then(|| std::str::from_utf8(&f.body).ok())
        .flatten();
    let (decision, filtered) = filter(&method, url, body);
    let Some(filtered) = filtered else {
        return Call::Pass;
    };
    match decision {
        Decision::Drop => {
            lock(&ANSWERS).push(failed(id));
            Call::Drop
        }
        decision => {
            {
                let mut held = lock(&FILTERED);
                if held.len() >= KEPT {
                    held.pop_front();
                }
                held.push_back((id, filtered));
            }
            match decision {
                Decision::Replace(new) => {
                    f.body = new.into_bytes();
                    Call::Replace(f.to_bytes())
                }
                _ => Call::Pass,
            }
        }
    }
}

/// A gateway frame the guest sends, run past the plugins' `outgoing`
/// filters. On the guest's thread.
pub fn on_send(buf: &[u8]) -> Call {
    on_send_with(buf, net::sending, net::sent)
}

fn on_send_with(
    buf: &[u8],
    sending: impl FnOnce(&str) -> Decision,
    sent: impl FnOnce(&str),
) -> Call {
    let Ok(mut f) = Frame::from_bytes(buf) else {
        return Call::Pass;
    };
    if f.kind != frame::CALL || f.method != WEBSOCKET_SEND_TEXT {
        return Call::Pass;
    }
    let Ok(text) = std::str::from_utf8(&f.body) else {
        return Call::Pass;
    };
    match sending(text) {
        Decision::Drop => Call::Drop,
        Decision::Replace(new) => {
            sent(&new);
            f.body = new.into_bytes();
            Call::Replace(f.to_bytes())
        }
        Decision::Pass => {
            sent(text);
            Call::Pass
        }
    }
}

/// The `@http` EVENT for a request that failed before any connection.
fn failed(id: i64) -> Vec<u8> {
    let id = i32::try_from(id).map_or(Value::I64(id), Value::I32);
    Frame::event(
        HTTP_EVENT,
        &Value::Object(vec![
            (ID, id),
            (STATUS, Value::I32(0)),
            (ERROR, Value::Str("dropped by a plugin".into())),
        ]),
    )
    .to_bytes()
}

/// The answers to the requests dropped since the last call.
pub fn take_answers() -> Vec<Vec<u8>> {
    std::mem::take(&mut *lock(&ANSWERS))
}

/// An event on its way to the guest, with the answers to filtered requests
/// run past the plugins' `http-filter`s. `Some` with the event to hand the
/// guest instead.
pub fn on_event(buf: &[u8]) -> Option<Vec<u8>> {
    on_event_with(buf, &mut net::http_response)
}

fn answer(
    f: &Frame,
    response: &mut impl FnMut(u64, u16, Option<&str>) -> Option<String>,
) -> Option<Frame> {
    if f.kind != frame::EVENT || f.method != HTTP_EVENT {
        return None;
    }
    let r = f.record_value().ok()?;
    let id = field(&r, ID).and_then(Value::as_int)?;
    let filtered = {
        let mut held = lock(&FILTERED);
        let at = held.iter().position(|(g, _)| *g == id)?;
        held.remove(at)?.1
    };
    let status = field(&r, STATUS)
        .and_then(Value::as_int)
        .and_then(|s| u16::try_from(s).ok())
        .unwrap_or(0);
    let body = std::str::from_utf8(&f.body).ok();
    let new = response(filtered, status, body)?;
    let mut f = f.clone();
    f.body = new.into_bytes();
    Some(f)
}

fn on_event_with(
    buf: &[u8],
    response: &mut impl FnMut(u64, u16, Option<&str>) -> Option<String>,
) -> Option<Vec<u8>> {
    if lock(&FILTERED).is_empty() {
        return None;
    }
    let f = Frame::from_bytes(buf).ok()?;
    if f.kind != frame::BATCH {
        return answer(&f, response).map(|f| f.to_bytes());
    }
    let entries = f.batch().ok()?;
    let mut changed = false;
    let out: Vec<Vec<u8>> = entries
        .iter()
        .map(
            |e| match Frame::from_bytes(e).ok().and_then(|x| answer(&x, response)) {
                Some(new) => {
                    changed = true;
                    new.to_bytes()
                }
                None => e.to_vec(),
            },
        )
        .collect();
    changed.then(|| {
        let mut batch = f.clone();
        batch.body = frame::batch_body(out.iter().map(Vec::as_slice));
        batch.to_bytes()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(id: i32, method: &str, url: &str, body: &str) -> Vec<u8> {
        let mut f = Frame::call(
            9,
            NETWORKING_REQUEST,
            1,
            &Value::Object(vec![
                (ID, Value::I32(id)),
                (METHOD, Value::Str(method.into())),
                (URL, Value::Str(url.into())),
            ]),
        );
        f.body = body.as_bytes().to_vec();
        f.to_bytes()
    }

    fn http_answer(id: i32, status: i32, body: &str) -> Vec<u8> {
        let mut f = Frame::event(
            HTTP_EVENT,
            &Value::Object(vec![(ID, Value::I32(id)), (STATUS, Value::I32(status))]),
        );
        f.body = body.as_bytes().to_vec();
        f.to_bytes()
    }

    const URL_: &str = "https://discord.com/api/v9/channels/1/messages";

    #[test]
    fn a_request_no_plugin_filters_passes() {
        let b = request(1, "POST", URL_, "{}");
        assert_eq!(
            on_call_with(&b, |_, _, _| (Decision::Pass, None)),
            Call::Pass
        );
        // Another host call is never read.
        let mut other = Frame::from_bytes(&b).unwrap();
        other.method = 169;
        assert_eq!(
            on_call_with(&other.to_bytes(), |_, _, _| panic!("read")),
            Call::Pass
        );
    }

    #[test]
    fn a_request_body_is_replaced_and_its_answer_rewritten() {
        let b = request(101, "post", URL_, r#"{"content":"hi"}"#);
        let call = on_call_with(&b, |m, u, body| {
            assert_eq!((m, u, body), ("POST", URL_, Some(r#"{"content":"hi"}"#)));
            (Decision::Replace(r#"{"content":"HI"}"#.into()), Some(7))
        });
        let Call::Replace(new) = call else {
            panic!("{call:?}");
        };
        let f = Frame::from_bytes(&new).unwrap();
        assert_eq!(f.body, br#"{"content":"HI"}"#);
        assert_eq!(f.record, Frame::from_bytes(&b).unwrap().record);

        // The answer, alone in a batch with another event.
        let other = Frame::event(5, &Value::Object(vec![])).to_bytes();
        let ans = http_answer(101, 200, r#"{"id":"1"}"#);
        let batch = Frame::batch_of([other.as_slice(), ans.as_slice()]).to_bytes();
        let out = on_event_with(&batch, &mut |id, status, body| {
            assert_eq!((id, status, body), (7, 200, Some(r#"{"id":"1"}"#)));
            Some(r#"{"id":"2"}"#.into())
        })
        .unwrap();
        let f = Frame::from_bytes(&out).unwrap();
        let entries = f.batch().unwrap();
        assert_eq!(entries[0], other.as_slice());
        assert_eq!(
            Frame::from_bytes(entries[1]).unwrap().body,
            br#"{"id":"2"}"#
        );
        // Answered once.
        assert_eq!(on_event_with(&ans, &mut |_, _, _| panic!("again")), None);
    }

    fn socket_frame(text: &str) -> Vec<u8> {
        let mut f = Frame::call(
            9,
            WEBSOCKET_SEND_TEXT,
            1,
            &Value::Object(vec![(ID, Value::I32(3))]),
        );
        f.body = text.as_bytes().to_vec();
        f.to_bytes()
    }

    #[test]
    fn a_gateway_frame_goes_past_the_outgoing_filters() {
        let presence = r#"{"op":3,"d":{"status":"online","afk":false}}"#;
        let mut seen = String::new();
        let call = on_send_with(
            &socket_frame(presence),
            |t| {
                assert_eq!(t, presence);
                Decision::Replace(r#"{"op":3,"d":{"status":"dnd","afk":false}}"#.into())
            },
            |t| seen = t.to_string(),
        );
        let Call::Replace(new) = call else {
            panic!("{call:?}");
        };
        let f = Frame::from_bytes(&new).unwrap();
        assert_eq!(f.body, br#"{"op":3,"d":{"status":"dnd","afk":false}}"#);
        assert!(seen.contains("dnd"));

        let voice = r#"{"op":4,"d":{}}"#;
        assert_eq!(
            on_send_with(&socket_frame(voice), |_| Decision::Drop, |_| panic!("sent")),
            Call::Drop
        );
        // A request is not a frame.
        let req = request(5, "GET", URL_, "");
        assert_eq!(
            on_send_with(&req, |_| panic!("read"), |_| panic!("sent")),
            Call::Pass
        );
    }

    #[test]
    fn a_dropped_request_is_answered_as_failed() {
        let b = request(202, "DELETE", URL_, "");
        assert_eq!(
            on_call_with(&b, |m, _, body| {
                assert_eq!((m, body), ("DELETE", None));
                (Decision::Drop, Some(8))
            }),
            Call::Drop
        );
        let answers = take_answers();
        let a = answers
            .iter()
            .map(|a| Frame::from_bytes(a).unwrap())
            .find(|a| field(&a.record_value().unwrap(), ID).and_then(Value::as_int) == Some(202))
            .unwrap();
        let r = a.record_value().unwrap();
        assert_eq!(field(&r, STATUS).and_then(Value::as_int), Some(0));
        assert_eq!(text(&r, ERROR), Some("dropped by a plugin"));
    }
}
