//! `pick`: selectors for the element the user presses in a split client.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{ArgGroup, Subcommand};
use rekindle_wire::Tree;
use rekindle_wire_inject::{Candidate, Pick, pick};

/// The file a host in a phone app reads a pick request from, in its config
/// dir, and the file it writes the pick to.
const REQUEST: &str = "pick.req";
const ANSWER: &str = "pick.json";

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Make selectors for the next element pressed in a split client, or
    /// for a node of a captured tree, ranked by how well they survive a
    /// new build.
    #[command(group(ArgGroup::new("from").required(true).args(["port", "dir", "capture"])))]
    Pick {
        /// The host's debug port (`REKINDLE_HOST_DEBUG_PORT`).
        #[arg(long)]
        port: Option<u16>,
        /// The host's config dir, where a phone app's host reads `pick.req`
        /// and writes `pick.json`.
        #[arg(long)]
        dir: Option<PathBuf>,
        /// A capture of the guest's turns (`guest.capture` from a tree dump).
        #[arg(long, requires = "node")]
        capture: Option<PathBuf>,
        /// Pick this node instead of waiting for a press.
        #[arg(long)]
        node: Option<u32>,
        /// Seconds to wait for the press.
        #[arg(long, default_value_t = 30)]
        wait: u64,
        /// Print the pick as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub(crate) fn run(c: Commands) -> Result<()> {
    let Commands::Pick {
        port,
        dir,
        capture,
        node,
        wait,
        json,
    } = c;
    let wait = Duration::from_secs(wait);
    let picked = if let Some(port) = port {
        from_port(port, node, wait)?
    } else if let Some(dir) = dir {
        from_dir(&dir, node, wait)?
    } else if let (Some(capture), Some(node)) = (capture, node) {
        from_capture(&capture, node)?
    } else {
        unreachable!("clap requires one source")
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&picked)?);
    } else {
        print(&picked);
    }
    Ok(())
}

/// `GET /pick` on the debug port, which answers once a press arrives.
fn from_port(port: u16, node: Option<u32>, wait: Duration) -> Result<Pick> {
    let path = match node {
        Some(n) => format!("/pick?node={n}"),
        None => format!("/pick?wait={}", wait.as_secs()),
    };
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        .with_context(|| format!("no debug endpoint on port {port}"))?;
    stream.set_read_timeout(Some(wait + Duration::from_secs(5)))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )?;
    if node.is_none() {
        eprintln!("Press an element in the client.");
    }
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status)?;
    let mut line = String::new();
    while reader.read_line(&mut line)? > 2 {
        line.clear();
    }
    let mut body = String::new();
    reader.read_to_string(&mut body)?;
    if !status.contains(" 200 ") {
        bail!("{}: {}", status.trim(), error_of(&body));
    }
    serde_json::from_str(&body).context("reading the pick")
}

/// Leave `pick.req` in the config dir and wait for `pick.json`.
fn from_dir(dir: &Path, node: Option<u32>, wait: Duration) -> Result<Pick> {
    let answer = dir.join(ANSWER);
    let _ = std::fs::remove_file(&answer);
    let request = node.map_or_else(|| "press".to_string(), |n| n.to_string());
    std::fs::write(dir.join(REQUEST), format!("{request}\n"))
        .with_context(|| format!("writing {}", dir.join(REQUEST).display()))?;
    if node.is_none() {
        eprintln!("Press an element in the client.");
    }
    let start = Instant::now();
    loop {
        if let Ok(body) = std::fs::read_to_string(&answer)
            && !body.trim().is_empty()
        {
            return serde_json::from_str(&body).map_err(|_| anyhow::anyhow!(error_of(&body)));
        }
        if start.elapsed() > wait + Duration::from_secs(2) {
            let _ = std::fs::write(dir.join(REQUEST), "cancel\n");
            bail!("no {ANSWER} in {} after {}s", dir.display(), wait.as_secs());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Apply every turn of a capture and pick `node` in the tree they leave.
fn from_capture(path: &Path, node: u32) -> Result<Pick> {
    let buf = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let (records, _) = rekindle_wire::capture::parse(&buf)
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let mut tree = Tree::new();
    for r in &records {
        tree.apply_bytes(&r.bytes)
            .map_err(|e| anyhow::anyhow!("turn {}: {e:?}", r.turn))?;
    }
    pick::pick(&tree, node).with_context(|| format!("no node {node} in the capture's tree"))
}

fn error_of(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
        .unwrap_or_else(|| body.trim().to_string())
}

fn print(p: &Pick) {
    println!("#{} {}  {}", p.node, p.kind.name(), p.name);
    println!(
        "{}",
        serde_json::to_string(&p.best.selector).unwrap_or_default()
    );
    println!();
    for c in std::iter::once(&p.best).chain(&p.fallbacks) {
        line(c);
    }
}

fn line(c: &Candidate) {
    let basis = serde_json::to_value(c.basis)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let matches = if c.unique() {
        "unique".to_string()
    } else {
        format!("{} nodes", c.matches)
    };
    println!(
        "{:>4}  {basis:<12} {matches:<9} {}",
        c.score,
        serde_json::to_string(&c.selector).unwrap_or_default()
    );
}
