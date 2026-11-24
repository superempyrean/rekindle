//! A live debug endpoint: the recent turns and UI events the wire detours
//! saw, and the tree rekindle mirrors, as JSON on a local port.
//!
//! It is off unless `REKINDLE_HOST_DEBUG_PORT` names a port, and it listens
//! on `127.0.0.1` only. `GET /ops` is the last [`KEEP`] turns, decoded,
//! `GET /events` the last [`KEEP`] UI events, and `GET /tree` the mirrored
//! tree with each node's cells.
//!
//! Host calls and host frames are never recorded: they carry the client's
//! HTTP requests and responses, headers and bodies included. What an input
//! holds or sends is replaced by its length: the `value`, `forceValue` and
//! `valueSpans` cells of every node, and every string in a UI event (a
//! `changeText` from a password field is one).

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use rekindle_wire::event::{self, UiEvent};
use rekindle_wire::turn::NodeOp;
use rekindle_wire::{PropId, Tree, Turn, Value};
use serde_json::{Map, Value as Json, json};

const PORT_ENV: &str = "REKINDLE_HOST_DEBUG_PORT";
/// How many turns and events each list keeps.
pub(crate) const KEEP: usize = 200;
/// Cells whose text is the user's input.
const INPUT_CELLS: [&str; 3] = ["value", "forceValue", "valueSpans"];

static ON: AtomicBool = AtomicBool::new(false);
static SEQ: AtomicU64 = AtomicU64::new(0);
static OPS: Mutex<VecDeque<Json>> = Mutex::new(VecDeque::new());
static EVENTS: Mutex<VecDeque<Json>> = Mutex::new(VecDeque::new());
static START: Mutex<Option<Instant>> = Mutex::new(None);

/// Start the endpoint if `REKINDLE_HOST_DEBUG_PORT` asks for it. Returns
/// the port it listens on.
pub(crate) fn start() -> Option<u16> {
    let port: u16 = std::env::var(PORT_ENV).ok()?.trim().parse().ok()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).ok()?;
    let port = listener.local_addr().ok()?.port();
    *START
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Instant::now());
    ON.store(true, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("rekindle-debug".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = serve(stream);
            }
        })
        .ok()?;
    Some(port)
}

/// Record a turn the guest emitted (its op stream, after the leading word).
pub(crate) fn turn(ops: &[u8]) {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    let entry = match Turn::from_bytes(ops) {
        Ok(t) => json!({
            "seq": SEQ.fetch_add(1, Ordering::Relaxed),
            "ms": elapsed_ms(),
            "ops": t.ops.iter().map(op_json).collect::<Vec<_>>(),
        }),
        Err(e) => json!({
            "seq": SEQ.fetch_add(1, Ordering::Relaxed),
            "ms": elapsed_ms(),
            "error": format!("{e:?}"),
            "bytes": ops.len(),
        }),
    };
    push(&OPS, entry);
}

/// Record an event the client dispatched to the guest, if it is a UI event.
/// Host frames are skipped.
pub(crate) fn event(bytes: &[u8]) {
    if !ON.load(Ordering::Relaxed) || !event::is_ui(bytes) {
        return;
    }
    let body = match UiEvent::from_bytes(bytes) {
        Ok(UiEvent::Node(e)) => json!({
            "op": "ui",
            "node": e.node,
            "name": prop_name(e.name),
            "value": redacted(&e.value),
        }),
        Ok(other) => json!({ "op": other.name(), "detail": redact_debug(&other) }),
        Err(e) => json!({ "error": format!("{e:?}"), "bytes": bytes.len() }),
    };
    push(
        &EVENTS,
        json!({ "seq": SEQ.fetch_add(1, Ordering::Relaxed), "ms": elapsed_ms(), "event": body }),
    );
}

/// The mirrored tree: every node with its kind, children and cells.
pub(crate) fn tree_json(tree: &Tree) -> Json {
    let nodes: Vec<Json> = tree
        .nodes()
        .map(|n| {
            let mut cells = Map::new();
            for (p, v) in tree.props().node_props(n.id) {
                let name = prop_name(p);
                let v = if INPUT_CELLS.contains(&name.as_str()) {
                    redacted(v)
                } else {
                    value_json(v)
                };
                cells.insert(name, v);
            }
            json!({ "id": n.id, "kind": n.kind.to_string(), "children": n.children, "cells": cells })
        })
        .collect();
    json!({ "root": tree.root(), "focus": tree.focus(), "nodes": nodes })
}

fn push(list: &Mutex<VecDeque<Json>>, entry: Json) {
    let mut l = list
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if l.len() == KEEP {
        l.pop_front();
    }
    l.push_back(entry);
}

fn elapsed_ms() -> u64 {
    START
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .map_or(0, |s| s.elapsed().as_millis() as u64)
}

fn prop_name(p: PropId) -> String {
    p.name().map_or_else(|| format!("#{}", p.0), str::to_string)
}

fn props_json(props: &[(PropId, Value)]) -> Json {
    let mut m = Map::new();
    for (p, v) in props {
        let name = prop_name(*p);
        let v = if INPUT_CELLS.contains(&name.as_str()) {
            redacted(v)
        } else {
            value_json(v)
        };
        m.insert(name, v);
    }
    Json::Object(m)
}

fn op_json(op: &NodeOp) -> Json {
    match op {
        NodeOp::Create { node, kind, props } => {
            json!({ "op": "create", "node": node, "kind": kind.to_string(), "props": props_json(props) })
        }
        NodeOp::Update { node, props } => {
            json!({ "op": "update", "node": node, "props": props_json(props) })
        }
        NodeOp::Children { node, children } => {
            json!({ "op": "children", "node": node, "children": children })
        }
        NodeOp::Delete { nodes } => json!({ "op": "delete", "nodes": nodes }),
        NodeOp::Root { node } => json!({ "op": "root", "node": node }),
        NodeOp::Focus { node } => json!({ "op": "focus", "node": node }),
        NodeOp::ScrollTo { node, value } => {
            json!({ "op": "scrollTo", "node": node, "value": value_json(value) })
        }
        NodeOp::RequestFrame => json!({ "op": "requestFrame" }),
        NodeOp::Motion { words } => json!({ "op": "motion", "words": words.len() }),
        NodeOp::ChildEdits { node, edits } => {
            json!({ "op": "childEdits", "node": node, "edits": format!("{edits:?}") })
        }
    }
}

/// A wire value as JSON. Objects keyed by `PropId` use the names.
fn value_json(v: &Value) -> Json {
    match v {
        Value::Null => Json::Null,
        Value::Bool(b) => json!(b),
        Value::I32(n) => json!(n),
        Value::I64(n) => json!(n),
        Value::F64(f) => json!(f),
        Value::Str(s) => json!(s),
        Value::Array(items) => Json::Array(items.iter().map(value_json).collect()),
        Value::Object(props) => props_json(props),
        Value::Quad(q) => json!(q),
        Value::Map(entries) => Json::Object(
            entries
                .iter()
                .map(|(k, v)| (k.clone(), value_json(v)))
                .collect(),
        ),
        Value::U32Array(words) => json!(words),
        Value::Sparse { list, pairs } => json!({ "list": list, "pairs": pairs }),
    }
}

/// A value with every string replaced by its length.
fn redacted(v: &Value) -> Json {
    match v {
        Value::Str(s) => json!(format!("<{} chars>", s.chars().count())),
        Value::Array(items) => Json::Array(items.iter().map(redacted).collect()),
        Value::Object(props) => Json::Object(
            props
                .iter()
                .map(|(p, v)| (prop_name(*p), redacted(v)))
                .collect(),
        ),
        Value::Map(entries) => Json::Object(
            entries
                .iter()
                .map(|(k, v)| (k.clone(), redacted(v)))
                .collect(),
        ),
        other => value_json(other),
    }
}

/// A non-node UI event's fields. They carry no text the user typed, but
/// any quoted string is cut to its length all the same.
fn redact_debug(e: &UiEvent) -> String {
    let text = format!("{e:?}");
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            out.push(c);
            continue;
        }
        let mut n = 0;
        for c in chars.by_ref() {
            if c == '"' {
                break;
            }
            n += 1;
        }
        let _ = write!(out, "<{n} chars>");
    }
    out
}

fn serve(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let path = line.split_whitespace().nth(1).unwrap_or("/");
    let (status, body) = match (line.starts_with("GET "), path) {
        (true, "/ops") => ("200 OK", list(&OPS)),
        (true, "/events") => ("200 OK", list(&EVENTS)),
        (true, "/tree") => (
            "200 OK",
            crate::wire::mirror(tree_json).unwrap_or(Json::Null),
        ),
        (true, "/") => (
            "200 OK",
            json!({ "endpoints": ["/ops", "/events", "/tree"] }),
        ),
        _ => (
            "404 Not Found",
            json!({ "error": "GET /ops, /events or /tree" }),
        ),
    };
    let body = body.to_string();
    let mut stream = stream;
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

fn list(l: &Mutex<VecDeque<Json>>) -> Json {
    Json::Array(
        l.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_text_is_cut_to_its_length() {
        let value = PropId::from_name("value").unwrap();
        let text = PropId::from_name("text").unwrap();
        let props = vec![
            (value, Value::Str("hunter2".into())),
            (text, Value::Str("Password".into())),
        ];
        let j = props_json(&props);
        assert_eq!(j["value"], json!("<7 chars>"));
        assert_eq!(j["text"], json!("Password"), "labels stay");
        assert_eq!(
            redacted(&Value::Map(vec![("text".into(), Value::Str("abc".into()))])),
            json!({ "text": "<3 chars>" })
        );
    }

    #[test]
    fn quoted_strings_in_other_events_are_cut() {
        let s = redact_debug(&UiEvent::Back);
        assert!(!s.contains('"'));
    }

    #[test]
    fn lists_keep_the_newest() {
        let l = Mutex::new(VecDeque::new());
        for i in 0..KEEP + 5 {
            push(&l, json!(i));
        }
        let all = list(&l);
        assert_eq!(all.as_array().unwrap().len(), KEEP);
        assert_eq!(all[0], json!(5));
    }
}
