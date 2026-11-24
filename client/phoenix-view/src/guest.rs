//! The guest's side of the platform: the loader (`phoenix-guest`), the
//! thread the guest runs on, and the answers to its host calls.
//!
//! Every call into the library is made on one thread, the guest thread. The
//! guest emits from inside those calls (and once from a thread of its own),
//! through the platform's [`Transport`]. A render turn goes to the UI
//! thread over a channel. A host CALL is answered before `emit` returns,
//! through the reply slot, as `ldh_reply` expects. The UI thread sends UI events back
//! over a second channel, and the guest thread dispatches them, along with
//! the frames the guest asks for, the timers it sets and, with `--live`, the
//! `@http` and `@ws` events of its network calls (see [`crate::net`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use futures::channel::mpsc::UnboundedSender;
use phoenix_guest::{Reply, Transport};

use crate::net::Net;
use crate::{device, store};
use rekindle_wire::frame::{self, Frame};
use rekindle_wire::{NodeOp, PropId, Turn, Value};

/// What the guest thread hands the UI thread.
pub enum FromGuest {
    /// A render turn, from its version byte.
    Turn(Vec<u8>),
    /// The palette the guest sent with `AppState.setTheme`.
    Theme(Value),
    /// The guest thread stopped, with the reason.
    Stopped(String),
}

/// What the UI thread hands the guest thread.
pub enum ToGuest {
    /// A UI event or host frame, as `ldh_dispatch` takes it.
    Dispatch(Vec<u8>),
}

/// Host method ids, from the id table of build 356207. Module and method ids
/// share one numbering.
mod method {
    pub const APP_STATE_SET_THEME: u16 = 84;
    pub const DEVICE_INFO_CONSTANTS: u16 = 114;
    pub const STORAGE_GET: u16 = 214;
    pub const STORAGE_REMOVE: u16 = 216;
    pub const STORAGE_SET: u16 = 217;
    pub const TIMING_CLEAR: u16 = 234;
    pub const TIMING_SET: u16 = 235;
    pub const UI_MANAGER_BATCH_EVENTS: u16 = 238;
    pub const TEXT_METRICS_TEXT_WIDTH: u16 = 466;
    /// Called with `{url}` right after a `linkPress` on a web link. Its
    /// name isn't in phoenix-view's table.
    pub const OPEN_URL: u16 = 150;

    pub fn name(id: u16) -> Option<&'static str> {
        Some(match id {
            78 => "AppState.exit",
            81 => "AppState.setBackHandler",
            83 => "AppState.setReduceMotion",
            APP_STATE_SET_THEME => "AppState.setTheme",
            DEVICE_INFO_CONSTANTS => "DeviceInfo.constants",
            120 => "Diagnostics.setFpsMonitor",
            121 => "Diagnostics.setFrameTiming",
            135 => "Haptics.pattern",
            136 => "Haptics.perform",
            146 => "KeyCommands.set",
            147 => "Keyboard.dismiss",
            167 => "Networking.cancel",
            168 => "Networking.request",
            169 => "Networking.upload",
            289 => "WebSocket.close",
            290 => "WebSocket.dropEarly",
            291 => "WebSocket.open",
            292 => "WebSocket.sendBinary",
            293 => "WebSocket.sendText",
            440 => "WebSocket.adopt",
            213 => "Storage.clearProduction",
            STORAGE_GET => "Storage.get",
            215 => "Storage.production",
            STORAGE_REMOVE => "Storage.remove",
            STORAGE_SET => "Storage.set",
            233 => "TextMetrics.emojiNickNudge",
            177 => "Perf.lastExit",
            178 => "Perf.mark",
            179 => "Perf.marks",
            181 => "Perf.processStats",
            TIMING_CLEAR => "Timing.clear",
            TIMING_SET => "Timing.set",
            UI_MANAGER_BATCH_EVENTS => "UIManager.batchEvents",
            246 => "Voice.capabilities",
            TEXT_METRICS_TEXT_WIDTH => "TextMetrics.textWidth",
            _ => return None,
        })
    }
}

/// Record keys, from the same id table.
mod atom {
    use rekindle_wire::PropId;
    pub const VERSION: PropId = PropId(390);
    pub const KEY: PropId = PropId(405);
    /// The URL in method 150's record.
    pub const URL: PropId = PropId(422);
    pub const FOUND: PropId = PropId(406);
    pub const ID: PropId = PropId(417);
    pub const SECURE: PropId = PropId(437);
    pub const DELAY: PropId = PropId(438);
    pub const TEXT: PropId = PropId(472);
    pub const SIZE: PropId = PropId(481);
    pub const FRAME_MS: PropId = PropId(484);
    pub const WIDTH: PropId = PropId(493);
    pub const WEIGHT: PropId = PropId(1180);
}

/// `@timer`, the host event a timer set with `Timing.set` fires.
const TIMER_EVENT: u16 = 368;
/// A display frame at 60 Hz, when the display's own rate isn't known.
pub const FRAME: Duration = Duration::from_micros(16_667);

/// A line in the loader's log, beside its own lines.
pub fn log(line: &str) {
    phoenix_guest::log(line);
}

/// Where the guest's files are, and how to load it.
pub struct Config {
    pub so: PathBuf,
    /// Holds `root\` (the guest's `/`, with `HOME=/sandbox`) and `guest.log`.
    pub work: PathBuf,
    pub trace: i32,
    /// Answer `DeviceInfo.constants`.
    pub device_info: bool,
    /// `--live`: reach Discord, and keep `Storage` in this DPAPI-sealed file.
    pub live: Option<PathBuf>,
    /// Names added to `DeviceInfo.constants`' `capabilities`.
    pub capabilities: Vec<String>,
    /// Open a link the guest asks to open after the user pressed it. Off
    /// in a self-test or snapshot run, which only log it.
    pub open_links: bool,
    /// The time between the display's refreshes, which paces the guest's
    /// frames and answers `frameMs`.
    pub frame: Duration,
}

/// Measures text for `TextMetrics.textWidth`, in dp.
pub type Measure = Box<dyn Fn(&str, f32, u16) -> f32 + Send + Sync>;

/// The platform state the guest's calls read and write, shared by `emit`
/// on any thread and the guest thread's loop.
struct Platform {
    out: UnboundedSender<FromGuest>,
    wake: mpsc::Sender<Wake>,
    state: Mutex<State>,
    frame_requested: AtomicBool,
    /// The time between frames, in microseconds.
    frame_us: AtomicU64,
    device_info: bool,
    measure: Measure,
    calls: AtomicU64,
    /// The window's size in dp and its density.
    window: Mutex<(f32, f32, f32)>,
    /// `--live`: the network modules and the sealed storage file.
    net: Option<Arc<Net>>,
    sealed: Option<store::Sealed>,
    vendor_id: String,
    /// `--capability` names, for `capabilities`.
    capabilities: Vec<String>,
    open_links: bool,
    /// When the user last pressed a link span.
    link_pressed: Mutex<Option<Instant>>,
}

#[derive(Default)]
struct State {
    /// `Storage`: in memory, and with `--live` sealed to disk on each change.
    storage: HashMap<String, Vec<u8>>,
    /// `Timing`: id to deadline.
    timers: HashMap<i64, Instant>,
}

enum Wake {
    Ui(ToGuest),
    /// An EVENT frame from a host module's own thread.
    Host(Vec<u8>),
    Timers,
}

/// The UI thread's end of the guest thread.
#[derive(Clone)]
pub struct Guest {
    tx: mpsc::Sender<Wake>,
    platform: Arc<Platform>,
}

impl Guest {
    /// Send a UI event or host frame to the guest.
    pub fn dispatch(&self, bytes: Vec<u8>) {
        let _ = self.tx.send(Wake::Ui(ToGuest::Dispatch(bytes)));
    }

    /// The window's size in dp and its density, for `DeviceInfo.constants`.
    pub fn set_window(&self, width: f32, height: f32, density: f32) {
        *self.platform.window.lock().unwrap() = (width, height, density);
    }

    /// The time between the display's refreshes, as the guest's frames
    /// follow them.
    pub fn set_frame_interval(&self, frame: Duration) {
        self.platform
            .frame_us
            .store(frame.as_micros() as u64, Ordering::Relaxed);
    }

    /// How many host calls the guest has made.
    pub fn calls(&self) -> u64 {
        self.platform.calls.load(Ordering::Relaxed)
    }

    /// The user pressed a link span. A URL the guest asks to open in the
    /// next three seconds is the one pressed.
    pub fn user_link_press(&self) {
        *self.platform.link_pressed.lock().unwrap() = Some(Instant::now());
    }

    /// The network modules, with `--live`.
    pub fn net(&self) -> Option<Arc<Net>> {
        self.platform.net.clone()
    }
}

/// Start the guest thread: load the library, create and start the guest.
/// Its turns arrive on `out`.
pub fn spawn(
    cfg: Config,
    out: UnboundedSender<FromGuest>,
    measure: Measure,
    window: (f32, f32, f32),
) -> Result<Guest> {
    if !cfg.so.is_file() {
        bail!("the host library not found at {}", cfg.so.display());
    }
    let root = cfg.work.join("root");
    std::fs::create_dir_all(root.join("sandbox"))?;
    let (tx, rx) = mpsc::channel();
    let (net, sealed, storage, vendor_id) = match &cfg.live {
        Some(path) => {
            let (sealed, storage) = store::Sealed::open(path)?;
            log(&format!(
                "[view] live: Storage sealed with DPAPI in {} ({} keys)",
                path.display(),
                storage.len()
            ));
            let to_guest = tx.clone();
            let net = Net::new(Box::new(move |frame| {
                let _ = to_guest.send(Wake::Host(frame));
            }))?;
            let vendor = path.with_file_name("vendor_id");
            let vendor_id = match std::fs::read_to_string(&vendor) {
                Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
                _ => {
                    // The first live run: the storage's folder may not
                    // exist yet.
                    if let Some(dir) = vendor.parent() {
                        std::fs::create_dir_all(dir)
                            .with_context(|| format!("creating {}", dir.display()))?;
                    }
                    let v = device::random_uuid();
                    std::fs::write(&vendor, &v)
                        .with_context(|| format!("writing {}", vendor.display()))?;
                    v
                }
            };
            (Some(net), Some(sealed), storage, vendor_id)
        }
        None => (None, None, HashMap::new(), device::random_uuid()),
    };
    let platform = Arc::new(Platform {
        out,
        wake: tx.clone(),
        state: Mutex::new(State {
            storage,
            ..State::default()
        }),
        frame_requested: AtomicBool::new(false),
        frame_us: AtomicU64::new(cfg.frame.as_micros() as u64),
        device_info: cfg.device_info,
        capabilities: cfg.capabilities.clone(),
        open_links: cfg.open_links,
        link_pressed: Mutex::new(None),
        measure,
        calls: AtomicU64::new(0),
        window: Mutex::new(window),
        net,
        sealed,
        vendor_id,
    });
    let p = platform.clone();
    std::thread::Builder::new()
        .name("guest".into())
        .spawn(move || {
            let why = match run(&cfg, &p, &rx) {
                Ok(()) => "the UI closed".to_string(),
                Err(e) => format!("{e:#}"),
            };
            log(&format!("[view] guest thread stopped: {why}"));
            let _ = p.out.unbounded_send(FromGuest::Stopped(why));
        })?;
    Ok(Guest { tx, platform })
}

fn run(cfg: &Config, platform: &Arc<Platform>, rx: &mpsc::Receiver<Wake>) -> Result<()> {
    let mut opts = phoenix_guest::Options::new(&cfg.so);
    opts.log = Some(cfg.work.join("guest.log"));
    opts.root = Some(cfg.work.join("root"));
    opts.trace = cfg.trace;
    opts.faults = phoenix_guest::Faults::GuestOnly;
    // A live login with an account outside Phoenix's rollout otherwise stops
    // at the client's own "not available for your account" gate. Neutralize
    // it for live sessions; `PHOENIX_KEEP_GATE` leaves it in place.
    opts.patch_access_gate = cfg.live.is_some() && std::env::var_os("PHOENIX_KEEP_GATE").is_none();
    let library = phoenix_guest::load(&opts).with_context(|| {
        format!(
            "loading the guest: see {}",
            cfg.work.join("guest.log").display()
        )
    })?;
    let guest = library.create(Arc::new(PlatformTransport(platform.clone())))?;
    guest.start()?;
    log("[view] guest started");
    let dispatch = |what: &str, bytes: &[u8]| {
        if let Err(e) = guest.dispatch(bytes) {
            log(&format!(
                "[view] dispatch {what} ({} bytes) failed: {e:#}",
                bytes.len()
            ));
        }
    };
    let mut next_frame = Instant::now();
    loop {
        let now = Instant::now();
        let due: Vec<i64> = {
            let mut st = platform.state.lock().unwrap();
            let due: Vec<i64> = st
                .timers
                .iter()
                .filter(|(_, t)| **t <= now)
                .map(|(id, _)| *id)
                .collect();
            for id in &due {
                st.timers.remove(id);
            }
            due
        };
        for id in due {
            let ev = Frame::event(TIMER_EVENT, &Value::Object(vec![(atom::ID, int(id))]));
            log(&format!("[view] timer {id} fires"));
            dispatch("@timer", &ev.to_bytes());
        }
        if platform.frame_requested.load(Ordering::Acquire) && now >= next_frame {
            platform.frame_requested.store(false, Ordering::Release);
            next_frame = now + Duration::from_micros(platform.frame_us.load(Ordering::Relaxed));
            dispatch("frame", &rekindle_wire::event::UiEvent::Frame.to_bytes());
            continue;
        }
        let mut wait = Duration::from_secs(1);
        if platform.frame_requested.load(Ordering::Acquire) {
            wait = wait.min(next_frame.saturating_duration_since(now));
        }
        if let Some(t) = platform.state.lock().unwrap().timers.values().min() {
            wait = wait.min(t.saturating_duration_since(now));
        }
        match rx.recv_timeout(wait) {
            Ok(Wake::Ui(ToGuest::Dispatch(bytes))) => dispatch("event", &bytes),
            Ok(Wake::Host(bytes)) => dispatch("host event", &bytes),
            Ok(Wake::Timers) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn int(v: i64) -> Value {
    i32::try_from(v).map_or(Value::I64(v), Value::I32)
}

/// The platform as the guest's transport. `emit` runs on the thread the
/// guest emits from, on its guest stack.
struct PlatformTransport(Arc<Platform>);

impl Transport for PlatformTransport {
    fn emit(&self, bytes: &[u8], reply: Reply<'_>) -> bool {
        self.0.on_emit(bytes, &reply);
        true
    }

    /// The guest calls `notify` after some host calls; this platform wakes
    /// its loop, which has nothing else to do for it.
    fn notify(&self) {
        let _ = self.0.wake.send(Wake::Timers);
    }
}

impl Platform {
    fn on_emit(&self, bytes: &[u8], slot: &Reply<'_>) {
        if rekindle_wire::event::is_ui(bytes) {
            let body = &bytes[4..];
            if let Ok(turn) = Turn::from_bytes(body)
                && turn.ops.iter().any(|op| matches!(op, NodeOp::RequestFrame))
            {
                self.frame_requested.store(true, Ordering::Release);
                let _ = self.wake.send(Wake::Timers);
            }
            let _ = self.out.unbounded_send(FromGuest::Turn(body.to_vec()));
            return;
        }
        let Ok(call) = Frame::from_bytes(bytes) else {
            log(&format!(
                "[view] emit: {} bytes that are neither a turn nor a frame",
                bytes.len()
            ));
            return;
        };
        if call.kind != frame::CALL {
            log(&format!("[view] emit: a frame of kind {}", call.kind));
            return;
        }
        self.calls.fetch_add(1, Ordering::Relaxed);
        let record = call.record_value().unwrap_or(Value::Null);
        let name = method::name(call.method)
            .map_or_else(|| format!("method {}", call.method), str::to_string);
        let (reply, note) = self.answer(&call, &record);
        log(&format!("[call] #{} {name}: {note}", call.call));
        if let Some((rec, body)) = reply {
            let frame = Frame {
                kind: frame::REPLY,
                record: rec.to_bytes(),
                body,
                ..call
            }
            .to_bytes();
            slot.send(&frame);
        }
    }

    /// The reply to one host call, if this platform answers it, and a note
    /// for the log.
    fn answer(&self, call: &Frame, record: &Value) -> (Option<(Value, Vec<u8>)>, String) {
        let field = |k: PropId| match record {
            Value::Object(fields) => fields.iter().find(|(p, _)| *p == k).map(|(_, v)| v),
            _ => None,
        };
        let text = |k: PropId| match field(k) {
            Some(Value::Str(s)) => Some(s.as_str()),
            _ => None,
        };
        let number = |k: PropId| match field(k) {
            Some(Value::F64(f)) => Some(*f),
            Some(v) => v.as_int().map(|i| i as f64),
            None => None,
        };
        match call.method {
            method::UI_MANAGER_BATCH_EVENTS => {
                let ms = self.frame_us.load(Ordering::Relaxed) as f64 / 1000.0;
                (
                    Some((
                        obj(vec![
                            (atom::VERSION, Value::I32(1)),
                            (atom::FRAME_MS, Value::F64(ms)),
                        ]),
                        vec![],
                    )),
                    format!("{{version: 1, frameMs: {ms:.2}}}"),
                )
            }
            method::STORAGE_GET => {
                let key = text(atom::KEY).unwrap_or_default().to_string();
                let secure = field(atom::SECURE) == Some(&Value::Bool(true));
                let tag = if secure { " (secure)" } else { "" };
                if secure && self.sealed.is_none() {
                    return (
                        Some((obj(vec![(atom::FOUND, Value::Bool(false))]), vec![])),
                        format!("{key:?}{tag}: not found, kept only with --live"),
                    );
                }
                match self.state.lock().unwrap().storage.get(&key) {
                    Some(v) => (
                        Some((obj(vec![(atom::FOUND, Value::Bool(true))]), v.clone())),
                        format!("{key:?}{tag}: {} bytes", v.len()),
                    ),
                    None => (
                        Some((obj(vec![(atom::FOUND, Value::Bool(false))]), vec![])),
                        format!("{key:?}{tag}: not found"),
                    ),
                }
            }
            method::STORAGE_SET => {
                let key = text(atom::KEY).unwrap_or_default().to_string();
                let secure = field(atom::SECURE) == Some(&Value::Bool(true));
                let tag = if secure { " (secure)" } else { "" };
                if secure && self.sealed.is_none() {
                    return (
                        None,
                        format!("{key:?}{tag}: not stored, kept only with --live"),
                    );
                }
                let note = format!("{key:?}{tag} = {} bytes", call.body.len());
                let mut st = self.state.lock().unwrap();
                st.storage.insert(key, call.body.clone());
                (None, format!("{note}{}", self.seal(&st.storage)))
            }
            method::STORAGE_REMOVE => {
                let key = text(atom::KEY).unwrap_or_default().to_string();
                let mut st = self.state.lock().unwrap();
                st.storage.remove(&key);
                (None, format!("{key:?} removed{}", self.seal(&st.storage)))
            }
            method::TIMING_SET => {
                let id = field(atom::ID).and_then(Value::as_int).unwrap_or(-1);
                let delay = number(atom::DELAY).unwrap_or(0.0).max(0.0);
                let at = Instant::now() + Duration::from_secs_f64(delay / 1000.0);
                self.state.lock().unwrap().timers.insert(id, at);
                let _ = self.wake.send(Wake::Timers);
                (None, format!("timer {id} in {delay} ms"))
            }
            method::TIMING_CLEAR => {
                let id = field(atom::ID).and_then(Value::as_int).unwrap_or(-1);
                self.state.lock().unwrap().timers.remove(&id);
                (None, format!("timer {id} cleared"))
            }
            method::TEXT_METRICS_TEXT_WIDTH => {
                let s = text(atom::TEXT).unwrap_or_default();
                let size = number(atom::SIZE).unwrap_or(16.0) as f32;
                let weight = number(atom::WEIGHT).unwrap_or(400.0) as u16;
                let w = (self.measure)(s, size, weight);
                (
                    Some((obj(vec![(atom::WIDTH, Value::F64(f64::from(w)))]), vec![])),
                    format!(
                        "{} chars at {size}, weight {weight} -> {w:.2} dp",
                        s.chars().count()
                    ),
                )
            }
            method::DEVICE_INFO_CONSTANTS if self.device_info => {
                (Some((self.device_constants(), vec![])), "answered".into())
            }
            method::APP_STATE_SET_THEME => {
                let _ = self.out.unbounded_send(FromGuest::Theme(record.clone()));
                (None, "palette passed to the view".into())
            }
            167..=169 | 289..=293 | 440 => {
                if let Some(net) = &self.net {
                    (None, net.call(call.method, record, &call.body))
                } else {
                    let host = text(PropId(422))
                        .and_then(|u| u.split('/').nth(2))
                        .unwrap_or("?");
                    (None, format!("REFUSED, no network without --live: {host}"))
                }
            }
            method::OPEN_URL => match self.link_url(record) {
                Some(url) => (None, self.open_link(&url)),
                None => (None, "not the user's link press; not opened".into()),
            },
            _ => (
                None,
                if matches!(record, Value::Null) {
                    "no reply".into()
                } else if self.net.is_some() {
                    // A live run's records can carry what the user typed or
                    // the account's data.
                    "no reply (record not logged in a live run)".into()
                } else {
                    format!("no reply ({})", short(record))
                },
            ),
        }
    }

    /// The `http` or `https` URL in a call's `url` field, if the user
    /// pressed a link span in the last three seconds. Nothing else is
    /// ever opened.
    fn link_url(&self, record: &Value) -> Option<String> {
        let pressed = (*self.link_pressed.lock().unwrap())?;
        if pressed.elapsed() > Duration::from_secs(3) {
            return None;
        }
        let Value::Object(fields) = record else {
            return None;
        };
        let url = fields.iter().find_map(|(p, v)| match v {
            Value::Str(s) if *p == atom::URL => Some(s.clone()),
            _ => None,
        })?;
        let lower = url.to_ascii_lowercase();
        (lower.starts_with("https://") || lower.starts_with("http://")).then_some(url)
    }

    /// Open a link the user pressed with the system's handler for it,
    /// once: the press is used up.
    fn open_link(&self, url: &str) -> String {
        *self.link_pressed.lock().unwrap() = None;
        let host = url.split('/').nth(2).unwrap_or("?").to_string();
        if !self.open_links {
            return format!("the user's link to {host}; not opened in a test run");
        }
        #[cfg(windows)]
        let opened = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()
            .is_ok();
        #[cfg(not(windows))]
        let opened = false;
        format!("the user's link to {host}; opened: {opened}")
    }

    /// `DeviceInfo.constants`, with the keys and fixed values the Android
    /// platform's `DeviceInfo` module (`b3`) writes. What Java reads off the
    /// phone (`Build.MODEL` and the rest) is left out, as Java leaves out a
    /// null; what it reads off the system comes from this one.
    fn device_constants(&self) -> Value {
        let (w, h, density) = *self.window.lock().unwrap();
        let s = |v: &str| Value::Str(v.into());
        let key = PropId;
        let locale = device::locale();
        let mut fields = vec![
            (key(638), s("production")),
            (key(640), s("android")),
            (key(641), s("native")),
            (key(642), s("internal")),
            (key(649), Value::F64(f64::from(w.min(h)))),
            (key(650), Value::I32(0)),
            (key(652), s(&device::ram_gb())),
            (key(653), s("")),
            (key(654), s(device::OS_VERSION)),
            (key(1418), Value::I32(device::API_LEVEL)),
            (
                key(656),
                obj(vec![
                    (key(633), Value::Bool(false)),
                    (key(634), Value::Bool(false)),
                    (key(635), Value::Bool(false)),
                    (key(636), Value::Bool(true)),
                ]),
            ),
            (key(657), s("0.1.0")),
            (key(659), Value::I64(1)),
            (key(660), s("internalRelease")),
            (key(661), Value::I64(1)),
            (key(665), s(&self.vendor_id)),
            (key(669), Value::I32(device::utc_offset_secs())),
            (key(670), s(&locale)),
            (key(671), Value::F64(f64::from(density))),
            (key(672), Value::Bool(false)),
            (key(583), Value::Bool(true)),
            (key(674), Value::F64(f64::from(w))),
            (key(675), Value::F64(f64::from(h))),
            (key(676), Value::F64(1.0)),
            (key(677), Value::Bool(false)),
            (key(678), Value::Bool(false)),
            (key(679), Value::Bool(false)),
            (key(2425), Value::Array(vec![s(&locale)])),
        ];
        // `capabilities`: with `--live`, the gateway's zstd-stream is
        // inflated here, so the guest may ask for it.
        // Android lists every module it has (`en.f6941b`); this lists the
        // ones it plays, and what `--capability` adds.
        let mut caps: Vec<Value> = self.capabilities.iter().map(|c| s(c)).collect();
        if self.net.is_some() {
            caps.push(s("ZstdInflate"));
        }
        if !caps.is_empty() {
            fields.push((key(565), Value::Array(caps)));
        }
        obj(fields)
    }

    /// Seal the storage to disk with `--live`: a note for the call log.
    fn seal(&self, storage: &HashMap<String, Vec<u8>>) -> String {
        match &self.sealed {
            Some(sealed) => match sealed.save(storage) {
                Ok(()) => ", sealed".into(),
                Err(e) => format!(", NOT sealed: {e:#}"),
            },
            None => String::new(),
        }
    }
}

fn obj(fields: Vec<(PropId, Value)>) -> Value {
    Value::Object(fields)
}

fn short(v: &Value) -> String {
    let s = format!("{v:?}");
    if s.len() > 160 {
        format!("{}...", &s[..s.floor_char_boundary(160)])
    } else {
        s
    }
}
