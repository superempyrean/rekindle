//! The inspector's link to its window: a socket on
//! localhost carrying one JSON object per line.
//!
//! The host listens on a free port and writes
//! `<temp>/rekindle-inspect/<pid>.json` with the port, the pid and the exe.
//! The window lists those files to find the apps it can inspect.
//!
//! Host to window: `hello` (on connect: the app, the mode, the current
//! selection), `hover` (the element under the pointer, when it changes, in
//! inspect mode), `selected` (everything known about a clicked element),
//! `mode`. Window to host: `mode` (`{"inspect": bool}`) and `clear`.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Mutex, Once};

use serde_json::{Value, json};

static CLIENTS: Mutex<Vec<TcpStream>> = Mutex::new(Vec::new());
static STARTED: Once = Once::new();

/// Where each inspectable app's `<pid>.json` is.
pub(crate) fn discovery_dir() -> PathBuf {
    std::env::temp_dir().join("rekindle-inspect")
}

/// Listen for the window, once per process.
pub(crate) fn start() {
    STARTED.call_once(|| {
        if let Err(e) = listen() {
            {
                use std::io::Write as _;
                // stderr can be a pipe nobody reads, and eprintln! panics on one.
                let _ = writeln!(
                    std::io::stderr(),
                    "rekindle inspector: no link to its window: {e}"
                );
            }
        }
    });
}

fn listen() -> std::io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let dir = discovery_dir();
    std::fs::create_dir_all(&dir)?;
    let info = json!({ "pid": std::process::id(), "port": port, "exe": exe });
    std::fs::write(
        dir.join(format!("{}.json", std::process::id())),
        info.to_string(),
    )?;
    std::thread::Builder::new()
        .name("rekindle-inspect-link".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                connected(stream);
            }
        })?;
    Ok(())
}

fn connected(stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    // A window that stops reading can't stall the client's frames.
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_millis(50)));
    let Ok(reader) = stream.try_clone() else {
        return;
    };
    let mut writer = stream;
    if writer
        .write_all(format!("{}\n", crate::inspector::hello()).as_bytes())
        .is_err()
    {
        return;
    }
    CLIENTS.lock().unwrap().push(writer);
    let _ = std::thread::Builder::new()
        .name("rekindle-inspect-reader".into())
        .spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                match v["type"].as_str() {
                    Some("mode") => {
                        if let Some(on) = v["inspect"].as_bool() {
                            crate::inspector::set_inspecting(on);
                        }
                    }
                    Some("clear") => crate::inspector::clear_selection(),
                    _ => {}
                }
            }
        });
}

/// Send `v` to every connected window, dropping any that has gone.
pub(crate) fn broadcast(v: &Value) {
    let line = format!("{v}\n");
    CLIENTS
        .lock()
        .unwrap()
        .retain_mut(|c| c.write_all(line.as_bytes()).is_ok());
}
