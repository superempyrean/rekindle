//! The client's REST requests and gateway dispatches, run past the
//! plugins from the platform's hooks.
//!
//! The hooks run on the client's own threads, and the plugins on the
//! runtime's. A hook hands the runtime a [`Request`] and waits for the
//! answer, up to [`FILTER_WAIT`] for a dispatch and [`HTTP_WAIT`] for a
//! request or a response, and lets it pass unchanged after that. Before it
//! builds anything, a hook asks [`intercepts`] or [`filters_http`], which
//! read what the runtime published on its last tick, so traffic no plugin
//! filters never waits.
//!
//! A REST request is filtered only on Discord's API (`api_path`), and only
//! on a route whose kind some plugin registered. The runtime keeps the
//! request until its response comes back through [`http_response`], up to
//! [`IN_FLIGHT`] of them; a request whose future the client drops never
//! gets one.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use rekindle_plugin_host::api::EventKind;
use rekindle_plugin_host::backend::gateway;
use rekindle_plugin_host::runtime::{RequestOutcome, RestRequest, api_path, route_kind};

/// How long a dispatch waits for the plugins' event filters.
pub const FILTER_WAIT: Duration = Duration::from_millis(250);
/// How long a request or a response waits for the plugins' `http-filter`s.
/// A filter may fetch what it needs from another host, so this is longer.
pub const HTTP_WAIT: Duration = Duration::from_secs(2);
/// How many filtered requests are kept for their responses.
pub const IN_FLIGHT: usize = 256;

/// What the plugins made of a dispatch, or of a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Pass,
    Drop,
    /// The dispatch's `d`, or the request's body, to use instead.
    Replace(String),
}

/// What a hook hands the runtime.
pub(super) enum Request {
    Filter {
        t: String,
        d: String,
        done: mpsc::SyncSender<Decision>,
    },
    Received {
        t: String,
        d: String,
    },
    Rest {
        req: RestRequest,
        done: mpsc::SyncSender<Decision>,
    },
    HttpResponse {
        id: u64,
        status: u16,
        body: Option<String>,
        done: mpsc::SyncSender<Option<String>>,
    },
}

static SENDER: Mutex<Option<mpsc::Sender<Request>>> = Mutex::new(None);
/// The event kinds some plugin's `event-filter` sees, one bit each.
static INTERCEPTED: AtomicU64 = AtomicU64::new(0);
/// The kinds of REST route some plugin's `http-filter` sees.
static HTTP_KINDS: AtomicU64 = AtomicU64::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static EVENTS: AtomicU64 = AtomicU64::new(0);
static REQUESTS: AtomicU64 = AtomicU64::new(0);

/// How many dispatches and requests are logged, then one in this many.
const LOGGED: u64 = 64;

fn counted(counter: &AtomicU64) -> bool {
    let n = counter.fetch_add(1, Ordering::Relaxed);
    n < LOGGED || n.is_multiple_of(LOGGED)
}

/// The kinds whose REST routes the runtime knows (`route_kind`).
const ROUTED: [EventKind; 11] = [
    EventKind::Message,
    EventKind::Reaction,
    EventKind::PollVote,
    EventKind::Pins,
    EventKind::Typing,
    EventKind::Member,
    EventKind::Relationship,
    EventKind::Guild,
    EventKind::Channel,
    EventKind::Role,
    EventKind::Emoji,
];

pub(super) fn open() -> mpsc::Receiver<Request> {
    let (tx, rx) = mpsc::channel();
    *SENDER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tx);
    rx
}

pub(super) fn close() {
    *SENDER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    INTERCEPTED.store(0, Ordering::Relaxed);
    HTTP_KINDS.store(0, Ordering::Relaxed);
}

fn send(r: Request) -> bool {
    SENDER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|tx| tx.send(r).is_ok())
}

fn bits(kinds: impl IntoIterator<Item = EventKind>) -> u64 {
    kinds.into_iter().fold(0, |b, k| b | 1 << (k as u8))
}

/// Publish what the plugins filter now, after a tick.
pub(super) fn publish(rk: &rekindle_extension::Rekindle) {
    INTERCEPTED.store(bits(rk.intercepted_kinds()), Ordering::Relaxed);
    let http = ROUTED.into_iter().filter(|&k| rk.filters_http(k));
    HTTP_KINDS.store(bits(http), Ordering::Relaxed);
}

/// Whether some plugin's event filter sees the events dispatch `t` causes.
#[must_use]
pub fn intercepts(t: &str) -> bool {
    gateway::dispatch_kind(t)
        .is_some_and(|k| INTERCEPTED.load(Ordering::Relaxed) & (1 << (k as u8)) != 0)
}

/// Whether some plugin's `http-filter` sees a request to `url`.
#[must_use]
pub fn filters_http(url: &str) -> bool {
    api_path(url)
        .and_then(route_kind)
        .is_some_and(|k| HTTP_KINDS.load(Ordering::Relaxed) & (1 << (k as u8)) != 0)
}

/// Whether dispatch `t`'s `d` may reach the runtime. The others reach it
/// with an empty `d`, for the events that need only the name.
#[must_use]
pub fn forwarded(t: &str) -> bool {
    gateway::forwarded(t)
}

fn wait<T>(rx: &mpsc::Receiver<T>, limit: Duration) -> Option<T> {
    rx.recv_timeout(limit).ok()
}

/// Run dispatch `t` past the plugins' event filters, before the client
/// processes it. On the client's thread; waits up to [`FILTER_WAIT`].
#[must_use]
pub fn filter(t: &str, d: &str) -> Decision {
    let (done, answer) = mpsc::sync_channel(1);
    if !send(Request::Filter {
        t: t.to_string(),
        d: d.to_string(),
        done,
    }) {
        return Decision::Pass;
    }
    wait(&answer, FILTER_WAIT).unwrap_or(Decision::Pass)
}

/// The client processed dispatch `t`: deliver its events. Doesn't wait.
pub fn received(t: &str, d: &str) {
    let d = if forwarded(t) { d } else { "null" };
    send(Request::Received {
        t: t.to_string(),
        d: d.to_string(),
    });
}

/// Run a request to `url` past the plugins' `http-filter`s, before the
/// client sends it. Returns what to do with it, and the id its response
/// is filtered under, or `None` for a request no plugin sees.
#[must_use]
pub fn http_request(method: &str, url: &str, body: Option<&str>) -> (Decision, Option<u64>) {
    if !filters_http(url) {
        return (Decision::Pass, None);
    }
    let Some(path) = api_path(url) else {
        return (Decision::Pass, None);
    };
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let (done, answer) = mpsc::sync_channel(1);
    let req = RestRequest {
        id,
        method: method.to_string(),
        path: path.to_string(),
        body: body.map(str::to_string),
    };
    if !send(Request::Rest { req, done }) {
        return (Decision::Pass, None);
    }
    (wait(&answer, HTTP_WAIT).unwrap_or(Decision::Pass), Some(id))
}

/// Run the response to request `id` past the plugins' `http-filter`s,
/// before the client reads it. `Some` with the body to read instead.
#[must_use]
pub fn http_response(id: u64, status: u16, body: Option<&str>) -> Option<String> {
    let (done, answer) = mpsc::sync_channel(1);
    if !send(Request::HttpResponse {
        id,
        status,
        body: body.map(str::to_string),
        done,
    }) {
        return None;
    }
    wait(&answer, HTTP_WAIT).flatten()
}

/// The runtime's side: the requests waiting for their responses.
#[derive(Default)]
pub(super) struct Pending {
    requests: VecDeque<RestRequest>,
}

impl Pending {
    /// Answer one request from a hook.
    pub(super) fn serve(
        &mut self,
        rk: &mut rekindle_extension::Rekindle,
        r: Request,
        now_ms: u64,
        log: fn(&str),
    ) {
        match r {
            Request::Filter { t, d, done } => {
                let decision = match serde_json::from_str::<serde_json::Value>(&d) {
                    Ok(value) => match rk.gateway_filter(&t, value.clone()) {
                        None => Decision::Drop,
                        Some(new) if new != value => Decision::Replace(new.to_string()),
                        Some(_) => Decision::Pass,
                    },
                    Err(_) => Decision::Pass,
                };
                if decision != Decision::Pass {
                    log(&format!(
                        "gateway: {t} {}",
                        if decision == Decision::Drop {
                            "dropped"
                        } else {
                            "replaced"
                        }
                    ));
                }
                let _ = done.send(decision);
            }
            Request::Received { t, d } => {
                let value = serde_json::from_str(&d).unwrap_or(serde_json::Value::Null);
                let n = rk.gateway_events(&t, value, now_ms);
                if n > 0 && counted(&EVENTS) {
                    log(&format!("gateway: {t} delivered as {n} events"));
                }
            }
            Request::Rest { req, done } => {
                if counted(&REQUESTS) {
                    log(&format!("http: {} {} filtered", req.method, route(&req.path)));
                }
                let decision = match rk.http_request(&req) {
                    RequestOutcome::Drop => Decision::Drop,
                    RequestOutcome::Send(body) if body != req.body => {
                        Decision::Replace(body.unwrap_or_default())
                    }
                    RequestOutcome::Send(_) => Decision::Pass,
                };
                if decision != Decision::Pass {
                    log(&format!(
                        "http: {} {} {}",
                        req.method,
                        route(&req.path),
                        if decision == Decision::Drop {
                            "dropped"
                        } else {
                            "rewritten"
                        }
                    ));
                }
                let _ = done.send(decision);
                if self.requests.len() >= IN_FLIGHT {
                    self.requests.pop_front();
                }
                self.requests.push_back(req);
            }
            Request::HttpResponse {
                id,
                status,
                body,
                done,
            } => {
                let Some(at) = self.requests.iter().position(|r| r.id == id) else {
                    let _ = done.send(None);
                    return;
                };
                let Some(req) = self.requests.remove(at) else {
                    return;
                };
                let read = rk.http_response(&req, status, body.clone());
                let changed = read.is_some() && read != body;
                if changed {
                    log(&format!(
                        "http: the response to {} {} rewritten",
                        req.method,
                        route(&req.path)
                    ));
                }
                let _ = done.send(read.filter(|_| changed));
            }
        }
    }
}

/// A path with its ids and query left out, for the log.
fn route(path: &str) -> String {
    path.split(['?', '#'])
        .next()
        .unwrap_or_default()
        .split('/')
        .map(|s| {
            if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
                "{id}"
            } else {
                s
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_route_in_the_log_names_no_ids() {
        assert_eq!(
            route("/channels/123/messages?limit=50"),
            "/channels/{id}/messages"
        );
    }

    #[test]
    fn nothing_waits_while_no_plugin_filters() {
        close();
        assert!(!intercepts("MESSAGE_CREATE"));
        assert!(!filters_http(
            "https://discord.com/api/v9/channels/1/messages"
        ));
        assert_eq!(
            http_request(
                "GET",
                "https://discord.com/api/v9/channels/1/messages",
                None
            ),
            (Decision::Pass, None)
        );
        assert_eq!(filter("MESSAGE_CREATE", "{}"), Decision::Pass);
    }
}
