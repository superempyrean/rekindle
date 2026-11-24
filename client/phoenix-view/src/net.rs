//! `--live`: the guest's `Networking` and `WebSocket` host modules, over
//! ureq and tungstenite on rustls.
//!
//! The shapes are the Android platform's (`yh` and `or` in
//! `com.discord.nativeapp.modules`, build 356207). A CALL gets no REPLY: the
//! platform answers later with an EVENT frame, `@http` (321) for a request
//! and `@ws` (378) for a socket, from a thread of its own. Here every
//! request and socket runs on its own thread and hands its EVENT to the guest
//! thread, which dispatches it.
//!
//! Every URL passes [`check`] before anything connects, redirects included.
//! It lets through Discord's own hosts over `https` and `wss` and refuses the
//! rest. The log carries method, host, path and status. Header values, query
//! strings and bodies are never logged, except the identity headers in
//! [`IDENTITY_HEADERS`].

use std::collections::{HashMap, HashSet};
use std::io::{ErrorKind, Read as _};
use std::net::{TcpStream, ToSocketAddrs as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rekindle_wire::frame::{self, Frame};
use rekindle_wire::{PropId, Value};
use rustls::{ClientConnection, StreamOwned};
use tungstenite::protocol::CloseFrame;
use tungstenite::{Message, WebSocket};
use ureq::http;

/// Host method and event ids, from the id table of build 356207.
pub mod id {
    pub const NETWORKING_CANCEL: u16 = 167;
    pub const NETWORKING_REQUEST: u16 = 168;
    pub const NETWORKING_UPLOAD: u16 = 169;
    pub const WEBSOCKET_CLOSE: u16 = 289;
    pub const WEBSOCKET_DROP_EARLY: u16 = 290;
    pub const WEBSOCKET_OPEN: u16 = 291;
    pub const WEBSOCKET_SEND_BINARY: u16 = 292;
    pub const WEBSOCKET_SEND_TEXT: u16 = 293;
    pub const WEBSOCKET_ADOPT: u16 = 440;
    /// `@http`, a request's answer.
    pub const HTTP_EVENT: u16 = 321;
    /// `@ws`, a socket's events.
    pub const WS_EVENT: u16 = 378;
}

/// Record keys, from the same table.
mod atom {
    use rekindle_wire::PropId;
    pub const ERROR: PropId = PropId(403);
    pub const ID: PropId = PropId(417);
    pub const METHOD: PropId = PropId(421);
    pub const URL: PropId = PropId(422);
    pub const HEADERS: PropId = PropId(423);
    pub const RESPONSE_HEADERS: PropId = PropId(424);
    pub const REDIRECTS: PropId = PropId(425);
    pub const ACCESS: PropId = PropId(426);
    pub const STATUS: PropId = PropId(430);
    pub const ORIGIN: PropId = PropId(441);
    pub const CODE: PropId = PropId(442);
    pub const EVENT: PropId = PropId(443);
    pub const RAW_LENGTH: PropId = PropId(445);
    pub const DELIVERED_AT_MS: PropId = PropId(446);
    pub const INFLATE_MS: PropId = PropId(447);
    pub const REASON: PropId = PropId(448);
    pub const KIND: PropId = PropId(449);
    pub const MESSAGE: PropId = PropId(450);
    pub const FAILURE: PropId = PropId(1458);
}

/// The domains the guest may reach, each with its subdomains.
pub const ALLOWED: &[&str] = &[
    "discord.com",
    "discord.gg",
    "discordapp.com",
    "discordapp.net",
];

/// Headers whose values the log shows. None of them carries a credential.
pub const IDENTITY_HEADERS: &[&str] = &[
    "user-agent",
    "x-super-properties",
    "x-discord-locale",
    "x-discord-timezone",
    "x-debug-options",
    "accept-language",
    "content-type",
];

/// Headers the HTTP stack writes itself.
const OWN_HEADERS: &[&str] = &["content-length", "host", "connection", "transfer-encoding"];

const MAX_REDIRECTS: usize = 10;
const MAX_BODY: u64 = 64 << 20;
/// How long a socket's thread waits for a frame before it looks at what
/// the guest wants sent.
const POLL: Duration = Duration::from_millis(20);

/// A URL [`check`] let through.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub host: String,
    pub path: String,
}

/// The allowlist, the one place a URL is let through: `scheme` only, port
/// 443, no credentials in the URL, and a host in [`ALLOWED`]. A refusal
/// names the host, never the full URL.
pub fn check(url: &str, scheme: &str) -> Result<Target, String> {
    let uri: http::Uri = url.parse().map_err(|_| "not a URL".to_string())?;
    let auth = uri.authority().ok_or("no host")?;
    let host = auth.host().trim_end_matches('.').to_ascii_lowercase();
    if uri.scheme_str() != Some(scheme) {
        return Err(format!(
            "{host}: scheme {}, only {scheme}",
            uri.scheme_str().unwrap_or("none")
        ));
    }
    if auth.as_str().contains('@') {
        return Err(format!("{host}: credentials in the URL"));
    }
    if uri.port_u16().is_some_and(|p| p != 443) {
        return Err(format!("{host}: port {}", uri.port_u16().unwrap_or(0)));
    }
    let ok = ALLOWED
        .iter()
        .any(|d| host == *d || host.strip_suffix(d).is_some_and(|s| s.ends_with('.')));
    if !ok {
        return Err(format!("{host}: not a Discord host"));
    }
    Ok(Target {
        host,
        path: uri.path().to_string(),
    })
}

/// One request or socket, for the self-test and the log.
#[derive(Debug, Clone)]
pub struct Exchange {
    pub id: i64,
    pub method: String,
    pub host: String,
    pub path: String,
    /// The HTTP status, or 101 for a socket that opened.
    pub status: Option<u16>,
    /// "refused: ...", a failure, or empty.
    pub note: String,
    /// The response asked for a captcha (`captcha_key`).
    pub captcha: bool,
}

enum Out {
    Text(String),
    Binary(Vec<u8>),
    Close(u16),
}

/// The platform's network side.
pub struct Net {
    deliver: Box<dyn Fn(Vec<u8>) + Send + Sync>,
    agent: ureq::Agent,
    tls: Arc<rustls::ClientConfig>,
    cancelled: Mutex<HashMap<i64, Arc<AtomicBool>>>,
    sockets: Mutex<HashMap<i64, mpsc::Sender<Out>>>,
    exchanges: Mutex<Vec<Exchange>>,
    shown: Mutex<HashSet<String>>,
    /// When the platform started, for `deliveredAtMs`.
    started: Instant,
}

fn log(line: &str) {
    #[cfg(test)]
    eprintln!("{line}");
    crate::guest::log(line);
}

fn field(record: &Value, k: PropId) -> Option<&Value> {
    match record {
        Value::Object(fields) => fields.iter().find(|(p, _)| *p == k).map(|(_, v)| v),
        _ => None,
    }
}

fn text(record: &Value, k: PropId) -> Option<&str> {
    match field(record, k) {
        Some(Value::Str(s)) => Some(s),
        _ => None,
    }
}

fn flag(record: &Value, k: PropId) -> Option<bool> {
    match field(record, k) {
        Some(Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

fn int(record: &Value, k: PropId) -> Option<i64> {
    field(record, k).and_then(Value::as_int)
}

fn strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| match v {
                Value::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => vec![],
    }
}

fn id_value(id: i64) -> Value {
    i32::try_from(id).map_or(Value::I64(id), Value::I32)
}

fn event(event: u16, record: &Value, body: Vec<u8>) -> Vec<u8> {
    Frame {
        kind: frame::EVENT,
        module: 0,
        method: event,
        call: 0,
        record: record.to_bytes(),
        body,
    }
    .to_bytes()
}

/// `@http` as Java's `HttpEvent` writes it: `id`, `status`, then the
/// optional `headers` (a map of the asked-for response headers, lower-cased),
/// `error` and `failure`.
fn http_event(
    id: i64,
    status: u16,
    headers: Vec<(String, String)>,
    error: Option<String>,
    failure: Option<&str>,
) -> Value {
    let mut f = vec![
        (atom::ID, id_value(id)),
        (atom::STATUS, Value::I32(i32::from(status))),
    ];
    if !headers.is_empty() {
        f.push((
            atom::HEADERS,
            Value::Map(
                headers
                    .into_iter()
                    .map(|(k, v)| (k, Value::Str(v)))
                    .collect(),
            ),
        ));
    }
    if let Some(e) = error {
        f.push((atom::ERROR, Value::Str(e)));
    }
    if let Some(e) = failure {
        f.push((atom::FAILURE, Value::Str(e.into())));
    }
    Value::Object(f)
}

/// `@ws` as Java's `WsEvent` writes it.
#[derive(Default)]
struct WsEvent {
    code: Option<u16>,
    reason: Option<String>,
    kind: Option<String>,
    status: Option<u16>,
    message: Option<String>,
    /// A binary message the platform inflated: its compressed length, when
    /// it arrived (ms since the platform started) and how long inflating it
    /// took.
    inflated: Option<Inflated>,
}

#[derive(Clone, Copy)]
struct Inflated {
    raw_length: i64,
    delivered_at_ms: i64,
    inflate_ms: i64,
}

impl WsEvent {
    fn value(self, id: i64, name: &str) -> Value {
        let mut f = vec![
            (atom::ID, id_value(id)),
            (atom::EVENT, Value::Str(name.into())),
        ];
        if let Some(c) = self.code {
            f.push((atom::CODE, Value::I32(i32::from(c))));
        }
        if let Some(r) = self.reason.filter(|r| !r.is_empty()) {
            f.push((atom::REASON, Value::Str(r)));
        }
        if let Some(k) = self.kind {
            f.push((atom::KIND, Value::Str(k)));
        }
        if let Some(s) = self.status {
            f.push((atom::STATUS, Value::I32(i32::from(s))));
        }
        if let Some(m) = self.message {
            f.push((atom::MESSAGE, Value::Str(m)));
        }
        if let Some(i) = self.inflated {
            f.push((atom::RAW_LENGTH, Value::I64(i.raw_length)));
            f.push((atom::DELIVERED_AT_MS, Value::I64(i.delivered_at_ms)));
            f.push((atom::INFLATE_MS, Value::I64(i.inflate_ms)));
        }
        Value::Object(f)
    }
}

/// A request's failure: the `error` text and the `failure` category the
/// Java platform uses (`dns`, `timeout`, `tls`, `badResponse`, `unknown`).
struct Fail {
    error: String,
    failure: &'static str,
}

impl Fail {
    fn refused(why: &str) -> Fail {
        Fail {
            error: format!("refused by phoenix-view: {why}"),
            failure: "unknown",
        }
    }
}

fn classify(e: &ureq::Error) -> &'static str {
    match e {
        ureq::Error::HostNotFound => "dns",
        ureq::Error::Timeout(_) => "timeout",
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) => "tls",
        ureq::Error::Protocol(_) | ureq::Error::LargeResponseHeader(..) => "badResponse",
        ureq::Error::Io(io) if io.kind() == ErrorKind::TimedOut => "timeout",
        ureq::Error::Io(io) if io.kind() == ErrorKind::UnexpectedEof => "badResponse",
        _ => "unknown",
    }
}

/// A response: status, the asked-for headers and the body.
type Answer = (u16, Vec<(String, String)>, Vec<u8>);

/// A request as `Networking.request` gives it.
struct Request {
    id: i64,
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    response_headers: Vec<String>,
    redirects: bool,
    body: Vec<u8>,
}

impl Net {
    /// `deliver` hands an EVENT frame to the guest thread.
    pub fn new(deliver: Box<dyn Fn(Vec<u8>) + Send + Sync>) -> anyhow::Result<Arc<Net>> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .https_only(true)
            .allow_non_standard_methods(true)
            // okhttp adds only Accept-Encoding: gzip, and unzips. The guest
            // writes User-Agent itself.
            .user_agent(ureq::config::AutoHeaderValue::None)
            .accept(ureq::config::AutoHeaderValue::None)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_recv_body(Some(Duration::from_secs(60)))
            .build()
            .into();
        let roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
        Ok(Arc::new(Net {
            deliver,
            agent,
            tls: Arc::new(tls),
            cancelled: Mutex::default(),
            sockets: Mutex::default(),
            exchanges: Mutex::default(),
            shown: Mutex::default(),
            started: Instant::now(),
        }))
    }

    /// Every request and socket so far.
    pub fn exchanges(&self) -> Vec<Exchange> {
        self.exchanges.lock().unwrap().clone()
    }

    fn record(&self, x: Exchange) {
        self.exchanges.lock().unwrap().push(x);
    }

    /// A host CALL of the two modules, answered later by an EVENT. The note
    /// is for the call log.
    pub fn call(self: &Arc<Self>, method: u16, record: &Value, body: &[u8]) -> String {
        match method {
            id::NETWORKING_REQUEST => self.request(record, body),
            id::NETWORKING_UPLOAD => {
                let id = int(record, atom::ID).unwrap_or(-1);
                let host = text(record, atom::URL)
                    .and_then(|u| u.parse::<http::Uri>().ok())
                    .and_then(|u| u.host().map(str::to_string))
                    .unwrap_or_default();
                self.record(Exchange {
                    id,
                    method: "UPLOAD".into(),
                    host: host.clone(),
                    path: String::new(),
                    status: None,
                    note: "refused: uploads are not supported".into(),
                    captcha: false,
                });
                (self.deliver)(event(
                    id::HTTP_EVENT,
                    &http_event(
                        id,
                        0,
                        vec![],
                        Some("refused by phoenix-view: uploads are not supported".into()),
                        Some("unknown"),
                    ),
                    vec![],
                ));
                format!("#{id} to {host}: refused, uploads are not supported")
            }
            id::NETWORKING_CANCEL => {
                let id = int(record, atom::ID).unwrap_or(-1);
                if let Some(c) = self.cancelled.lock().unwrap().get(&id) {
                    c.store(true, Ordering::Release);
                }
                format!("#{id} cancelled")
            }
            id::WEBSOCKET_OPEN | id::WEBSOCKET_ADOPT => {
                let id = int(record, atom::ID).unwrap_or(-1);
                let url = text(record, atom::URL).unwrap_or_default().to_string();
                let origin = text(record, atom::ORIGIN).unwrap_or_default().to_string();
                self.open(id, url, origin)
            }
            id::WEBSOCKET_SEND_TEXT | id::WEBSOCKET_SEND_BINARY => {
                let id = int(record, atom::ID).unwrap_or(-1);
                let out = if method == id::WEBSOCKET_SEND_TEXT {
                    Out::Text(String::from_utf8_lossy(body).into_owned())
                } else {
                    Out::Binary(body.to_vec())
                };
                match self.sockets.lock().unwrap().get(&id) {
                    Some(tx) => {
                        let _ = tx.send(out);
                        format!("socket {id}: {} bytes", body.len())
                    }
                    None => format!("socket {id}: not open"),
                }
            }
            id::WEBSOCKET_CLOSE => {
                let id = int(record, atom::ID).unwrap_or(-1);
                let code = int(record, atom::CODE).map_or(1000, |c| c as u16);
                match self.sockets.lock().unwrap().remove(&id) {
                    Some(tx) => {
                        let _ = tx.send(Out::Close(code));
                        format!("socket {id}: close {code}")
                    }
                    None => format!("socket {id}: not open"),
                }
            }
            id::WEBSOCKET_DROP_EARLY => "no early socket is held".into(),
            _ => format!("method {method}: not a network call"),
        }
    }

    /// Log the identity headers' values the first time each is seen, and
    /// every other header by name.
    fn log_headers(&self, id: i64, headers: &[(String, String)]) {
        let mut names = Vec::new();
        for (k, v) in headers {
            let lower = k.to_ascii_lowercase();
            if !IDENTITY_HEADERS.contains(&lower.as_str()) {
                names.push(lower);
                continue;
            }
            names.push(lower.clone());
            if !self.shown.lock().unwrap().insert(format!("{lower}: {v}")) {
                continue;
            }
            if lower == "x-super-properties" {
                use base64::Engine as _;
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(v)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .unwrap_or_else(|| "(not base64 JSON)".into());
                log(&format!(
                    "[net] #{id} identity x-super-properties = {decoded}"
                ));
            } else {
                log(&format!("[net] #{id} identity {lower}: {v}"));
            }
        }
        log(&format!("[net] #{id} headers: {}", names.join(", ")));
    }

    fn request(self: &Arc<Self>, record: &Value, body: &[u8]) -> String {
        let id = int(record, atom::ID).unwrap_or(-1);
        let headers: Vec<(String, String)> = match field(record, atom::HEADERS) {
            Some(Value::Array(pairs)) => pairs
                .iter()
                .filter_map(|p| match p {
                    Value::Array(kv) => match kv.as_slice() {
                        [Value::Str(k), Value::Str(v)] => Some((k.clone(), v.clone())),
                        _ => None,
                    },
                    _ => None,
                })
                .collect(),
            _ => vec![],
        };
        let req = Request {
            id,
            method: text(record, atom::METHOD)
                .unwrap_or("GET")
                .to_ascii_uppercase(),
            url: text(record, atom::URL).unwrap_or_default().to_string(),
            headers,
            response_headers: strings(field(record, atom::RESPONSE_HEADERS)),
            redirects: flag(record, atom::REDIRECTS) != Some(false),
            body: body.to_vec(),
        };
        let (host, path) = match check(&req.url, "https") {
            Ok(t) => (t.host, t.path),
            Err(why) => {
                log(&format!("[net] #{id} {} REFUSED: {why}", req.method));
                self.record(Exchange {
                    id,
                    method: req.method.clone(),
                    host: why.split(':').next().unwrap_or_default().to_string(),
                    path: String::new(),
                    status: None,
                    note: format!("refused: {why}"),
                    captcha: false,
                });
                (self.deliver)(event(
                    id::HTTP_EVENT,
                    &http_event(
                        id,
                        0,
                        vec![],
                        Some(Fail::refused(&why).error),
                        Some("unknown"),
                    ),
                    vec![],
                ));
                return format!("#{id} {} REFUSED: {why}", req.method);
            }
        };
        if flag(record, atom::ACCESS) == Some(true) {
            // The Java platform sends these through Cloudflare Access with
            // the staff session. phoenix-view has none and sends nothing.
            let why = format!("{host}: asks for Cloudflare Access");
            log(&format!("[net] #{id} {} REFUSED: {why}", req.method));
            self.record(Exchange {
                id,
                method: req.method.clone(),
                host,
                path,
                status: None,
                note: format!("refused: {why}"),
                captcha: false,
            });
            (self.deliver)(event(
                id::HTTP_EVENT,
                &http_event(
                    id,
                    0,
                    vec![],
                    Some(Fail::refused(&why).error),
                    Some("unknown"),
                ),
                vec![],
            ));
            return format!("#{id} REFUSED: {why}");
        }
        log(&format!(
            "[net] #{id} {} {host} {path}{} ({} body bytes)",
            req.method,
            if req.url.contains('?') { "?..." } else { "" },
            req.body.len()
        ));
        self.log_headers(id, &req.headers);
        let cancelled = Arc::new(AtomicBool::new(false));
        self.cancelled.lock().unwrap().insert(id, cancelled.clone());
        let net = self.clone();
        let note = format!("#{id} {} {host} {path}: sent", req.method);
        let spawned = std::thread::Builder::new()
            .name(format!("http-{id}"))
            .spawn(move || net.run_request(&req, &host, &path, &cancelled));
        if let Err(e) = spawned {
            log(&format!("[net] #{id}: no thread: {e}"));
        }
        note
    }

    fn run_request(&self, req: &Request, host: &str, path: &str, cancelled: &AtomicBool) {
        let t0 = Instant::now();
        let result = self.fetch(req);
        self.cancelled.lock().unwrap().remove(&req.id);
        let ms = t0.elapsed().as_millis();
        if cancelled.load(Ordering::Acquire) {
            log(&format!("[net] #{} cancelled after {ms} ms", req.id));
            return;
        }
        let id = req.id;
        match result {
            Ok((status, headers, body)) => {
                let captcha = status >= 400 && contains(&body, b"\"captcha_key\"");
                log(&format!(
                    "[net] #{id} {} {host} {path} -> {status} ({ms} ms, {} bytes){}",
                    req.method,
                    body.len(),
                    if captcha {
                        "; the response asks for a captcha (captcha_key)"
                    } else {
                        ""
                    }
                ));
                self.record(Exchange {
                    id,
                    method: req.method.clone(),
                    host: host.into(),
                    path: path.into(),
                    status: Some(status),
                    note: String::new(),
                    captcha,
                });
                (self.deliver)(event(
                    id::HTTP_EVENT,
                    &http_event(id, status, headers, None, None),
                    body,
                ));
            }
            Err(fail) => {
                log(&format!(
                    "[net] #{id} {} {host} {path} -> failed ({}, {ms} ms): {}",
                    req.method, fail.failure, fail.error
                ));
                self.record(Exchange {
                    id,
                    method: req.method.clone(),
                    host: host.into(),
                    path: path.into(),
                    status: None,
                    note: fail.error.clone(),
                    captcha: false,
                });
                (self.deliver)(event(
                    id::HTTP_EVENT,
                    &http_event(id, 0, vec![], Some(fail.error), Some(fail.failure)),
                    vec![],
                ));
            }
        }
    }

    /// GET an image for the image loader, as the Android platform's loader
    /// fetches a node's `uri`: no headers of the guest's, redirects through
    /// [`check`]. The log carries host, path, status and size.
    pub fn get_image(&self, url: &str) -> Result<Vec<u8>, String> {
        let target = check(url, "https").map_err(|why| {
            log(&format!("[img] REFUSED: {why}"));
            format!("refused: {why}")
        })?;
        let req = Request {
            id: 0,
            method: "GET".into(),
            url: url.to_string(),
            headers: vec![("Accept".into(), "image/webp,image/*;q=0.8".into())],
            response_headers: vec![],
            redirects: true,
            body: vec![],
        };
        match self.fetch(&req) {
            Ok((200, _, bytes)) => {
                log(&format!(
                    "[img] GET {}{} -> 200, {} bytes",
                    target.host,
                    target.path,
                    bytes.len()
                ));
                Ok(bytes)
            }
            Ok((status, _, _)) => {
                log(&format!(
                    "[img] GET {}{} -> {status}",
                    target.host, target.path
                ));
                Err(format!("HTTP {status}"))
            }
            Err(f) => {
                log(&format!(
                    "[img] GET {}{} failed: {}",
                    target.host, target.path, f.failure
                ));
                Err(f.error)
            }
        }
    }

    /// Send `req`, following redirects through [`check`] when it allows
    /// them, as okhttp does: 303, and 301 or 302 after anything but GET or
    /// HEAD, become a GET without a body.
    fn fetch(&self, req: &Request) -> Result<Answer, Fail> {
        let mut url = req.url.clone();
        let mut method = req.method.clone();
        let mut body = req.body.clone();
        for hop in 0..=MAX_REDIRECTS {
            if hop > 0 {
                check(&url, "https").map_err(|why| {
                    log(&format!("[net] #{} redirect REFUSED: {why}", req.id));
                    Fail::refused(&why)
                })?;
            }
            let mut b = http::Request::builder().method(method.as_str()).uri(&url);
            for (k, v) in &req.headers {
                if !OWN_HEADERS.contains(&k.to_ascii_lowercase().as_str()) {
                    b = b.header(k, v);
                }
            }
            let send_body = matches!(method.as_str(), "POST" | "PUT" | "PATCH") || !body.is_empty();
            let sent = if send_body {
                b.body(body.clone())
                    .map_err(|e| Fail {
                        error: format!("bad request: {e}"),
                        failure: "unknown",
                    })
                    .and_then(|r| {
                        self.agent.run(r).map_err(|e| Fail {
                            failure: classify(&e),
                            error: e.to_string(),
                        })
                    })
            } else {
                b.body(())
                    .map_err(|e| Fail {
                        error: format!("bad request: {e}"),
                        failure: "unknown",
                    })
                    .and_then(|r| {
                        self.agent.run(r).map_err(|e| Fail {
                            failure: classify(&e),
                            error: e.to_string(),
                        })
                    })
            };
            let mut resp = sent?;
            let status = resp.status().as_u16();
            let location = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            if req.redirects
                && matches!(status, 301 | 302 | 303 | 307 | 308)
                && let Some(loc) = location
                && hop < MAX_REDIRECTS
            {
                url = resolve(&url, &loc)
                    .ok_or_else(|| Fail::refused("a redirect to an unusable Location"))?;
                if status == 303
                    || (matches!(status, 301 | 302) && !matches!(method.as_str(), "GET" | "HEAD"))
                {
                    method = "GET".into();
                    body.clear();
                }
                log(&format!("[net] #{} -> {status}, following", req.id));
                continue;
            }
            let headers = req
                .response_headers
                .iter()
                .filter_map(|name| {
                    let v = resp.headers().get(name.as_str())?.to_str().ok()?;
                    Some((name.to_ascii_lowercase(), v.to_string()))
                })
                .collect();
            let mut bytes = Vec::new();
            resp.body_mut()
                .with_config()
                .limit(MAX_BODY)
                .reader()
                .read_to_end(&mut bytes)
                .map_err(|e| Fail {
                    error: e.to_string(),
                    failure: "badResponse",
                })?;
            return Ok((status, headers, bytes));
        }
        Err(Fail {
            error: "too many redirects".into(),
            failure: "badResponse",
        })
    }

    fn open(self: &Arc<Self>, id: i64, url: String, origin: String) -> String {
        let target = match check(&url, "wss") {
            Ok(t) => t,
            Err(why) => {
                log(&format!("[net] socket {id} REFUSED: {why}"));
                self.record(Exchange {
                    id,
                    method: "WS".into(),
                    host: why.split(':').next().unwrap_or_default().to_string(),
                    path: String::new(),
                    status: None,
                    note: format!("refused: {why}"),
                    captcha: false,
                });
                (self.deliver)(event(
                    id::WS_EVENT,
                    &WsEvent {
                        kind: Some("refused".into()),
                        message: Some(format!("refused by phoenix-view: {why}")),
                        ..WsEvent::default()
                    }
                    .value(id, "error"),
                    vec![],
                ));
                return format!("socket {id} REFUSED: {why}");
            }
        };
        let (tx, rx) = mpsc::channel();
        self.sockets.lock().unwrap().insert(id, tx);
        let query = url
            .split_once('?')
            .map(|(_, q)| q.to_string())
            .unwrap_or_default();
        log(&format!(
            "[net] socket {id} wss {} {} ({})",
            target.host, target.path, query
        ));
        let net = self.clone();
        let note = format!("socket {id} to {}: dialling", target.host);
        let spawned = std::thread::Builder::new()
            .name(format!("ws-{id}"))
            .spawn(move || net.run_socket(id, &url, &target, &origin, &rx));
        if let Err(e) = spawned {
            log(&format!("[net] socket {id}: no thread: {e}"));
        }
        note
    }

    fn ws(&self, id: i64, name: &str, ev: WsEvent, body: Vec<u8>) {
        (self.deliver)(event(id::WS_EVENT, &ev.value(id, name), body));
    }

    fn run_socket(
        &self,
        id: i64,
        url: &str,
        target: &Target,
        origin: &str,
        rx: &mpsc::Receiver<Out>,
    ) {
        let t0 = Instant::now();
        let mut ws = match self.dial(url, &target.host, origin) {
            Ok(ws) => ws,
            Err(ev) => {
                log(&format!(
                    "[net] socket {id} failed ({}): {}",
                    ev.kind.as_deref().unwrap_or("?"),
                    ev.message.as_deref().unwrap_or("")
                ));
                self.record(Exchange {
                    id,
                    method: "WS".into(),
                    host: target.host.clone(),
                    path: target.path.clone(),
                    status: ev.status,
                    note: ev.message.clone().unwrap_or_default(),
                    captcha: false,
                });
                self.sockets.lock().unwrap().remove(&id);
                self.ws(id, "error", ev, vec![]);
                return;
            }
        };
        log(&format!(
            "[net] socket {id} open ({} ms)",
            t0.elapsed().as_millis()
        ));
        self.record(Exchange {
            id,
            method: "WS".into(),
            host: target.host.clone(),
            path: target.path.clone(),
            status: Some(101),
            note: String::new(),
            captcha: false,
        });
        self.ws(id, "open", WsEvent::default(), vec![]);
        // A zstd-stream gateway is inflated here, one decompression stream
        // per socket, as Java's `ZstdInflate` does. A message that fails to
        // inflate closes the socket with 1007, and the guest reconnects with
        // zlib-stream.
        let mut zstd = url.contains("compress=zstd-stream").then(ZstdStream::new);
        let (mut frames, mut bytes) = (0u64, 0u64);
        let mut peer_close: Option<(u16, String)> = None;
        // The guest's sender goes away after `WebSocket.close`, or when the
        // platform stops: the socket closes and is read to its end.
        let mut orphaned = false;
        let end = loop {
            match ws.read() {
                Ok(Message::Text(t)) => {
                    frames += 1;
                    bytes += t.len() as u64;
                    self.ws(id, "text", WsEvent::default(), t.as_bytes().to_vec());
                }
                Ok(Message::Binary(b)) => {
                    frames += 1;
                    bytes += b.len() as u64;
                    let Some(z) = zstd.as_mut() else {
                        self.ws(id, "binary", WsEvent::default(), b.to_vec());
                        continue;
                    };
                    let delivered_at_ms = ms(self.started.elapsed());
                    let t = Instant::now();
                    match z.inflate(&b) {
                        Ok(out) => {
                            let inflated = Inflated {
                                raw_length: b.len() as i64,
                                delivered_at_ms,
                                inflate_ms: ms(t.elapsed()),
                            };
                            let ev = WsEvent {
                                inflated: Some(inflated),
                                ..WsEvent::default()
                            };
                            self.ws(id, "binary", ev, out);
                        }
                        Err(e) => {
                            log(&format!("[net] socket {id}: zstd: {e}"));
                            let _ = ws.close(Some(CloseFrame {
                                code: 1007.into(),
                                reason: "".into(),
                            }));
                            break Some(WsEvent {
                                code: Some(1007),
                                ..WsEvent::default()
                            });
                        }
                    }
                }
                Ok(Message::Close(f)) => {
                    peer_close = Some(f.map_or((1005, String::new()), |f| {
                        (u16::from(f.code), f.reason.as_str().to_string())
                    }));
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                    let (code, reason) = peer_close.clone().unwrap_or((1000, String::new()));
                    break Some(WsEvent {
                        code: Some(code),
                        reason: Some(reason),
                        ..WsEvent::default()
                    });
                }
                Err(e) => {
                    break Some(WsEvent {
                        kind: Some(ws_kind(&e).into()),
                        message: Some(e.to_string()),
                        ..WsEvent::default()
                    });
                }
            }
            let mut stop = None;
            while !orphaned {
                let sent = match rx.try_recv() {
                    Ok(Out::Text(s)) => ws.send(Message::text(s)),
                    Ok(Out::Binary(b)) => ws.send(Message::binary(b)),
                    Ok(Out::Close(code)) => {
                        peer_close.get_or_insert((code, String::new()));
                        ws.close(Some(CloseFrame {
                            code: code.into(),
                            reason: "".into(),
                        }))
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        orphaned = true;
                        if peer_close.is_some() {
                            break;
                        }
                        peer_close = Some((1000, String::new()));
                        ws.close(None)
                    }
                };
                match sent {
                    Ok(())
                    | Err(
                        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed,
                    ) => {}
                    Err(tungstenite::Error::Io(e))
                        if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                    Err(e) => {
                        stop = Some(WsEvent {
                            kind: Some(ws_kind(&e).into()),
                            message: Some(e.to_string()),
                            ..WsEvent::default()
                        });
                        break;
                    }
                }
            }
            if let Some(s) = stop {
                break Some(s);
            }
        };
        self.sockets.lock().unwrap().remove(&id);
        log(&format!(
            "[net] socket {id} ended after {} s, {frames} frames, {bytes} bytes in: {}",
            t0.elapsed().as_secs(),
            match &end {
                Some(e) if e.kind.is_some() => format!(
                    "error {} {}",
                    e.kind.as_deref().unwrap_or(""),
                    e.message.as_deref().unwrap_or("")
                ),
                Some(e) => format!("close {}", e.code.unwrap_or(0)),
                None => "the platform stopped".into(),
            }
        ));
        match end {
            Some(e) if e.kind.is_some() => self.ws(id, "error", e, vec![]),
            Some(e) => self.ws(id, "close", e, vec![]),
            None => {}
        }
    }

    /// TCP, TLS and the WebSocket handshake. A failure comes back as the
    /// `error` event Java's `onFailure` sends: the exception's class as
    /// `kind`, the HTTP status if there was one, and the message.
    fn dial(
        &self,
        url: &str,
        host: &str,
        origin: &str,
    ) -> Result<WebSocket<StreamOwned<ClientConnection, TcpStream>>, WsEvent> {
        let fail = |kind: &str, status: Option<u16>, message: String| WsEvent {
            kind: Some(kind.into()),
            status,
            message: Some(message),
            ..WsEvent::default()
        };
        let addrs: Vec<_> = (host, 443)
            .to_socket_addrs()
            .map_err(|e| fail("UnknownHostException", None, e.to_string()))?
            .collect();
        let mut last = None;
        let mut tcp = None;
        for a in &addrs {
            match TcpStream::connect_timeout(a, Duration::from_secs(15)) {
                Ok(s) => {
                    tcp = Some(s);
                    break;
                }
                Err(e) => last = Some(e),
            }
        }
        let tcp = tcp.ok_or_else(|| {
            let e = last.map_or_else(|| "no address".to_string(), |e| e.to_string());
            fail("ConnectException", None, e)
        })?;
        let _ = tcp.set_nodelay(true);
        let _ = tcp.set_read_timeout(Some(Duration::from_secs(15)));
        let name = rustls::pki_types::ServerName::try_from(host.to_string())
            .map_err(|e| fail("SSLException", None, e.to_string()))?;
        let conn = ClientConnection::new(self.tls.clone(), name)
            .map_err(|e| fail("SSLException", None, e.to_string()))?;
        let stream = StreamOwned::new(conn, tcp);
        let mut request = tungstenite::client::IntoClientRequest::into_client_request(url)
            .map_err(|e| fail("IllegalArgumentException", None, e.to_string()))?;
        if !origin.is_empty() {
            let v = origin
                .parse()
                .map_err(|_| fail("IllegalArgumentException", None, "bad origin".into()))?;
            request.headers_mut().insert("Origin", v);
        }
        let (ws, _) = tungstenite::client(request, stream).map_err(|e| match e {
            tungstenite::HandshakeError::Failure(tungstenite::Error::Http(resp)) => fail(
                "ProtocolException",
                Some(resp.status().as_u16()),
                format!("Expected HTTP 101 response but was '{}'", resp.status()),
            ),
            tungstenite::HandshakeError::Failure(e) => fail(ws_kind(&e), None, e.to_string()),
            tungstenite::HandshakeError::Interrupted(_) => {
                fail("SocketTimeoutException", None, "timeout".into())
            }
        })?;
        let _ = ws.get_ref().sock.set_read_timeout(Some(POLL));
        Ok(ws)
    }
}

fn ws_kind(e: &tungstenite::Error) -> &'static str {
    match e {
        tungstenite::Error::Io(io) => match io.kind() {
            ErrorKind::TimedOut | ErrorKind::WouldBlock => "SocketTimeoutException",
            ErrorKind::UnexpectedEof => "EOFException",
            ErrorKind::InvalidData => "SSLException",
            _ => "IOException",
        },
        tungstenite::Error::Tls(_) => "SSLException",
        tungstenite::Error::Protocol(_) => "ProtocolException",
        _ => "IOException",
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// A redirect's `Location` against the URL it answered.
fn resolve(base: &str, loc: &str) -> Option<String> {
    if loc.starts_with("https://") || loc.starts_with("http://") {
        return Some(loc.to_string());
    }
    let uri: http::Uri = base.parse().ok()?;
    let origin = format!("{}://{}", uri.scheme_str()?, uri.authority()?);
    if loc.starts_with('/') {
        Some(format!("{origin}{loc}"))
    } else {
        None
    }
}

fn ms(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

/// A zstd-stream gateway's decompression stream. Discord compresses the
/// whole session as one zstd frame and flushes it at the end of each
/// message, so every message inflates to whole output with the window of
/// the ones before it. Java's `hs` reads a message until the stream gives
/// nothing more, and fails if input is left over or the message never ends.
struct ZstdStream {
    dctx: zstd::stream::raw::Decoder<'static>,
}

impl ZstdStream {
    fn new() -> ZstdStream {
        ZstdStream {
            dctx: zstd::stream::raw::Decoder::new().expect("a zstd decompression context"),
        }
    }

    fn inflate(&mut self, message: &[u8]) -> Result<Vec<u8>, String> {
        use zstd::stream::raw::{InBuffer, Operation as _, OutBuffer};
        let mut out = Vec::with_capacity((message.len() * 4).max(1024));
        let mut chunk = vec![0u8; (message.len() * 2).max(1024)];
        let mut input = InBuffer::around(message);
        // Java gives up after 4096 rounds.
        for _ in 0..4096 {
            let mut output = OutBuffer::around(&mut chunk[..]);
            self.dctx
                .run(&mut input, &mut output)
                .map_err(|e| e.to_string())?;
            let n = output.pos();
            out.extend_from_slice(&chunk[..n]);
            if input.pos() == message.len() && n < chunk.len() {
                return Ok(out);
            }
        }
        Err("message didn't end".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Messages compressed as one zstd frame, flushed after each, as
    /// Discord's zstd-stream gateway sends them. Each inflates whole, the
    /// later ones back-referencing the earlier.
    #[test]
    fn zstd_stream() {
        use zstd::stream::raw::{Encoder, InBuffer, Operation as _, OutBuffer};
        let messages: Vec<Vec<u8>> = (0..50)
            .map(|i| {
                format!(
                    r#"{{"op":0,"s":{i},"t":"MESSAGE_CREATE","d":{{"content":"{}"}}}}"#,
                    "hello ".repeat(i * 40)
                )
                .into_bytes()
            })
            .collect();
        let mut enc = Encoder::new(3).unwrap();
        let mut z = ZstdStream::new();
        for m in &messages {
            let mut wire = vec![0u8; m.len() + 1024];
            let mut out = OutBuffer::around(&mut wire[..]);
            let mut input = InBuffer::around(&m[..]);
            enc.run(&mut input, &mut out).unwrap();
            assert_eq!(input.pos(), m.len());
            assert_eq!(enc.flush(&mut out).unwrap(), 0);
            let n = out.pos();
            assert_eq!(&z.inflate(&wire[..n]).unwrap(), m);
        }
        assert!(z.inflate(b"not zstd at all").is_err());
    }

    #[test]
    fn allowlist() {
        for ok in [
            "https://discord.com/api/v9/experiments",
            "https://canary.discord.com/api/v9/users/@me",
            "https://cdn.discordapp.com/avatars/1/2.png",
            "https://media.discordapp.net/x",
            "https://discord.gg/abc",
            "https://DISCORD.COM./api",
            "https://discord.com:443/api",
        ] {
            assert!(check(ok, "https").is_ok(), "{ok}");
        }
        assert!(check("wss://gateway.discord.gg/?v=9&encoding=json", "wss").is_ok());
        assert!(check("wss://gateway-us-east1-b.discord.gg", "wss").is_ok());
        for bad in [
            "https://example.com/",
            "https://notdiscord.com/",
            "https://discord.com.evil.example/",
            "https://evildiscord.com/",
            "http://discord.com/api",
            "https://discord.com:8443/api",
            "https://user:pass@discord.com/api",
            "https://project-phoenix.vibeyard.tools/",
            "https://login.cloudflareaccess.org/",
            "https://latency.discord.media/",
            "wss://dealer.spotify.com",
            "not a url",
        ] {
            assert!(check(bad, "https").is_err(), "{bad}");
        }
        assert!(check("https://gateway.discord.gg", "wss").is_err());
        let why = check("https://example.com/secret?token=x", "https").unwrap_err();
        assert!(!why.contains("secret") && !why.contains("token"), "{why}");
    }

    #[test]
    fn redirects_resolve() {
        assert_eq!(
            resolve("https://discord.com/api/v9/x?y", "/api/v9/z").as_deref(),
            Some("https://discord.com/api/v9/z")
        );
        assert_eq!(
            resolve("https://discord.com/a", "https://example.com/").as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(resolve("https://discord.com/a", "b"), None);
    }

    #[test]
    fn http_event_shape() {
        let v = http_event(7, 200, vec![("x-a".into(), "1".into())], None, None);
        let Value::Object(f) = v else { panic!() };
        assert_eq!(f[0], (PropId(417), Value::I32(7)));
        assert_eq!(f[1], (PropId(430), Value::I32(200)));
        assert_eq!(
            f[2],
            (
                PropId(423),
                Value::Map(vec![("x-a".into(), Value::Str("1".into()))])
            )
        );
    }

    /// A refused request is answered at once, without a thread or a
    /// connection.
    #[test]
    fn refused_request_is_answered() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = got.clone();
        let net = Net::new(Box::new(move |b| g.lock().unwrap().push(b))).unwrap();
        let rec = Value::Object(vec![
            (atom::ID, Value::I32(3)),
            (atom::METHOD, Value::Str("GET".into())),
            (atom::URL, Value::Str("https://example.com/".into())),
            (atom::HEADERS, Value::Array(vec![])),
        ]);
        let note = net.call(id::NETWORKING_REQUEST, &rec, &[]);
        assert!(note.contains("REFUSED"), "{note}");
        let frames = got.lock().unwrap();
        assert_eq!(frames.len(), 1);
        let f = Frame::from_bytes(&frames[0]).unwrap();
        assert_eq!((f.kind, f.method), (frame::EVENT, id::HTTP_EVENT));
        let r = f.record_value().unwrap();
        assert_eq!(int(&r, atom::STATUS), Some(0));
        assert!(text(&r, atom::ERROR).unwrap().contains("example.com"));
    }

    /// The socket path against the real gateway, with no account: it
    /// opens, the gateway's HELLO (op 10) arrives as `text`, and a close
    /// from the guest's side ends with `close`. Reaches the network, so
    /// run it by name: `cargo test -p phoenix-view -- --ignored gateway`.
    #[test]
    #[ignore = "reaches gateway.discord.gg"]
    fn gateway_hello() {
        hello("wss://gateway.discord.gg/?v=9&encoding=json", "text");
    }

    /// The same with `compress=zstd-stream`: HELLO arrives as a binary
    /// message, inflated here, with its compressed length.
    #[test]
    #[ignore = "reaches gateway.discord.gg"]
    fn gateway_hello_zstd() {
        let r = hello(
            "wss://gateway.discord.gg/?v=9&encoding=json&compress=zstd-stream",
            "binary",
        );
        assert!(field(&r, atom::RAW_LENGTH).is_some());
    }

    fn hello(url: &str, kind: &str) -> Value {
        let (tx, rx) = mpsc::channel();
        let net = Net::new(Box::new(move |b| {
            let _ = tx.send(b);
        }))
        .unwrap();
        let open = Value::Object(vec![
            (atom::ID, Value::I32(1)),
            (atom::URL, Value::Str(url.into())),
            (atom::ORIGIN, Value::Str("https://discord.com".into())),
        ]);
        net.call(id::WEBSOCKET_OPEN, &open, &[]);
        let next = || {
            let f = Frame::from_bytes(&rx.recv_timeout(Duration::from_secs(20)).unwrap()).unwrap();
            assert_eq!(f.method, id::WS_EVENT);
            let r = f.record_value().unwrap();
            (text(&r, atom::EVENT).unwrap().to_string(), r, f.body)
        };
        assert_eq!(next().0, "open");
        let (ev, hello, body) = next();
        assert_eq!(ev, kind);
        assert!(
            contains(&body, b"\"op\":10"),
            "{}",
            String::from_utf8_lossy(&body)
        );
        let close = Value::Object(vec![
            (atom::ID, Value::I32(1)),
            (atom::CODE, Value::I32(1000)),
        ]);
        net.call(id::WEBSOCKET_CLOSE, &close, &[]);
        let (ev, r, _) = next();
        assert_eq!(ev, "close");
        assert_eq!(int(&r, atom::CODE), Some(1000));
        hello
    }
}
