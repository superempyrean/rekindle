//! The preview server: a `tiny_http` listener on loopback plus the session loop.
//!
//! Threads:
//! - **main** (the caller of [`serve`]) owns the [`Session`] and so the
//!   runtime; every wasm call happens here. It handles API jobs and ticks the
//!   session (builds, watchers, logs) between them.
//! - **http** accepts connections, serves the page, validates requests, and
//!   forwards API actions to main over a channel. It never touches the runtime.
//! - **waiters**: one short-lived thread per `/api/wait` long-poll, parked on
//!   a condvar until the revision moves.

use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::{Result, bail};
use serde_json::json;
use tiny_http::{Header, Request, Response, Server};

use super::api::{self, Action, ApiError, Method, Route};
use super::session::Session;
use super::term;

const PAGE: &str = include_str!("preview.html");
const WAIT_TIMEOUT: Duration = Duration::from_secs(25);
const MAX_WAITERS: usize = 32;
const TICK: Duration = Duration::from_millis(150);

pub(super) struct ServeOptions {
    /// Preferred port; the next few are tried if it's taken.
    pub(super) port: u16,
    /// Open the preview in the default browser once listening.
    pub(super) open_browser: bool,
}

/// A revision counter that long-polls can wait on.
#[derive(Default)]
struct Revision {
    rev: Mutex<u64>,
    cv: Condvar,
}

impl Revision {
    fn get(&self) -> u64 {
        *self.rev.lock().unwrap()
    }

    fn bump(&self) {
        *self.rev.lock().unwrap() += 1;
        self.cv.notify_all();
    }

    /// Block until the revision differs from `seen` (or the timeout passes).
    fn wait_past(&self, seen: u64, timeout: Duration) -> u64 {
        let r = self.rev.lock().unwrap();
        let (r, _) = self
            .cv
            .wait_timeout_while(r, timeout, |r| *r == seen)
            .unwrap();
        *r
    }
}

struct Job {
    action: Action,
    req: Request,
}

fn bind(port: u16) -> Result<(Server, u16)> {
    let mut last = String::new();
    for p in port..port.saturating_add(10) {
        match Server::http(("127.0.0.1", p)) {
            Ok(s) => return Ok((s, p)),
            Err(e) => last = e.to_string(),
        }
    }
    bail!(
        "could not listen on 127.0.0.1:{port}..{}: {last}",
        port.saturating_add(9)
    )
}

/// Serve `session` until the process is interrupted.
pub(super) fn serve(mut session: Session, opts: &ServeOptions) -> Result<()> {
    let (server, port) = bind(opts.port)?;
    if port != opts.port {
        term::warn(&format!("port {} is busy; using {port}", opts.port));
    }
    let url = format!("http://127.0.0.1:{port}/");
    term::line("");
    term::url(&format!("{} preview:", session.title()), &url);
    term::dim("logs stream below; Ctrl-C to stop");
    term::line("");
    if opts.open_browser {
        open_browser(&url);
    }
    run(&mut session, server, port, || None::<()>)
}

/// Serve `session` on a free loopback port, with nothing printed, while the
/// process `start` launches (given the port) runs; then return its exit
/// status. `rekindle test` runs the project's tests this way.
pub(super) fn serve_for(
    session: &mut Session,
    start: impl FnOnce(u16) -> Result<std::process::Child>,
) -> Result<std::process::ExitStatus> {
    let server = Server::http(("127.0.0.1", 0))
        .map_err(|e| anyhow::anyhow!("could not listen on loopback: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .ok_or_else(|| anyhow::anyhow!("the server has no port"))?;
    let mut child = start(port)?;
    let status = run(session, server, port, || child.try_wait().ok().flatten());
    if status.is_err() {
        // The child may have exited already; either way it isn't running.
        let _ = child.kill();
    }
    status
}

/// Serve `session` on a free loopback port until stdin closes. The first
/// line on stdout is `READY <port> <plugin>`, for the process that started
/// this one (`rekindle test-server`, which `rekindle-test` starts).
pub(super) fn serve_until_closed(session: &mut Session, plugin: &str) -> Result<()> {
    use std::io::{Read as _, Write as _};
    use std::sync::atomic::AtomicBool;

    let server = Server::http(("127.0.0.1", 0))
        .map_err(|e| anyhow::anyhow!("could not listen on loopback: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .ok_or_else(|| anyhow::anyhow!("the server has no port"))?;
    let closed = Arc::new(AtomicBool::new(false));
    {
        let closed = closed.clone();
        std::thread::Builder::new()
            .name("rekindle-stdin".into())
            .spawn(move || {
                let mut sink = [0u8; 256];
                let mut stdin = std::io::stdin();
                while matches!(stdin.read(&mut sink), Ok(n) if n > 0) {}
                closed.store(true, Ordering::Relaxed);
            })?;
    }
    let mut out = std::io::stdout();
    writeln!(out, "READY {port} {plugin}")?;
    out.flush()?;
    run(session, server, port, || {
        closed.load(Ordering::Relaxed).then_some(())
    })
}

/// The session loop: API jobs and ticks until `done` returns something.
fn run<T>(
    session: &mut Session,
    server: Server,
    port: u16,
    mut done: impl FnMut() -> Option<T>,
) -> Result<T> {
    let revision = Arc::new(Revision::default());
    let (tx, rx) = channel::<Job>();
    {
        let revision = revision.clone();
        std::thread::Builder::new()
            .name("rekindle-http".into())
            .spawn(move || http_loop(&server, port, &tx, &revision))?;
    }

    let mut seen = session.fingerprint();
    loop {
        if let Some(out) = done() {
            return Ok(out);
        }
        let job = match rx.recv_timeout(TICK) {
            Ok(job) => Some(job),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => bail!("the HTTP server stopped"),
        };
        let outcome = job.map(|j| {
            let r = session.act(j.action);
            (j.req, r)
        });
        session.tick();
        let fp = session.fingerprint();
        if fp != seen {
            seen = fp;
            revision.bump();
        }
        if let Some((req, r)) = outcome {
            let rev = revision.get();
            let resp = match r {
                Ok(()) => json_response(200, &session.state(rev)),
                Err(e) => error_response(&e, Some(&session.state(rev))),
            };
            respond(req, resp);
        }
    }
}

fn http_loop(server: &Server, port: u16, tx: &Sender<Job>, revision: &Arc<Revision>) {
    let waiters = Arc::new(AtomicUsize::new(0));
    for mut req in server.incoming_requests() {
        let header = |name: &str| {
            req.headers()
                .iter()
                .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
                .map(|h| h.value.as_str().to_string())
        };
        if !api::allowed_origin(header("Host").as_deref(), header("Origin").as_deref(), port) {
            respond(
                req,
                error_response(&ApiError::new(403, "forbidden: wrong Host/Origin"), None),
            );
            continue;
        }
        let method = match req.method() {
            tiny_http::Method::Get => Method::Get,
            tiny_http::Method::Post => Method::Post,
            _ => Method::Other,
        };
        if method == Method::Post {
            // A JSON content type forces a CORS preflight for cross-site
            // fetches, which we never approve.
            let ct = header("Content-Type").unwrap_or_default();
            if !ct.to_ascii_lowercase().starts_with("application/json") {
                respond(
                    req,
                    error_response(
                        &ApiError::new(415, "POST bodies must be application/json"),
                        None,
                    ),
                );
                continue;
            }
        }
        let body = match read_body(&mut req) {
            Ok(b) => b,
            Err(e) => {
                respond(req, error_response(&e, None));
                continue;
            }
        };
        let url = req.url().to_string();
        match api::route(method, &url, &body) {
            Ok(Route::Page) => {
                let resp = Response::from_string(PAGE)
                    .with_header(header_kv("Content-Type", "text/html; charset=utf-8"))
                    .with_header(header_kv("Cache-Control", "no-store"))
                    .with_header(header_kv(
                        "Content-Security-Policy",
                        "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; img-src data:",
                    ));
                respond(req, resp);
            }
            Ok(Route::Wait { rev }) => {
                if waiters.load(Ordering::SeqCst) >= MAX_WAITERS {
                    respond(req, json_response(200, &json!({ "rev": revision.get() })));
                    continue;
                }
                waiters.fetch_add(1, Ordering::SeqCst);
                let (revision, waiters) = (revision.clone(), waiters.clone());
                std::thread::spawn(move || {
                    let now = revision.wait_past(rev, WAIT_TIMEOUT);
                    respond(req, json_response(200, &json!({ "rev": now })));
                    waiters.fetch_sub(1, Ordering::SeqCst);
                });
            }
            Ok(Route::Api(action)) => {
                if let Err(e) = tx.send(Job { action, req }) {
                    respond(
                        e.0.req,
                        error_response(&ApiError::new(503, "session stopped"), None),
                    );
                    return;
                }
            }
            Err(e) => {
                respond(req, error_response(&e, None));
            }
        }
    }
}

fn read_body(req: &mut Request) -> Result<Vec<u8>, ApiError> {
    if req.body_length().is_some_and(|n| n > api::MAX_BODY) {
        return Err(ApiError::new(413, "request body too large"));
    }
    let mut body = Vec::new();
    req.as_reader()
        .take(api::MAX_BODY as u64 + 1)
        .read_to_end(&mut body)
        .map_err(|e| ApiError::bad(format!("reading body: {e}")))?;
    if body.len() > api::MAX_BODY {
        return Err(ApiError::new(413, "request body too large"));
    }
    Ok(body)
}

/// Sends `resp`. A request that can't be answered was dropped by its
/// client, and there's no one left to tell.
fn respond(req: Request, resp: Response<impl Read>) {
    let _ = req.respond(resp);
}

fn header_kv(k: &str, v: &str) -> Header {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).expect("static header is valid")
}

fn json_response(status: u16, v: &serde_json::Value) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_data(serde_json::to_vec(v).unwrap_or_default())
        .with_status_code(status)
        .with_header(header_kv("Content-Type", "application/json"))
        .with_header(header_kv("Cache-Control", "no-store"))
}

/// Errors carry the current state too (when available), so the page can
/// re-render (e.g. a stale panel) after a refused interaction.
fn error_response(
    e: &ApiError,
    state: Option<&serde_json::Value>,
) -> Response<std::io::Cursor<Vec<u8>>> {
    json_response(e.status, &json!({ "error": e.message, "state": state }))
}

fn open_browser(url: &str) {
    let r = if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = r {
        term::warn(&format!("could not open a browser ({e}); visit {url}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_wakes_waiters() {
        let r = Arc::new(Revision::default());
        assert_eq!(
            r.wait_past(5, Duration::from_millis(1)),
            0,
            "already past: returns at once"
        );
        let r2 = r.clone();
        let t = std::thread::spawn(move || r2.wait_past(0, Duration::from_secs(10)));
        std::thread::sleep(Duration::from_millis(50));
        r.bump();
        assert_eq!(t.join().unwrap(), 1);
        assert_eq!(
            r.wait_past(1, Duration::from_millis(20)),
            1,
            "times out unchanged"
        );
    }
}
