//! `phoenix-view`: Phoenix's real guest, live in a GPUI window.
//!
//! The guest is the one compiled into the Android `x86_64` host library
//! (`liblibdiscord_wasm_host.so`, build 356207), loaded natively by the
//! `phoenix-guest` crate (`crates/phoenix-guest`). This binary is the platform the Android build's Java `UIManager` is: it
//! applies the guest's turns, paints them, answers its host calls and sends
//! it the window's input. A plain run is live: it reaches the network, and
//! then only Discord's hosts. `--offline` holds it back.

mod apk;
mod axml;
mod capture;
mod device;
mod drawable;
mod export;
mod guest;
mod input;
mod motion;
mod navanim;
mod net;
mod remote;
mod res;
mod scroller;
mod scrollport;
mod selftest;
mod store;
mod theme;
mod view;

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, AppContext as _, Bounds, Font, FontWeight, Pixels, Size, WindowBounds, WindowOptions, px,
};

const HELP: &str = "\
phoenix-view: Phoenix's real guest in a GPUI window

usage: phoenix-view [options]

  --so <path>        the x86_64 host library (default
                     H:\\phoenix-apk\\x86_64\\clean\\liblibdiscord_wasm_host.so,
                     or PHOENIX_SO)
  --apk <path>       the base APK, for fonts and drawables (default
                     H:\\phoenix-apk\\discord-356207-base.apk, or PHOENIX_APK);
                     the config splits beside it are read too
  --split <path>     a config split kept elsewhere, such as the device's
                     split_config.xxhdpi.apk; may repeat
  --work <dir>       the guest's root and log (default %TEMP%\\phoenix-view)
  --theme <file>     a rekindle .theme.json, run through rekindle's wire engine
  --variant <name>   the theme's variant
  --work-json <file> a wire-inject Work as JSON, instead of a theme
  --no-device-info   leave DeviceInfo.constants unanswered
  --trace <n>        the loader's import logging (0, 1 or 2; default 0)
  --live             let the guest reach Discord (its hosts only), and keep
                     its Storage sealed with DPAPI on disk. The default,
                     except under --selftest or --snapshot
  --offline          keep the guest off the network, whatever else is set
  --storage <file>   the sealed Storage file when live (default
                     %LOCALAPPDATA%\\phoenix-view\\storage.dpapi)
  --snapshot <secs>  show the window off the desktop, then after <secs>
                     save snapshot.png and snapshot-tree.txt to the work dir
                     and quit; with --live, in the real Storage, signed in
  --click <label>    with --snapshot: then click the node whose
                     accessibilityLabel or accessibilityId is <label> (or
                     starts with it, if <label> ends in *), through the
                     window's own event dispatch, and save what follows as
                     snapshot-<n>.png; may repeat. scroll:<dp>,
                     wheel:<notches> and fling:<dp> scroll over the centre
                     (or at @<x>,<y> after them, in dp) instead, positive
                     toward older rows, and link:<text> clicks a link
  --frames <ms,...>  with --click: also save snapshot-<n>-<ms>.png that many
                     milliseconds after each click, to see motion
  --size <w>x<h>     the window's size in dp (default 412x860, a phone's)
  --capability <c>   add <c> to DeviceInfo.constants' capabilities, as an
                     Android module would; may repeat
  --no-motion        leave the guest's motion unrun, so nothing animates
  --capture <file>   append each turn the guest sends to <file>, in the
                     wire capture format
  --export-assets <dir>
                     write the drawables the --from captures name, and the
                     APK's fonts, into <dir> for the theme playground, and quit
  --from <capture>   with --export-assets: a capture to read; may repeat
  --day              with --export-assets: the drawables' day configuration
  --selftest         drive the window off the desktop and check it; with
                     --live, also a refused host and a login with a fake
                     address, in a scratch Storage file
";

#[allow(clippy::struct_excessive_bools, reason = "one per switch")]
pub struct Args {
    so: PathBuf,
    apk: PathBuf,
    splits: Vec<PathBuf>,
    work: PathBuf,
    theme: Option<PathBuf>,
    variant: Option<String>,
    work_json: Option<PathBuf>,
    device_info: bool,
    trace: i32,
    live: bool,
    motion: bool,
    storage: Option<PathBuf>,
    selftest: bool,
    snapshot: Option<u64>,
    clicks: Vec<String>,
    frames: Vec<u64>,
    capabilities: Vec<String>,
    size: Size<Pixels>,
    capture: Option<PathBuf>,
    export: Option<PathBuf>,
    export_from: Vec<PathBuf>,
    day: bool,
}

fn args() -> Result<Args, String> {
    let env =
        |k: &str, d: &str| std::env::var_os(k).map_or_else(|| PathBuf::from(d), PathBuf::from);
    let mut a = Args {
        so: env(
            "PHOENIX_SO",
            r"H:\phoenix-apk\x86_64\clean\liblibdiscord_wasm_host.so",
        ),
        apk: env("PHOENIX_APK", r"H:\phoenix-apk\discord-356207-base.apk"),
        splits: vec![],
        work: std::env::temp_dir().join("phoenix-view"),
        theme: None,
        variant: None,
        work_json: None,
        device_info: true,
        trace: 0,
        live: false,
        motion: true,
        storage: None,
        selftest: false,
        snapshot: None,
        clicks: vec![],
        frames: vec![],
        capabilities: vec![],
        size: PHONE,
        capture: None,
        export: None,
        export_from: vec![],
        day: false,
    };
    // Live unless told otherwise: a plain run reaches Discord. `--selftest`
    // and `--snapshot` stay offline by default, since that is their contract.
    let mut live_flag: Option<bool> = None;
    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        let arg = arg.to_string_lossy().into_owned();
        let mut val = || {
            it.next()
                .map(PathBuf::from)
                .ok_or(format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "--so" => a.so = val()?,
            "--apk" => a.apk = val()?,
            "--split" => a.splits.push(val()?),
            "--work" => a.work = val()?,
            "--theme" => a.theme = Some(val()?),
            "--variant" => a.variant = Some(val()?.to_string_lossy().into_owned()),
            "--work-json" => a.work_json = Some(val()?),
            "--trace" => {
                a.trace = val()?
                    .to_string_lossy()
                    .parse()
                    .map_err(|_| "--trace takes a number")?;
            }
            "--no-device-info" => a.device_info = false,
            "--live" => live_flag = Some(true),
            "--offline" => live_flag = Some(false),
            "--no-motion" => a.motion = false,
            "--capture" => a.capture = Some(val()?),
            "--export-assets" => a.export = Some(val()?),
            "--from" => a.export_from.push(val()?),
            "--day" => a.day = true,
            "--storage" => a.storage = Some(val()?),
            "--selftest" => a.selftest = true,
            "--snapshot" => {
                a.snapshot = Some(
                    val()?
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| "--snapshot takes seconds")?,
                );
            }
            "--click" => a.clicks.push(val()?.to_string_lossy().into_owned()),
            "--frames" => {
                a.frames = val()?
                    .to_string_lossy()
                    .split(',')
                    .map(|v| v.trim().parse::<u64>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| "--frames takes milliseconds, separated by commas")?;
                a.frames.sort_unstable();
            }
            "--capability" => a.capabilities.push(val()?.to_string_lossy().into_owned()),
            "--size" => {
                let v = val()?.to_string_lossy().into_owned();
                let (w, h) = v
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse::<f32>().ok()?, h.parse::<f32>().ok()?)))
                    .filter(|(w, h)| *w >= 100. && *h >= 100.)
                    .ok_or("--size takes <width>x<height> in dp, each at least 100")?;
                a.size = Size {
                    width: px(w),
                    height: px(h),
                };
            }
            other => return Err(format!("unknown argument {other}\n\n{HELP}")),
        }
    }
    a.live = live_flag.unwrap_or(!(a.selftest || a.snapshot.is_some()));
    Ok(a)
}

/// The window, in dp: a phone's.
pub const PHONE: Size<Pixels> = Size {
    width: px(412.),
    height: px(860.),
};

fn main() {
    let args = match args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("phoenix-view: {e}");
            std::process::exit(2);
        }
    };
    if let Some(dir) = &args.export {
        let from: Vec<&std::path::Path> = args.export_from.iter().map(PathBuf::as_path).collect();
        let done = apk::Apk::open(&args.apk, &args.splits)
            .and_then(|apk| export::run(&apk, dir, &from, !args.day));
        if let Err(e) = done {
            eprintln!("phoenix-view: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    let theme = match (&args.theme, &args.work_json) {
        (Some(t), _) => Some(theme::ThemeSource::theme_file(t, args.variant.as_deref())),
        (None, Some(w)) => Some(theme::ThemeSource::work_file(w)),
        _ => None,
    }
    .transpose()
    .unwrap_or_else(|e| {
        eprintln!("phoenix-view: {e:#}");
        std::process::exit(2);
    });
    gpui_platform::application().run(move |cx: &mut App| {
        input::bind_keys(cx);
        let apk = match apk::Apk::open(&args.apk, &args.splits) {
            Ok(a) => {
                for (p, why) in &a.skipped {
                    eprintln!("phoenix-view: left out {}: {why}", p.display());
                }
                Some(Arc::new(a))
            }
            Err(e) => {
                eprintln!("phoenix-view: no fonts or drawables: {e:#}");
                None
            }
        };
        let fonts = load_fonts(apk.as_deref(), cx);
        let measure = measurer(cx, fonts.sans.clone());
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let live = args.live.then(|| match &args.storage {
            Some(p) => p.clone(),
            // A self-test never touches the owner's sealed file, and starts
            // signed out.
            None if args.selftest => {
                let p = args.work.join("selftest-storage.dpapi");
                let _ = std::fs::remove_file(&p);
                p
            }
            None => store::default_path(),
        });
        if let Some(p) = &live {
            println!(
                "phoenix-view: live; Discord's hosts only; Storage sealed in {}",
                p.display()
            );
        }
        let cfg = guest::Config {
            so: args.so.clone(),
            work: args.work.clone(),
            trace: args.trace,
            device_info: args.device_info,
            live,
            capabilities: args.capabilities.clone(),
            open_links: !args.selftest && args.snapshot.is_none(),
            // The window's own display replaces it once the window is open.
            frame: cx
                .primary_display()
                .and_then(|d| d.refresh_interval())
                .unwrap_or(guest::FRAME),
        };
        println!(
            "phoenix-view: guest log at {}",
            args.work.join("guest.log").display()
        );
        let started = guest::spawn(
            cfg,
            tx,
            measure,
            (args.size.width.into(), args.size.height.into(), 1.0),
        )
        .map(|g| (g, rx));
        if let Err(e) = &started {
            eprintln!("phoenix-view: {e:#}");
        }
        let selftest = args.selftest;
        let snapshot = args.snapshot;
        let clicks = args.clicks.clone();
        let frames = args.frames.clone();
        let hidden = selftest || snapshot.is_some();
        let live = args.live;
        let motion = args.motion;
        let capture = args.capture.clone();
        let work = args.work.clone();
        let size = args.size;
        let bounds = Bounds::centered(None, size, cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("Phoenix".into()),
                    ..Default::default()
                }),
                show: !hidden,
                focus: !hidden,
                // GPUI draws an inactive window at 30 Hz by default. A
                // phone draws at the panel's rate whatever else is on
                // screen, and so does this window.
                inactive_frame_interval: None,
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| {
                    let mut v = view::PhoenixView::new(started, apk, fonts, theme, cx);
                    v.motion.enabled = motion;
                    if let Some(p) = &capture {
                        match rekindle_wire::capture::Writer::create(p) {
                            Ok(w) => v.capture = Some(w),
                            Err(e) => eprintln!("phoenix-view: no capture to {}: {e}", p.display()),
                        }
                    }
                    v
                });
                // A move to another display, or a change of its refresh
                // rate, draws a frame, which sends the guest the new rate.
                let v = view.downgrade();
                window
                    .observe_window_display(move |_, cx| {
                        let _ = v.update(cx, |_, cx| cx.notify());
                    })
                    .detach();
                if selftest {
                    selftest::start(view.clone(), live, work.clone(), window, cx);
                } else if let Some(secs) = snapshot {
                    selftest::snapshot(
                        view.clone(),
                        secs,
                        (clicks, frames),
                        size,
                        work.clone(),
                        window,
                        cx,
                    );
                }
                view
            },
        );
        if let Err(e) = opened {
            eprintln!("phoenix-view: no window: {e:#}");
            cx.quit();
        }
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        if !hidden {
            cx.activate(true);
        }
    });
}

/// Register the APK's fonts and find their families.
fn load_fonts(apk: Option<&apk::Apk>, cx: &mut App) -> view::Fonts {
    let Some(apk) = apk else {
        return view::Fonts::default();
    };
    let fonts: Vec<Cow<'static, [u8]>> = apk.fonts().into_iter().map(Cow::Owned).collect();
    let n = fonts.len();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("phoenix-view: fonts: {e:#}");
        return view::Fonts::default();
    }
    let names = cx.text_system().all_font_names();
    let find =
        |want: &dyn Fn(&str) -> bool| names.iter().find(|n| want(n)).map(|n| n.clone().into());
    let f = view::Fonts {
        sans: find(&|n| n.to_lowercase().replace(' ', "") == "ggsans"),
        ginto: find(&|n| n.contains("Ginto") && n.contains("Discord"))
            .or_else(|| find(&|n| n.contains("Ginto"))),
        mono: find(&|n| n.to_lowercase().replace(' ', "") == "ggmono")
            .or_else(|| find(&|n| n.to_lowercase().contains("mono"))),
    };
    println!(
        "phoenix-view: {n} fonts from the APK; sans {:?}, ginto {:?}, mono {:?}",
        f.sans, f.ginto, f.mono
    );
    f
}

/// `TextMetrics.textWidth`: the sum of the advances in the APK's sans.
fn measurer(cx: &App, family: Option<gpui::SharedString>) -> guest::Measure {
    let ts = cx.text_system().clone();
    Box::new(move |text, size, weight| {
        let font = Font {
            weight: FontWeight(f32::from(weight)),
            ..gpui::font(family.clone().unwrap_or_else(|| ".SystemUIFont".into()))
        };
        let id = ts.resolve_font(&font);
        text.chars()
            .map(|c| {
                ts.advance(id, px(size), c)
                    .map_or(0.0, |a| f32::from(a.width))
            })
            .sum()
    })
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
    fn SetWindowLongPtrW(hwnd: isize, index: i32, value: isize) -> isize;
    fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32)
    -> i32;
    fn GetWindowRect(hwnd: isize, rect: *mut [i32; 4]) -> i32;
    fn GetClientRect(hwnd: isize, rect: *mut [i32; 4]) -> i32;
}
const SWP_NOMOVE: u32 = 0x2;
const SWP_NOZORDER: u32 = 0x4;
const SWP_NOACTIVATE: u32 = 0x10;
const SWP_SHOWWINDOW: u32 = 0x40;

fn hwnd(window: &gpui::Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let h = HasWindowHandle::window_handle(window).ok()?;
    match h.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

/// A `SetWindowPos` to make once no GPUI update holds the window: GPUI
/// reads the `WM_SIZE` it sends in a callback that can't reach a window that
/// is being updated.
pub struct Place(isize, i32, i32, i32, i32, u32);

impl Place {
    pub fn apply(self) {
        let Place(hwnd, x, y, w, h, flags) = self;
        // SAFETY: the window's own handle and plain values.
        unsafe { SetWindowPos(hwnd, 0, x, y, w, h, flags) };
    }
}

/// Resize the window's content to `width` x `height` dp without moving or
/// activating it. GPUI's own resize activates the window.
pub fn resize_quietly(window: &gpui::Window, width: f32, height: f32) -> Option<Place> {
    let hwnd = hwnd(window)?;
    let (old, scale) = (window.viewport_size(), window.scale_factor());
    let mut r = [0i32; 4];
    // SAFETY: the window's own handle and a RECT-sized buffer.
    unsafe { GetWindowRect(hwnd, &raw mut r) };
    let w = r[2] - r[0] + ((width - f32::from(old.width)) * scale).round() as i32;
    let h = r[3] - r[1] + ((height - f32::from(old.height)) * scale).round() as i32;
    Some(Place(
        hwnd,
        0,
        0,
        w,
        h,
        SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
    ))
}

/// Show the window far outside every monitor, never activated and out of
/// the taskbar, so a self-test paints without reaching the desktop.
pub fn off_desktop(window: &gpui::Window, size: Size<Pixels>) -> Option<Place> {
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TOOLWINDOW: isize = 0x80;
    const WS_EX_NOACTIVATE: isize = 0x0800_0000;
    const WS_EX_APPWINDOW: isize = 0x4_0000;
    let hwnd = hwnd(window)?;
    // SAFETY: the window's own handle and plain values.
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            (ex | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) & !WS_EX_APPWINDOW,
        );
    }
    // A window opened hidden still has CW_USEDEFAULT's size, which GPUI
    // reads as its viewport until a WM_SIZE, so give its client area the
    // size asked for, plus whatever frame the window has.
    let scale = window.scale_factor();
    let (mut outer, mut client) = ([0i32; 4], [0i32; 4]);
    // SAFETY: the window's own handle and RECT-sized buffers.
    unsafe {
        GetWindowRect(hwnd, &raw mut outer);
        GetClientRect(hwnd, &raw mut client);
    }
    let frame = (
        (outer[2] - outer[0]) - (client[2] - client[0]),
        (outer[3] - outer[1]) - (client[3] - client[1]),
    );
    let (w, h) = (
        (f32::from(size.width) * scale).round() as i32 + frame.0,
        (f32::from(size.height) * scale).round() as i32 + frame.1,
    );
    Some(Place(
        hwnd,
        -30000,
        -30000,
        w,
        h,
        SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
    ))
}
