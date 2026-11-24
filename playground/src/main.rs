//! The `rekindle-playground` command. See `HELP`.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode};

use playground::sanitize::{Sanitizer, strings};
use rekindle_wire::capture;
use rekindle_wire_inject::{Engine, Work};

const HELP: &str = "\
rekindle-playground: the theme playground

usage:
  rekindle-playground serve [--port N] [--no-build]
      build the wasm module and serve the playground on http://127.0.0.1:N/ (7780)
  rekindle-playground sanitize <in.capture> <out.capture> [--turn N] [--keep FILE]
      write the screen the first N turns leave (all of them by default) as a
      one-turn capture, the user's content replaced; FILE lists strings to keep,
      one per line
  rekindle-playground strings <capture> [--turn N]
      list the strings that screen holds, with the cells they are in
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("serve") => serve(&args[1..]),
        Some("sanitize") => sanitize(&args[1..]),
        Some("strings") => list_strings(&args[1..]),
        _ => {
            print!("{HELP}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rekindle-playground: {e}");
            ExitCode::FAILURE
        }
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn positional(args: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if a.starts_with("--") {
            skip = !matches!(a.as_str(), "--no-build");
        } else {
            out.push(a);
        }
    }
    out
}

/// The guest's tree after the first `turn` turns of `path` (all of them
/// when `None`).
fn screen(path: &str, turn: Option<String>) -> Result<Engine, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let (records, _) = capture::parse(&bytes).map_err(|e| format!("{path}: {e}"))?;
    let n = match turn {
        Some(t) => t.parse::<usize>().map_err(|_| format!("--turn {t}"))?,
        None => records.len(),
    };
    let mut engine = Engine::new(Work::default()).map_err(|e| e.0)?;
    for r in records.iter().take(n) {
        if let Err(e) = engine.filter(&r.bytes) {
            eprintln!("turn {}: {e}", r.turn);
        }
    }
    Ok(engine)
}

fn sanitize(args: &[String]) -> Result<(), String> {
    let pos = positional(args);
    let [input, output] = pos[..] else {
        return Err("sanitize needs <in.capture> <out.capture>".into());
    };
    let keep = match flag(args, "--keep") {
        Some(f) => std::fs::read_to_string(&f)
            .map_err(|e| format!("{f}: {e}"))?
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with("# "))
            .map(str::to_string)
            .collect(),
        None => Vec::new(),
    };
    let engine = screen(input, flag(args, "--turn"))?;
    let turn = Sanitizer::new(keep).snapshot(engine.guest());
    let mut out = capture::header().to_vec();
    capture::encode_record(0, 0, &turn.to_bytes(), &mut out);
    std::fs::write(output, out).map_err(|e| format!("{output}: {e}"))?;
    println!("{output}: {} ops", turn.ops.len());
    Ok(())
}

fn list_strings(args: &[String]) -> Result<(), String> {
    let pos = positional(args);
    let [input] = pos[..] else {
        return Err("strings needs <capture>".into());
    };
    let engine = screen(input, flag(args, "--turn"))?;
    for (s, fields) in strings(engine.guest()) {
        let fields: Vec<&str> = fields.iter().map(String::as_str).collect();
        println!("{}\t{s:?}", fields.join(","));
    }
    Ok(())
}

/// The playground's directory, beside this crate's manifest.
fn here() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Where cargo puts the wasm module.
fn wasm_path() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| here().join("../target"), PathBuf::from);
    target.join("wasm32-unknown-unknown/release/playground.wasm")
}

fn build_wasm() -> Result<(), String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args([
            "build",
            "--lib",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "-p",
            "rekindle-playground",
        ])
        .current_dir(here())
        .status()
        .map_err(|e| format!("running cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("the wasm build failed".into())
    }
}

fn serve(args: &[String]) -> Result<(), String> {
    if !args.iter().any(|a| a == "--no-build") {
        build_wasm()?;
    }
    let port = flag(args, "--port").unwrap_or_else(|| "7780".into());
    let listener =
        TcpListener::bind(format!("127.0.0.1:{port}")).map_err(|e| format!("port {port}: {e}"))?;
    println!("rekindle-playground: http://127.0.0.1:{port}/");
    // A browser opens connections it may never send on, so each one gets
    // its own thread.
    for stream in listener.incoming().flatten() {
        std::thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
            if let Err(e) = answer(stream) {
                eprintln!("{e}");
            }
        });
    }
    Ok(())
}

/// The file a request path names: the wasm module, a fixture, one of the
/// user's own files in `local/`, or a file of the page. `None` for a path that leaves those directories.
fn file_for(path: &str) -> Option<PathBuf> {
    let path = path.split(['?', '#']).next()?.trim_start_matches('/');
    if path.is_empty() {
        return Some(here().join("web/index.html"));
    }
    if Path::new(path)
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return None;
    }
    Some(match path {
        "playground.wasm" => wasm_path(),
        p if p.starts_with("fixtures/") || p.starts_with("local/") => here().join(p),
        p => here().join("web").join(p),
    })
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("wasm") => "application/wasm",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ttf") => "font/ttf",
        _ => "application/octet-stream",
    }
}

fn answer(mut stream: TcpStream) -> Result<(), String> {
    let mut line = String::new();
    let mut reader = BufReader::new(&stream);
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    // The headers, read so closing the connection doesn't reset it.
    let mut header = String::new();
    while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
        header.clear();
    }
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or("/"));
    let found = (method == "GET")
        .then(|| file_for(path))
        .flatten()
        .and_then(|f| std::fs::read(&f).ok().map(|b| (f, b)));
    let response = match found {
        Some((file, body)) => {
            let mut r = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                content_type(&file),
                body.len()
            )
            .into_bytes();
            r.extend(body);
            r
        }
        None => {
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
        }
    };
    stream.write_all(&response).map_err(|e| e.to_string())
}
