//! The guest's cache store, read through the host module that keeps it.
//!
//! The guest keeps its state in `Database`, a host module of tables of
//! string keys and byte values. On Android the module keeps each store in
//! `cache.sqlite`. Plugins' `client-state` queries read it here, through
//! `rekindle_plugin_host::backend::cache`, with calls of rekindle's own into
//! the same module: `get`, `getMany` and `scan` on the handle the guest
//! opened. Nothing read is kept.
//!
//! The guest opens its store with `Database.open` (`name`, the user's id,
//! and `tables`) and gets a `handle` back. [`on_call`] and [`on_reply`] see
//! that pair go by and keep the handle until the guest closes it.
//!
//! A host call has to be made on the guest's thread, where the platform's
//! modules expect it. A query from the runtime's thread waits for the guest's
//! thread to come through one of the hooks ([`serve`]) or through a wait on
//! the runtime ([`wait`]), up to [`STORE_WAIT`].

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use rekindle_plugin_host::backend::cache::StoreReader;
use rekindle_wire::frame::{self, Frame};
use rekindle_wire::{PropId, Value};

/// The module's id, and its methods'.
const MODULE: u16 = 22;
const OPEN: u16 = 379;
const GET: u16 = 380;
const SCAN: u16 = 381;
const CLOSE: u16 = 387;
const DESTROY: u16 = 388;
const GET_MANY: u16 = 2838;

/// Record keys, from the host's id table.
const NAME: PropId = PropId(394);
const HANDLE: PropId = PropId(400);
const TABLE: PropId = PropId(404);
const KEY: PropId = PropId(405);
const FOUND: PropId = PropId(406);
const PREFIX: PropId = PropId(407);
const KEYS_ONLY: PropId = PropId(408);
const ERROR: PropId = PropId(403);
const KEYS: PropId = PropId(2796);

/// How long a query from another thread waits for the guest's thread.
pub(crate) const STORE_WAIT: Duration = Duration::from_millis(500);
/// How often a wait on the runtime looks for queries to make.
const SLICE: Duration = Duration::from_millis(2);
/// rekindle's calls carry ids of their own, apart from the guest's.
const CALL_IDS: u32 = 0x5245_0000;

/// The guest's open store: its name and handle.
static STORE: Mutex<Option<(String, i32)>> = Mutex::new(None);
/// The name of the store the guest is opening, by the open's call id.
static OPENING: Mutex<Option<(u32, String)>> = Mutex::new(None);
/// A query waiting for the guest's thread: a CALL frame, and where its
/// REPLY goes.
type Job = (Vec<u8>, mpsc::SyncSender<Option<Vec<u8>>>);
static JOBS: Mutex<VecDeque<Job>> = Mutex::new(VecDeque::new());
/// The guest's thread, once a hook has run on it.
static GUEST_THREAD: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);
static NEXT_CALL: AtomicU32 = AtomicU32::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn on_guest_thread() -> bool {
    *lock(&GUEST_THREAD) == Some(std::thread::current().id())
}

fn field(record: &Value, k: PropId) -> Option<&Value> {
    match record {
        Value::Object(fields) => fields.iter().find(|(p, _)| *p == k).map(|(_, v)| v),
        _ => None,
    }
}

/// A host call the guest made: an open, a close or a destroy of its store.
pub(crate) fn on_call(buf: &[u8]) {
    let Ok(f) = Frame::from_bytes(buf) else {
        return;
    };
    if f.kind != frame::CALL || f.module != MODULE {
        return;
    }
    let Ok(record) = f.record_value() else {
        return;
    };
    match f.method {
        OPEN => {
            if let Some(Value::Str(name)) = field(&record, NAME) {
                *lock(&OPENING) = Some((f.call, name.clone()));
            }
        }
        CLOSE => {
            let closed = field(&record, HANDLE).and_then(Value::as_int);
            let mut store = lock(&STORE);
            if store
                .as_ref()
                .is_some_and(|(_, h)| Some(i64::from(*h)) == closed)
            {
                *store = None;
            }
        }
        DESTROY => *lock(&STORE) = None,
        _ => {}
    }
}

/// The reply to a host call the guest made: the handle of the store it
/// opened.
pub(crate) fn on_reply(reply: &[u8]) {
    let Ok(f) = Frame::from_bytes(reply) else {
        return;
    };
    if f.kind != frame::REPLY || f.module != MODULE || f.method != OPEN {
        return;
    }
    let Some((call, name)) = lock(&OPENING).take() else {
        return;
    };
    let handle = f
        .record_value()
        .ok()
        .and_then(|r| field(&r, HANDLE).and_then(Value::as_int))
        .and_then(|h| i32::try_from(h).ok());
    if let (true, Some(h)) = (call == f.call, handle) {
        super::log(&format!("store: the guest opened its store as handle {h}"));
        *lock(&STORE) = Some((name, h));
    }
}

/// Make the queries waiting for the guest's thread. On the guest's thread,
/// from the platform's hooks.
pub(crate) fn serve() {
    *lock(&GUEST_THREAD) = Some(std::thread::current().id());
    loop {
        let Some((call, answer)) = lock(&JOBS).pop_front() else {
            return;
        };
        let _ = answer.send(super::call(&call));
    }
}

/// Wait up to `wait` on `rx`, making the queries the runtime asks for
/// meanwhile. The guest's thread waits on the runtime this way, so a
/// plugin that reads the client while handling a press gets its answer.
pub(crate) fn wait<T>(rx: &mpsc::Receiver<T>, wait: Duration) -> Option<T> {
    let until = Instant::now() + wait;
    loop {
        serve();
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return rx.try_recv().ok();
        }
        match rx.recv_timeout(left.min(SLICE)) {
            Ok(v) => return Some(v),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
}

/// Make `call` on the guest's thread and return its REPLY.
fn run(call: Vec<u8>) -> Option<Vec<u8>> {
    if on_guest_thread() {
        return super::call(&call);
    }
    let (tx, rx) = mpsc::sync_channel(1);
    lock(&JOBS).push_back((call, tx));
    // A call the guest's thread makes too late finds no one waiting.
    rx.recv_timeout(STORE_WAIT).ok().flatten()
}

/// A query on the guest's store: its REPLY's record and body.
fn query(method: u16, fields: Vec<(PropId, Value)>) -> Option<(Value, Vec<u8>)> {
    let (_, handle) = lock(&STORE).clone()?;
    let mut fields = fields;
    fields.push((HANDLE, Value::I32(handle)));
    fields.sort_by_key(|(p, _)| p.0);
    let id = CALL_IDS | (NEXT_CALL.fetch_add(1, Ordering::Relaxed) & 0xffff);
    let call = Frame::call(MODULE, method, id, &Value::Object(fields)).to_bytes();
    let n = CALLS.fetch_add(1, Ordering::Relaxed);
    let reply = Frame::from_bytes(&run(call)?).ok()?;
    let record = reply.record_value().ok()?;
    if let Some(Value::Str(e)) = field(&record, ERROR) {
        if n < 8 {
            super::log(&format!("store: method {method}: {e}"));
        }
        return None;
    }
    Some((record, reply.body))
}

/// A `getMany` or `scan` body: rows of a `u32` key length, the key, a `u32`
/// value length and the value.
fn rows(mut b: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let take = |b: &mut &[u8]| -> Option<Vec<u8>> {
        let (n, rest) = b.split_first_chunk::<4>()?;
        let n = u32::from_le_bytes(*n) as usize;
        let (v, rest) = (rest.get(..n)?, rest.get(n..)?);
        *b = rest;
        Some(v.to_vec())
    };
    while !b.is_empty() {
        let (Some(k), Some(v)) = (take(&mut b), take(&mut b)) else {
            break;
        };
        out.push((String::from_utf8_lossy(&k).into_owned(), v));
    }
    out
}

fn text(s: &str) -> Value {
    Value::Str(s.to_string())
}

/// The guest's store, read with rekindle's own calls into its module.
pub(crate) struct GuestStore;

impl StoreReader for GuestStore {
    fn get(&self, table: &str, key: &str) -> Option<Vec<u8>> {
        let (record, body) = query(GET, vec![(TABLE, text(table)), (KEY, text(key))])?;
        (field(&record, FOUND) == Some(&Value::Bool(true))).then_some(body)
    }

    fn get_many(&self, table: &str, keys: &[String]) -> Vec<(String, Vec<u8>)> {
        let keys = Value::Array(keys.iter().map(|k| text(k)).collect());
        query(GET_MANY, vec![(TABLE, text(table)), (KEYS, keys)])
            .map(|(_, body)| rows(&body))
            .unwrap_or_default()
    }

    fn scan(&self, table: &str, prefix: &str) -> Vec<(String, Vec<u8>)> {
        scan(table, prefix, false)
    }

    fn keys(&self, table: &str, prefix: &str) -> Vec<String> {
        scan(table, prefix, true)
            .into_iter()
            .map(|(k, _)| k)
            .collect()
    }
}

fn scan(table: &str, prefix: &str, keys_only: bool) -> Vec<(String, Vec<u8>)> {
    let fields = vec![
        (TABLE, text(table)),
        (PREFIX, text(prefix)),
        (KEYS_ONLY, Value::Bool(keys_only)),
    ];
    query(SCAN, fields)
        .map(|(_, body)| rows(&body))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(k: &str, v: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&(k.len() as u32).to_le_bytes());
        b.extend_from_slice(k.as_bytes());
        b.extend_from_slice(&(v.len() as u32).to_le_bytes());
        b.extend_from_slice(v);
        b
    }

    #[test]
    fn rows_read_back() {
        let mut body = row("a", b"one");
        body.extend(row("bb", b""));
        assert_eq!(
            rows(&body),
            vec![("a".into(), b"one".to_vec()), ("bb".into(), Vec::new())]
        );
        // A row cut short ends the list.
        body.truncate(body.len() - 1);
        assert_eq!(rows(&body).len(), 1);
    }

    #[test]
    fn the_guests_open_names_its_store() {
        let open = Frame::call(
            MODULE,
            OPEN,
            77,
            &Value::Object(vec![
                (NAME, text("1234")),
                (PropId(398), Value::Array(vec![])),
            ]),
        );
        on_call(&open.to_bytes());
        let reply = Frame::reply(&open, &Value::Object(vec![(HANDLE, Value::I32(3))]));
        on_reply(&reply.to_bytes());
        assert_eq!(lock(&STORE).clone(), Some(("1234".into(), 3)));
        let close = Frame::call(
            MODULE,
            CLOSE,
            78,
            &Value::Object(vec![(HANDLE, Value::I32(3))]),
        );
        on_call(&close.to_bytes());
        assert_eq!(lock(&STORE).clone(), None);
    }
}
