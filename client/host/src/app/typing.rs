//! The `silent-typing` tweak: the guest's typing indicators never leave the
//! phone.
//!
//! The guest tells Discord the user is typing with `POST
//! /api/v{n}/channels/{id}/typing`, a `Networking.request` host call. The
//! platform makes the request and answers later with an `@http` EVENT that
//! carries the request's id. With the tweak on, [`drops`] picks out that
//! call, the platform's hook returns without making it, and the next
//! dispatch hands the guest a `204` for it ([`take_answers`]), as Discord
//! would have sent.
//!
//! The method, event and field ids are the Android platform's (build
//! 356207). Nothing is dropped until a real `@http` answer to a real
//! request has passed, which shows this build uses the same ids.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rekindle_extension::tweaks::{self, SILENT_TYPING};
use rekindle_wire::frame::{self, Frame};
use rekindle_wire::{PropId, Value};

/// `Networking.request`.
const NETWORKING_REQUEST: u16 = 168;
/// `@http`, a request's answer.
const HTTP_EVENT: u16 = 321;
const ID: PropId = PropId(417);
const METHOD: PropId = PropId(421);
const URL: PropId = PropId(422);
const STATUS: PropId = PropId(430);

/// How many request ids are kept to match answers against.
const PENDING: usize = 64;

/// The ids of the guest's last requests.
static REQUESTS: Mutex<VecDeque<i64>> = Mutex::new(VecDeque::new());
/// A real answer matched a real request.
static CONFIRMED: AtomicBool = AtomicBool::new(false);
/// Answers to dropped requests, for the next dispatch.
static ANSWERS: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
static DROPPED: AtomicU64 = AtomicU64::new(0);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A host call the guest made. True if the platform should return without
/// making it. On the guest's thread.
pub(super) fn drops(buf: &[u8]) -> bool {
    let Some((id, method, url)) = request(buf) else {
        return false;
    };
    if !is_typing(&method, &url) || !tweaks::on(SILENT_TYPING) {
        let mut r = lock(&REQUESTS);
        if r.len() == PENDING {
            r.pop_front();
        }
        r.push_back(id);
        return false;
    }
    if !CONFIRMED.load(Ordering::Relaxed) {
        super::log("silent-typing: no request answered yet on this build; typing sent");
        return false;
    }
    lock(&ANSWERS).push(answer(id));
    let n = DROPPED.fetch_add(1, Ordering::Relaxed);
    if n < 4 || n.is_multiple_of(64) {
        super::log(&format!(
            "silent-typing: typing request #{id} dropped ({} so far)",
            n + 1
        ));
    }
    true
}

/// An event on its way to the guest. An `@http` answer to one of the
/// guest's requests confirms the ids.
pub(super) fn on_event(buf: &[u8]) {
    if CONFIRMED.load(Ordering::Relaxed) {
        return;
    }
    let Ok(f) = Frame::from_bytes(buf) else {
        return;
    };
    let answered = |f: &Frame| {
        f.kind == frame::EVENT
            && f.method == HTTP_EVENT
            && f.record_value()
                .ok()
                .and_then(|r| int(&r, ID))
                .is_some_and(|id| lock(&REQUESTS).contains(&id))
    };
    let hit = if f.kind == frame::BATCH {
        f.batch()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|e| Frame::from_bytes(e).ok())
            .any(|e| answered(&e))
    } else {
        answered(&f)
    };
    if hit {
        CONFIRMED.store(true, Ordering::Relaxed);
        super::log("silent-typing: request answers match this build's ids");
    }
}

/// The answers to the requests dropped since the last call.
pub(super) fn take_answers() -> Vec<Vec<u8>> {
    std::mem::take(&mut *lock(&ANSWERS))
}

/// A `Networking.request` CALL's id, method and URL.
fn request(buf: &[u8]) -> Option<(i64, String, String)> {
    let f = Frame::from_bytes(buf).ok()?;
    if f.kind != frame::CALL || f.method != NETWORKING_REQUEST {
        return None;
    }
    let r = f.record_value().ok()?;
    let id = int(&r, ID)?;
    let method = text(&r, METHOD).unwrap_or("GET").to_string();
    let url = text(&r, URL)?.to_string();
    Some((id, method, url))
}

/// `POST` to `…/channels/{id}/typing`.
fn is_typing(method: &str, url: &str) -> bool {
    if !method.eq_ignore_ascii_case("POST") {
        return false;
    }
    let path = url.split(['?', '#']).next().unwrap_or_default();
    let mut seg = path.trim_end_matches('/').rsplit('/');
    seg.next() == Some("typing")
        && seg
            .next()
            .is_some_and(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_digit()))
        && seg.next() == Some("channels")
}

/// The `@http` EVENT Discord's `204 No Content` becomes.
fn answer(id: i64) -> Vec<u8> {
    let id = i32::try_from(id).map_or(Value::I64(id), Value::I32);
    Frame::event(
        HTTP_EVENT,
        &Value::Object(vec![(ID, id), (STATUS, Value::I32(204))]),
    )
    .to_bytes()
}

fn field(r: &Value, k: PropId) -> Option<&Value> {
    match r {
        Value::Object(f) => f.iter().find(|(p, _)| *p == k).map(|(_, v)| v),
        _ => None,
    }
}

fn int(r: &Value, k: PropId) -> Option<i64> {
    field(r, k).and_then(Value::as_int)
}

fn text(r: &Value, k: PropId) -> Option<&str> {
    match field(r, k)? {
        Value::Str(s) => Some(s),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(method: &str, url: &str, id: i32) -> Vec<u8> {
        Frame::call(
            9,
            NETWORKING_REQUEST,
            1,
            &Value::Object(vec![
                (ID, Value::I32(id)),
                (METHOD, Value::Str(method.into())),
                (URL, Value::Str(url.into())),
            ]),
        )
        .to_bytes()
    }

    #[test]
    fn only_a_post_to_a_channels_typing_path_is_typing() {
        let u = "https://discord.com/api/v9/channels/123456789012345678/typing";
        assert!(is_typing("POST", u));
        assert!(is_typing("post", &format!("{u}?x=1")));
        assert!(!is_typing("GET", u));
        assert!(!is_typing(
            "POST",
            "https://discord.com/api/v9/channels/1/messages"
        ));
        assert!(!is_typing(
            "POST",
            "https://discord.com/api/v9/channels/abc/typing"
        ));
        assert!(!is_typing(
            "POST",
            "https://discord.com/api/v9/users/1/typing"
        ));
    }

    #[test]
    fn a_request_call_is_read_and_answered_with_a_204() {
        let b = call("POST", "https://discord.com/api/v9/channels/7/typing", 42);
        let (id, m, u) = request(&b).unwrap();
        assert_eq!((id, m.as_str()), (42, "POST"));
        assert!(is_typing(&m, &u));
        let a = Frame::from_bytes(&answer(42)).unwrap();
        assert_eq!((a.kind, a.method), (frame::EVENT, HTTP_EVENT));
        let r = a.record_value().unwrap();
        assert_eq!((int(&r, ID), int(&r, STATUS)), (Some(42), Some(204)));
        // Another module method is not a request.
        let mut other = Frame::from_bytes(&b).unwrap();
        other.method = 169;
        assert!(request(&other.to_bytes()).is_none());
    }
}
