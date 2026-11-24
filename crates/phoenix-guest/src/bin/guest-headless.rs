//! Runs Phoenix's guest headless and records every buffer it emits.
//!
//! ```text
//! guest-headless <workdir> <liblibdiscord_wasm_host.so> [seconds] [VAR=value ...]
//! ```
//!
//! The library is copied into `<workdir>/lib`. Each run writes
//! `<workdir>/out/<time>/log.txt`, `turns.bin` (a `u32` length and the
//! bytes, per emit) and `capture.bin` (the render turns, in the capture
//! format the wire tooling reads), and `<workdir>/out/last.txt` names the
//! newest. On
//! Windows the guest's `/` is `<workdir>/root` and its `HOME` is
//! `/sandbox`; on Linux its `HOME` is `<workdir>/sandbox`.
//!
//! Variables: `PRESS=<node>` sends a press after ten frames, `TRACE=0|1|2`
//! sets import logging (default 1), `NO_REPLY=1` answers no host call, and
//! `NO_BATCH=1` leaves `UIManager.batchEvents` unanswered.

#[cfg(all(target_arch = "x86_64", any(windows, target_os = "linux")))]
fn main() {
    if let Err(e) = run::main() {
        eprintln!("guest-headless: {e:#}");
        std::process::exit(1);
    }
}

#[cfg(not(all(target_arch = "x86_64", any(windows, target_os = "linux"))))]
fn main() {
    eprintln!("guest-headless runs on x86_64 Windows or Linux");
    std::process::exit(1);
}

#[cfg(all(target_arch = "x86_64", any(windows, target_os = "linux")))]
mod run {
    use std::collections::HashMap;
    use std::fmt::Write as _;
    use std::fs::File;
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use anyhow::{Context as _, Result, bail};
    use phoenix_guest::{Guest, Options, Reply, Transport, log};
    use rekindle_wire::capture;

    /// Host method ids, from the id table of build 356207.
    const STORAGE_GET: u16 = 214;
    const STORAGE_SET: u16 = 217;
    const BATCH_EVENTS: u16 = 238;
    /// Record keys.
    const KEY: u16 = 405;

    struct Vars(HashMap<String, String>);

    impl Vars {
        fn get(&self, k: &str) -> Option<&str> {
            self.0.get(k).map(String::as_str)
        }
    }

    /// Records every emit and answers the host calls the Java platform
    /// would: `UIManager.batchEvents`, and `Storage` in memory. Secure keys
    /// are never answered.
    struct Recorder {
        turns: Mutex<File>,
        capture: Mutex<(u64, capture::Writer<File>)>,
        emits: AtomicU64,
        storage: Mutex<HashMap<String, Vec<u8>>>,
        no_reply: bool,
        no_batch: bool,
    }

    fn hex(p: &[u8], max: usize) -> String {
        let mut s = String::new();
        for b in p.iter().take(max) {
            let _ = write!(s, "{b:02x}");
        }
        if p.len() > max {
            s.push_str("...");
        }
        s
    }

    /// The `key` field (Str) of a Storage record, an object of Str and Bool
    /// fields, and whether its one Bool field (`secure`) is true.
    fn record_key(r: &[u8]) -> Option<(String, bool)> {
        if r.len() < 5 || r[0] != 8 {
            return None;
        }
        let count = u32::from_le_bytes(r[1..5].try_into().ok()?);
        let (mut p, mut key, mut secure) = (5, None, false);
        for _ in 0..count {
            let k = u16::from_le_bytes(r.get(p..p + 2)?.try_into().ok()?);
            let t = *r.get(p + 2)?;
            p += 3;
            match t {
                6 => {
                    let l = u32::from_le_bytes(r.get(p..p + 4)?.try_into().ok()?) as usize;
                    p += 4;
                    if k == KEY {
                        key = Some(String::from_utf8_lossy(r.get(p..p + l)?).into_owned());
                    }
                    p += l;
                }
                1 | 2 if k != KEY => secure = t == 2,
                1 | 2 => {}
                _ => break,
            }
        }
        key.map(|k| (k, secure))
    }

    impl Recorder {
        fn reply(reply: &Reply<'_>, call: &[u8], rec: &[u8], body: &[u8]) {
            let mut b = call[..16].to_vec();
            b[0] = 2;
            b[12..16].copy_from_slice(&(rec.len() as u32).to_le_bytes());
            b.extend_from_slice(rec);
            b.extend_from_slice(body);
            reply.send(&b);
            log(&format!("[reply] {}", hex(&b, 64).trim_end_matches("...")));
        }

        fn answer(&self, p: &[u8], reply: &Reply<'_>) {
            if p.len() < 16 || p[0] != 1 || !reply.is_some() {
                return;
            }
            let method = u16::from_le_bytes([p[4], p[5]]);
            let rlen = u32::from_le_bytes(p[12..16].try_into().unwrap()) as usize;
            let Some(rec) = p.get(16..16 + rlen) else {
                return;
            };
            let body = &p[16 + rlen..];
            match method {
                BATCH_EVENTS if !self.no_batch => {
                    // {version: 1, frameMs: 16.67}
                    let mut r = vec![8, 2, 0, 0, 0, 0x86, 0x01, 3, 1, 0, 0, 0, 0xe4, 0x01, 5];
                    r.extend((1000.0f64 / 60.0).to_le_bytes());
                    Self::reply(reply, p, &r, &[]);
                }
                STORAGE_GET => {
                    let Some((key, secure)) = record_key(rec) else {
                        return;
                    };
                    let st = self.storage.lock().unwrap();
                    match st.get(&key).filter(|_| !secure) {
                        Some(v) => Self::reply(reply, p, &[8, 1, 0, 0, 0, 0x96, 0x01, 2], v),
                        None => Self::reply(reply, p, &[8, 1, 0, 0, 0, 0x96, 0x01, 1], &[]),
                    }
                }
                STORAGE_SET => {
                    if let Some((key, false)) = record_key(rec) {
                        self.storage.lock().unwrap().insert(key, body.to_vec());
                    }
                }
                _ => {}
            }
        }
    }

    impl Transport for Recorder {
        fn emit(&self, p: &[u8], reply: Reply<'_>) -> bool {
            let mut turns = self.turns.lock().unwrap();
            let k = self.emits.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = turns.write_all(&(p.len() as u32).to_le_bytes());
            let _ = turns.write_all(p);
            let is_turn = p.len() >= 4 && p[..4] == [0; 4];
            let kind = if is_turn { "turn" } else { "frame" };
            if is_turn {
                let mut c = self.capture.lock().unwrap();
                c.0 += 1;
                let n = c.0;
                let _ = c.1.record(n, &p[4..]);
            }
            log(&format!(
                "[emit #{k}] {kind} len={} tid={} {}",
                p.len(),
                phoenix_guest::thread_id(),
                hex(p, 96)
            ));
            if !self.no_reply {
                self.answer(p, &reply);
            }
            true
        }

        fn notify(&self) {
            log(&format!("[notify] tid={}", phoenix_guest::thread_id()));
        }
    }

    fn stamp() -> String {
        let s = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        format!("{:02}{:02}{:02}", (s / 3600) % 24, (s / 60) % 60, s % 60)
    }

    fn out_dir(work: &Path) -> Result<PathBuf> {
        let base = stamp();
        let mut name = base.clone();
        let mut n = 1;
        while work.join("out").join(&name).exists() {
            n += 1;
            name = format!("{base}-{n}");
        }
        let out = work.join("out").join(&name);
        std::fs::create_dir_all(&out)?;
        std::fs::write(work.join("out").join("last.txt"), format!("{name}\n"))?;
        Ok(out)
    }

    pub(super) fn main() -> Result<()> {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let (pos, vars): (Vec<&String>, Vec<&String>) = args.iter().partition(|a| !a.contains('='));
        let [work, so, rest @ ..] = pos.as_slice() else {
            bail!(
                "usage: guest-headless <workdir> <liblibdiscord_wasm_host.so> [seconds] [VAR=value ...]"
            );
        };
        let secs: u64 = match rest.first() {
            Some(s) => s.parse().context("seconds")?,
            None => 20,
        };
        let vars = Vars(
            vars.iter()
                .filter_map(|v| v.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
        let work = PathBuf::from(work);
        for d in ["lib", "out", "root/sandbox", "sandbox"] {
            std::fs::create_dir_all(work.join(d))?;
        }
        let work = std::path::absolute(&work)?;
        let lib = work.join("lib").join("host.so");
        std::fs::copy(so, &lib).with_context(|| format!("copying {so}"))?;
        let out = out_dir(&work)?;
        println!("out={}", out.display());

        let mut opts = Options::new(&lib);
        opts.log = Some(out.join("log.txt"));
        opts.trace = vars
            .get("TRACE")
            .map_or(Ok(1), str::parse)
            .context("TRACE")?;
        if cfg!(windows) {
            opts.root = Some(work.join("root"));
        } else {
            let home = work.join("sandbox");
            opts.env[0].1 = home.to_string_lossy().into_owned();
        }
        let rec = Arc::new(Recorder {
            turns: Mutex::new(File::create(out.join("turns.bin"))?),
            capture: Mutex::new((0, capture::Writer::create(out.join("capture.bin"))?)),
            emits: AtomicU64::new(0),
            storage: Mutex::new(HashMap::new()),
            no_reply: vars.get("NO_REPLY").is_some(),
            no_batch: vars.get("NO_BATCH").is_some(),
        });
        let watched = rec.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(secs));
            log(&format!(
                "[timeout] emits={}",
                watched.emits.load(Ordering::SeqCst)
            ));
            phoenix_guest::log_import_stats();
            phoenix_guest::exit(3);
        });

        let library = phoenix_guest::load(&opts)?;
        log("[main] init done");
        if let Some(e) = library.engines() {
            log(&format!("[main] ldh_engines -> {e}"));
        }
        let emits = || rec.emits.load(Ordering::SeqCst);
        let guest = match library.create(rec.clone()) {
            Ok(g) => g,
            Err(e) => {
                log(&format!("[main] {e:#}"));
                finish(emits());
            }
        };
        log(&format!("[main] ldh_create_linked -> ok emits={}", emits()));
        let r = guest.start();
        log(&format!(
            "[main] ldh_start -> {} emits={}",
            status(&r),
            emits()
        ));

        script(&guest, vars.get("PRESS"), &emits)?;
        std::thread::sleep(Duration::from_secs(2));
        finish(emits())
    }

    /// The UI events from the wire spec: the window, insets and frame
    /// interval, ten frames 50 ms apart, and a press on `press` followed by
    /// twenty more.
    fn script(guest: &Guest, press: Option<&str>, emits: &dyn Fn() -> u64) -> Result<()> {
        let send = |what: &str, body: &[u8]| {
            let mut buf = vec![0u8; 4];
            buf.extend_from_slice(body);
            log(&format!("[event] {what}: {}", hex(&buf, 64)));
            let r = guest.dispatch(&buf);
            log(&format!(
                "[event] {what} -> {} emits={}",
                status(&r),
                emits()
            ));
        };
        let floats = |head: [u8; 2], v: &[f32]| {
            let mut b = head.to_vec();
            for f in v {
                b.extend(f.to_le_bytes());
            }
            b
        };
        // window: f32 width, f32 height, f32 scale (op 5)
        send("window", &floats([1, 5], &[411.0, 914.0, 2.625]));
        send("insets", &floats([1, 4], &[0.0, 24.0, 0.0, 48.0, 0.0]));
        let mut fi = vec![1, 6];
        fi.extend((1.0f64 / 60.0).to_le_bytes());
        send("frameInterval", &fi);
        let frames = |n: usize| {
            for _ in 0..n {
                send("frame", &[1, 7]);
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        frames(10);
        if let Some(node) = press {
            let node: i32 = node.parse().context("PRESS")?;
            let mut pe = vec![1, 1];
            pe.extend(node.to_le_bytes());
            pe.extend(614u16.to_le_bytes());
            pe.push(0);
            send("press", &pe);
            frames(20);
        }
        Ok(())
    }

    fn status(r: &Result<()>) -> String {
        match r {
            Ok(()) => "ok".into(),
            Err(e) => format!("err={e:#}"),
        }
    }

    fn finish(emits: u64) -> ! {
        log(&format!("[main] done, emits={emits}"));
        phoenix_guest::log_import_stats();
        phoenix_guest::exit(0)
    }
}
