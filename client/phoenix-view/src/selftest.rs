//! `--selftest`: drive the window off the desktop and check what the real
//! guest does.
//!
//! The window is shown far outside every monitor and never activated. The
//! click on "Log In" goes through GPUI's own `Window::dispatch_event`, the
//! text goes through the field's input handler (the path the IME takes) and
//! the resize through `SetWindowPos` on this process's own window, without
//! activating it. No OS input is synthesised.
//!
//! With `--live` the run also checks the network: the requests the guest
//! makes on its own get answers, the allowlist refuses a host that isn't
//! Discord's, and a login with an address that can't exist (`.invalid` is
//! reserved, RFC 2606) reaches discord.com and shows Discord's answer.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, Entity, EntityInputHandler, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point, Window, px,
};
use rekindle_wire::{PropId, Value};

use crate::view::{PhoenixView, ids};

const TICK: Duration = Duration::from_millis(100);
const PATIENCE: Duration = Duration::from_secs(20);
/// How long a screen stands before it is captured.
const SETTLE: Duration = Duration::from_millis(1500);

/// The live run's login. Neither can belong to an account.
const FAKE_EMAIL: &str = "invalid@example.invalid";
const FAKE_PASSWORD: &str = "not-a-real-password-0";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Step {
    Welcome,
    Login,
    Typed,
    Password,
    Submitted,
    Refused,
    Resized,
    Done,
}

struct Run {
    step: Step,
    since: Instant,
    /// Turns seen when the press was sent, and when the resize was.
    mark: usize,
    /// The welcome screen's Log In, which the run presses.
    login: rekindle_wire::NodeId,
    pass: Vec<String>,
    fail: Vec<String>,
    typed: Option<u32>,
    live: bool,
    /// Texts on screen when the login went out.
    before: Vec<String>,
    /// Where the captures and tree dumps go.
    work: PathBuf,
    /// When the welcome screen was first whole on screen.
    settled: Option<Instant>,
}

impl Run {
    fn next(&mut self, step: Step) {
        self.step = step;
        self.since = Instant::now();
    }
}

pub fn start(
    view: Entity<PhoenixView>,
    live: bool,
    work: PathBuf,
    window: &mut Window,
    cx: &mut App,
) {
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        // Shown from here, where nothing holds the app, so GPUI can take the
        // WM_SIZE the move sends.
        if let Ok(Some(place)) =
            handle.update(cx, |_, window, _| crate::off_desktop(window, crate::PHONE))
        {
            place.apply();
        }
        let mut run = Run {
            step: Step::Welcome,
            since: Instant::now(),
            mark: 0,
            login: 0,
            pass: vec![],
            fail: vec![],
            typed: None,
            live,
            before: vec![],
            work,
            settled: None,
        };
        loop {
            cx.background_executor().timer(TICK).await;
            let Ok(place) = handle.update(cx, |_, window, cx| {
                window.refresh();
                tick(&view, &mut run, window, cx)
            }) else {
                return;
            };
            if let Some(place) = place {
                place.apply();
            }
            if run.step == Step::Done {
                break;
            }
        }
        for l in &run.pass {
            println!("pass: {l}");
        }
        for l in &run.fail {
            println!("FAIL: {l}");
        }
        let ok = run.fail.is_empty();
        println!(
            "phoenix-view selftest: {} ({} pass, {} fail)",
            if ok { "ok" } else { "FAILED" },
            run.pass.len(),
            run.fail.len()
        );
        crate::guest::log(&format!(
            "[selftest] {} pass, {} fail",
            run.pass.len(),
            run.fail.len()
        ));
        let () = cx.update(|cx| cx.quit());
        std::process::exit(i32::from(!ok));
    })
    .detach();
}

/// `--snapshot`: show the window off the desktop, wait `secs`, save what it
/// shows and its tree to the work dir. Then, for each of `clicks`, click
/// the node with that label through the window's own event dispatch and
/// save what follows. Quit.
pub fn snapshot(
    view: Entity<PhoenixView>,
    secs: u64,
    (clicks, frames): (Vec<String>, Vec<u64>),
    size: gpui::Size<Pixels>,
    work: PathBuf,
    window: &mut Window,
    cx: &mut App,
) {
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        if let Ok(Some(place)) = handle.update(cx, |_, window, _| crate::off_desktop(window, size)) {
            place.apply();
        }
        let wait = async |cx: &mut gpui::AsyncApp, secs: u64| {
            let until = Instant::now() + Duration::from_secs(secs);
            while Instant::now() < until {
                cx.background_executor().timer(TICK).await;
                let _ = handle.update(cx, |_, window, _| window.refresh());
            }
            // One more frame, so the tree and the bounds agree.
            cx.background_executor().timer(TICK).await;
        };
        wait(cx, secs).await;
        let save = |name: &str, window: &mut Window, cx: &mut App| {
            let v = view.read(cx);
            let tree = work.join(format!("{name}-tree.txt"));
            let png = work.join(format!("{name}.png"));
            if let Err(e) = std::fs::write(&tree, v.dump()) {
                eprintln!("phoenix-view: writing {}: {e}", tree.display());
            }
            match crate::capture::client_area(window).and_then(|s| s.save(&png)) {
                Ok(()) => {
                    crate::guest::log(&format!("[snapshot] {name}"));
                    println!(
                        "phoenix-view: snapshot in {} and {}",
                        png.display(),
                        tree.display()
                    );
                }
                Err(e) => eprintln!("phoenix-view: no capture: {e:#}"),
            }
        };
        let _ = handle.update(cx, |_, window, cx| save("snapshot", window, cx));
        for (n_click, label) in clicks.iter().enumerate() {
            // `scroll:<dp>` is a touchpad's scroll over the window's centre
            // by that many dp, `wheel:<notches>` a mouse wheel's notches,
            // and `fling:<dp>` a quick drag of that many dp that lets go
            // moving; each positive toward older rows at the top.
            // `@<x>,<y>` after it puts the pointer there instead, in dp.
            let action = label.split_once(':').and_then(|(kind, rest)| {
                let (n, at) = match rest.split_once('@') {
                    Some((n, at)) => {
                        let (x, y) = at.split_once(',')?;
                        (n, Some(gpui::point(px(x.parse().ok()?), px(y.parse().ok()?))))
                    }
                    None => (rest, None),
                };
                let n = n.parse::<f32>().ok()?;
                matches!(kind, "scroll" | "wheel" | "fling").then_some((kind, n, at))
            });
            if let Some((kind, n, at)) = action {
                let began = handle.update(cx, |_, _, cx| view.read(cx).now()).unwrap_or(0.);
                let at = at.unwrap_or(gpui::point(size.width / 2., size.height / 2.));
                let modifiers = Modifiers::default();
                let scroll = |delta| {
                    PlatformInput::Scroll(gpui::ScrollEvent {
                        position: at,
                        delta,
                        modifiers,
                        touch_phase: gpui::TouchPhase::Moved,
                        source: gpui::ScrollSource::default(),
                    })
                };
                match kind {
                    "scroll" => {
                        let ev = scroll(gpui::ScrollDelta::Pixels(gpui::point(Pixels::ZERO, px(n))));
                        let _ = handle.update(cx, |_, window, cx| window.dispatch_event(ev, cx));
                    }
                    "wheel" => {
                        // Windows reports a notch as three lines.
                        for _ in 0..(n.abs().round() as u32) {
                            let ev = scroll(gpui::ScrollDelta::Lines(gpui::point(0., 3. * n.signum())));
                            let _ = handle.update(cx, |_, window, cx| window.dispatch_event(ev, cx));
                            cx.background_executor().timer(Duration::from_millis(30)).await;
                        }
                    }
                    _ => {
                        // Down, eight moves 12 ms apart, and up while still
                        // moving, as a flick of a finger.
                        let _ = handle.update(cx, |_, window, cx| {
                            window.dispatch_event(
                                PlatformInput::MouseDown(MouseDownEvent {
                                    button: MouseButton::Left,
                                    position: at,
                                    modifiers,
                                    click_count: 1,
                                    first_mouse: false,
                                }),
                                cx,
                            );
                        });
                        let mut to = at;
                        for _ in 0..8 {
                            cx.background_executor().timer(Duration::from_millis(12)).await;
                            to.y += px(n / 8.);
                            let _ = handle.update(cx, |_, window, cx| {
                                window.dispatch_event(
                                    PlatformInput::MouseMove(MouseMoveEvent {
                                        position: to,
                                        pressed_button: Some(MouseButton::Left),
                                        modifiers,
                                    }),
                                    cx,
                                );
                            });
                        }
                        let _ = handle.update(cx, |_, window, cx| {
                            window.dispatch_event(
                                PlatformInput::MouseUp(MouseUpEvent {
                                    button: MouseButton::Left,
                                    position: to,
                                    modifiers,
                                    click_count: 1,
                                }),
                                cx,
                            );
                        });
                    }
                }
                // Frames of the motion that follows, from when the input
                // stopped.
                let stopped = Instant::now();
                for ms in &frames {
                    let due = stopped + Duration::from_millis(*ms);
                    while Instant::now() < due {
                        cx.background_executor()
                            .timer((due - Instant::now()).min(Duration::from_millis(8)))
                            .await;
                        let _ = handle.update(cx, |_, window, _| window.refresh());
                    }
                    let _ = handle.update(cx, |_, window, cx| {
                        save(&format!("snapshot-{}-{ms}", n_click + 1), window, cx);
                    });
                }
                wait(cx, 5).await;
                // The frames of the scroll and its motion, for three
                // seconds from the input.
                let costs = handle
                    .update(cx, |_, _, cx| {
                        let v = view.read(cx);
                        let f = v.stats.frames.borrow();
                        f.iter().filter(|c| c.at >= began && c.at <= began + 3000.).copied().collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                println!("phoenix-view: {kind} {n}: {}", frame_summary(&costs));
                let _ = handle.update(cx, |_, window, cx| {
                    save(&format!("snapshot-{}", n_click + 1), window, cx);
                });
                continue;
            }
            // `action:<name>@<label>` performs the accessibility action
            // <name> on the node labelled <label>.
            if let Some((name, want)) = label.strip_prefix("action:").and_then(|r| r.split_once('@')) {
                let sent = handle
                    .update(cx, |_, _, cx| {
                        view.update(cx, |v, _| {
                            let node = v.find_labelled(want)?;
                            let value = Value::Object(vec![(
                                PropId::from_name("name")?,
                                Value::Str(name.to_string()),
                            )]);
                            Some((node, v.node_event(node, "accessibilityAction", value)))
                        })
                    })
                    .ok()
                    .flatten();
                println!("phoenix-view: accessibility action {name:?} on {want:?}: {sent:?}");
                wait(cx, 5).await;
                let _ = handle.update(cx, |_, window, cx| {
                    save(&format!("snapshot-{}", n_click + 1), window, cx);
                });
                continue;
            }
            // `press:<label>` sends `press` to the node labelled <label>,
            // painted or not.
            if let Some(want) = label.strip_prefix("press:") {
                let sent = handle
                    .update(cx, |_, _, cx| {
                        view.update(cx, |v, _| {
                            let node = v.find_labelled(want)?;
                            Some((node, v.node_event(node, "press", Value::Null)))
                        })
                    })
                    .ok()
                    .flatten();
                println!("phoenix-view: press on {want:?}: {sent:?}");
                wait(cx, 5).await;
                let _ = handle.update(cx, |_, window, cx| {
                    save(&format!("snapshot-{}", n_click + 1), window, cx);
                });
                continue;
            }
            // `long:<label>` long-presses the node labelled <label>.
            if let Some(want) = label.strip_prefix("long:") {
                let sent = handle
                    .update(cx, |_, _, cx| {
                        view.update(cx, |v, _| {
                            let node = v.find_labelled(want)?;
                            Some((node, v.long_press(node)))
                        })
                    })
                    .ok()
                    .flatten();
                println!("phoenix-view: long press on {want:?}: {sent:?}");
                wait(cx, 5).await;
                let _ = handle.update(cx, |_, window, cx| {
                    save(&format!("snapshot-{}", n_click + 1), window, cx);
                });
                continue;
            }
            // `link:<text>` clicks the link span whose text holds <text>.
            if let Some(want) = label.strip_prefix("link:") {
                let at = handle
                    .update(cx, |_, _, cx| view.read(cx).link_point(want))
                    .ok()
                    .flatten();
                match at {
                    Some(at) => {
                        let _ = handle.update(cx, |_, window, cx| click(window, cx, at));
                        wait(cx, 3).await;
                        println!("phoenix-view: clicked the link {want:?} at {at:?}");
                        let _ = handle.update(cx, |_, window, cx| {
                            save(&format!("snapshot-{}", n_click + 1), window, cx);
                        });
                    }
                    None => eprintln!("phoenix-view: no link drawn with {want:?}"),
                }
                continue;
            }
            let clicked = handle.update(cx, |_, window, cx| {
                let v = view.read(cx);
                let node = v.find_labelled(label)?;
                let b = painted(v, node)?;
                let (sent, turns) = (v.stats.events.len(), v.stats.turns.len());
                click(window, cx, b.center());
                let v = view.read(cx);
                Some((node, b.center(), v.stats.events[sent..].to_vec(), turns))
            });
            match clicked {
                Ok(Some((node, at, events, turns))) => {
                    // Frames of the motion the click started, the window
                    // asked to draw meanwhile as a shown one draws.
                    let clicked_at = Instant::now();
                    for ms in &frames {
                        let due = clicked_at + Duration::from_millis(*ms);
                        while Instant::now() < due {
                            cx.background_executor()
                                .timer((due - Instant::now()).min(Duration::from_millis(8)))
                                .await;
                            let _ = handle.update(cx, |_, window, _| window.refresh());
                        }
                        let _ = handle.update(cx, |_, window, cx| {
                            save(&format!("snapshot-{}-{ms}", n_click + 1), window, cx);
                        });
                    }
                    wait(cx, 5).await;
                    let after = handle
                        .update(cx, |_, _, cx| view.read(cx).stats.turns.len() - turns)
                        .unwrap_or(0);
                    println!(
                        "phoenix-view: click on {label:?} (#{node}) at {at:?} sent {events:?}; {after} turns followed"
                    );
                    let _ = handle.update(cx, |_, window, cx| {
                        save(&format!("snapshot-{}", n_click + 1), window, cx);
                    });
                }
                _ => eprintln!("phoenix-view: no painted node labelled {label:?}"),
            }
        }
        // Frames drawn while nothing moves and nothing asks for one.
        let count = |cx: &mut gpui::AsyncApp| {
            handle
                .update(cx, |_, _, cx| view.read(cx).stats.frames.borrow().len())
                .unwrap_or(0)
        };
        let before = count(cx);
        cx.background_executor().timer(Duration::from_secs(3)).await;
        println!("phoenix-view: idle: {} frames in 3 s", count(cx).saturating_sub(before));
        let () = cx.update(|cx| cx.quit());
        std::process::exit(0);
    })
    .detach();
}

/// The frames' count and their cost, in ms: `render` alone and the whole
/// frame to the end of its paint.
fn frame_summary(costs: &[crate::view::FrameCost]) -> String {
    let costs: Vec<_> = costs.iter().filter(|c| c.scrolling).copied().collect();
    if costs.is_empty() {
        return "no frames".into();
    }
    let stats = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        let pct = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
        format!(
            "mean {mean:.1}, median {:.1}, p95 {:.1}, max {:.1}",
            pct(0.5),
            pct(0.95),
            pct(1.)
        )
    };
    let nodes = costs.iter().map(|c| c.nodes).sum::<usize>() / costs.len();
    let span = costs.last().unwrap().at - costs[0].at;
    let fps = if span > 0. {
        (costs.len() - 1) as f64 * 1000. / span
    } else {
        0.
    };
    let gaps: Vec<f64> = costs.windows(2).map(|w| w[1].at - w[0].at).collect();
    let gaps = if gaps.is_empty() {
        "none".into()
    } else {
        stats(gaps)
    };
    format!(
        "{} frames of motion over {span:.0} ms, {fps:.0} fps; between frames {gaps}; render {}; frame {}; {nodes} nodes built a frame",
        costs.len(),
        stats(costs.iter().map(|c| c.render).collect()),
        stats(costs.iter().map(|c| c.total).collect()),
    )
}

fn painted(view: &PhoenixView, node: u32) -> Option<Bounds<Pixels>> {
    view.hits
        .borrow()
        .get(&node)
        .copied()
        .filter(|b| b.size.width > Pixels::ZERO && b.size.height > Pixels::ZERO)
}

/// One step of the run, and a resize to make after it.
fn tick(
    view: &Entity<PhoenixView>,
    run: &mut Run,
    window: &mut Window,
    cx: &mut App,
) -> Option<crate::Place> {
    if run.since.elapsed() > PATIENCE {
        let v = view.read(cx);
        run.fail.push(format!(
            "{:?}: nothing after {} s (turns {:?}, {} nodes, {} shown, {} painted, title {:?}, Log In {:?}, guest {}, viewport {:?} at scale {})",
            run.step,
            PATIENCE.as_secs(),
            v.stats.turns,
            v.tree.len(),
            v.shown().len(),
            v.hits.borrow().len(),
            v.find_text("WELCOME TO DISCORD"),
            v.find_button("Log In"),
            v.stopped.as_deref().unwrap_or("running"),
            window.viewport_size(),
            window.scale_factor()
        ));
        run.next(Step::Done);
        return None;
    }
    match run.step {
        Step::Welcome => {
            let v = view.read(cx);
            let (Some(title), Some(login)) =
                (v.find_text("WELCOME TO DISCORD"), v.find_button("Log In"))
            else {
                return None;
            };
            let (Some(tb), Some(lb)) = (painted(v, title), painted(v, login)) else {
                return None;
            };
            // Images decode off the UI thread: give them a moment.
            let settled = *run.settled.get_or_insert_with(Instant::now);
            if settled.elapsed() < SETTLE {
                return None;
            }
            screen_checks(v, "welcome", &["Log In", "Register"], window, run);
            run.pass.push(format!(
                "welcome screen rendered: {} nodes from {} turns {:?}, \"WELCOME TO DISCORD\" (#{title}) painted at {:?}, \"Log In\" (#{login}) at {:?}, {} host calls",
                v.tree.len(),
                v.stats.turns.len(),
                v.stats.turns,
                tb.origin,
                lb.origin,
                v.guest.as_ref().map_or(0, crate::guest::Guest::calls)
            ));
            run.mark = v.stats.turns.len();
            run.login = login;
            click(window, cx, lb.center());
            let sent = view
                .read(cx)
                .stats
                .events
                .iter()
                .any(|(n, e)| *n == login && e == "press");
            if sent {
                run.pass.push(format!(
                    "click at {:?} through Window::dispatch_event sent press (614) to #{login}",
                    lb.center()
                ));
            } else {
                run.fail
                    .push(format!("the click at {:?} sent no press", lb.center()));
            }
            run.next(Step::Login);
        }
        Step::Login => {
            let v = view.read(cx);
            let back = v.find_text("Welcome back!")?;
            let b = painted(v, back)?;
            if run.since.elapsed() < SETTLE {
                return None;
            }
            screen_checks(v, "login", &["Log In"], window, run);
            let after = &v.stats.turns[run.mark..];
            run.pass.push(format!(
                "press -> login screen: turns after the press {after:?}{}, \"Welcome back!\" (#{back}) painted at {:?}, {} nodes",
                if after.contains(&7128) { ", the 7128-byte login turn among them" } else { "" },
                b.origin,
                v.tree.len()
            ));
            if !after.contains(&7128) {
                run.fail
                    .push(format!("no 7128-byte turn after the press: {after:?}"));
            }
            // The click pressed Log In, whose `press_look` worklet the
            // library's motion engine runs: it writes the button's
            // Background from the worklet's own colours.
            let m = &v.motion;
            let wrote = m
                .seen
                .contains(&(run.login, crate::motion::channel::BACKGROUND));
            let line = format!(
                "motion: {} worklet cells, {} viewMotion cells, {} ticks, {} channel writes, {} op 8 events; the press on #{} wrote its Background: {wrote}",
                m.counts.worklets,
                m.counts.view_motions,
                m.counts.ticks,
                m.counts.writes,
                m.counts.events,
                run.login
            );
            if wrote && m.counts.ticks > 0 {
                run.pass.push(line);
            } else if m.enabled {
                run.fail.push(line);
            }
            if let Some(work) = theme_check(v) {
                run.pass.push(work);
            }
            run.next(Step::Typed);
        }
        Step::Typed => {
            let Some(node) = run.typed else {
                let v = view.read(cx);
                let &node = v.shown_inputs().first()?;
                let input = v.input(node)?;
                run.mark = v.stats.turns.len();
                input.update(cx, |i, cx| {
                    let focus = gpui::Focusable::focus_handle(i, cx);
                    window.focus(&focus, cx);
                    i.replace_text_in_range(None, email(run.live), window, cx);
                });
                run.typed = Some(node);
                return None;
            };
            if run.since.elapsed() < Duration::from_secs(2) {
                return None;
            }
            let v = view.read(cx);
            let sent = v
                .stats
                .events
                .iter()
                .filter(|(n, e)| *n == node && e == "changeText")
                .count();
            let value = match v.tree.props().get(node, ids().value) {
                Some(Value::Str(s)) => s.clone(),
                _ => String::new(),
            };
            let shown = v
                .input(node)
                .map(|i| i.read(cx).text().to_string())
                .unwrap_or_default();
            let after = &v.stats.turns[run.mark..];
            if sent > 0 && shown == email(run.live) {
                run.pass.push(format!(
                    "typing in the email field (#{node}) sent changeText (576) {sent}x; the field shows {shown:?}; the guest's value cell is {value:?}; turns after: {after:?}"
                ));
            } else {
                run.fail.push(format!(
                    "typing in #{node}: {sent} changeText sent, field shows {shown:?}"
                ));
            }
            run.mark = v.stats.turns.len();
            if run.live {
                run.next(Step::Password);
                return None;
            }
            run.next(Step::Resized);
            return crate::resize_quietly(window, 700., 640.);
        }
        Step::Password => {
            let v = view.read(cx);
            let &node = v.shown_inputs().get(1)?;
            let input = v.input(node)?;
            input.update(cx, |i, cx| {
                let focus = gpui::Focusable::focus_handle(i, cx);
                window.focus(&focus, cx);
                i.replace_text_in_range(None, FAKE_PASSWORD, window, cx);
            });
            let v = view.read(cx);
            let login = v.find_button("Log In")?;
            let b = painted(v, login)?;
            run.before = texts(v);
            run.pass.push(format!(
                "live: {}",
                unauthenticated(&view.read(cx).guest.as_ref()?.net()?.exchanges())
            ));
            click(window, cx, b.center());
            run.pass.push(format!(
                "typed {FAKE_EMAIL:?} and a password into #{node}, pressed \"Log In\" (#{login})"
            ));
            run.next(Step::Submitted);
        }
        Step::Submitted => {
            let v = view.read(cx);
            let net = v.guest.as_ref()?.net()?;
            let xs = net.exchanges();
            let login = xs.iter().find(|x| x.path.ends_with("/auth/login"))?;
            // The guest's answer to the response takes a frame or two.
            if run.since.elapsed() < Duration::from_secs(3) {
                return None;
            }
            let now = texts(v);
            let new: Vec<&String> = now.iter().filter(|t| !run.before.contains(t)).collect();
            let line = format!(
                "login: {} {} {} -> {}{}; texts new on screen: {new:?}",
                login.method,
                login.host,
                login.path,
                login
                    .status
                    .map_or_else(|| login.note.clone(), |s| s.to_string()),
                if login.captcha {
                    "; Discord asked for a captcha"
                } else {
                    ""
                }
            );
            if login.status.is_some() && login.host == "discord.com" {
                run.pass.push(line);
            } else {
                run.fail.push(line);
            }
            run.next(Step::Refused);
        }
        Step::Refused => {
            let mut lines = vec![];
            for url in [
                "https://example.com/",
                "wss://dealer.spotify.com/",
                "http://discord.com/api",
            ] {
                // Networking takes https, WebSocket wss.
                let scheme = if url.starts_with("wss:") {
                    "wss"
                } else {
                    "https"
                };
                match crate::net::check(url, scheme) {
                    Ok(_) => run.fail.push(format!("the allowlist let {url} through")),
                    Err(why) => {
                        crate::guest::log(&format!("[selftest] allowlist refused: {why}"));
                        lines.push(why);
                    }
                }
            }
            run.pass
                .push(format!("the allowlist refused: {}", lines.join("; ")));
            run.mark = view.read(cx).stats.turns.len();
            run.next(Step::Resized);
            return crate::resize_quietly(window, 700., 640.);
        }
        Step::Resized => {
            let v = view.read(cx);
            let &at = v.stats.window_events.last()?;
            if v.stats.window_events.len() < 2 || at < run.mark {
                return None;
            }
            let after = &v.stats.turns[at..];
            if after.is_empty() {
                if run.since.elapsed() > Duration::from_secs(3) {
                    run.fail.push(format!(
                        "resize to {:?}: a window event went out, no turn came back",
                        window.viewport_size()
                    ));
                    run.next(Step::Done);
                }
                return None;
            }
            run.pass.push(format!(
                "resize to {:?} sent window (op 5); the guest re-laid out with turns {after:?}",
                window.viewport_size()
            ));
            run.next(Step::Done);
        }
        Step::Done => {}
    }
    None
}

/// Capture the screen and dump its tree to the work dir, and check that
/// each button in `buttons` lies inside the viewport and that every image
/// on screen has its drawable.
fn screen_checks(v: &PhoenixView, name: &str, buttons: &[&str], window: &Window, run: &mut Run) {
    let tree = run.work.join(format!("{name}-tree.txt"));
    if let Err(e) = std::fs::write(&tree, v.dump()) {
        run.fail
            .push(format!("{name}: writing {}: {e}", tree.display()));
    }
    let png = run.work.join(format!("{name}.png"));
    match crate::capture::client_area(window).and_then(|shot| {
        shot.save(&png)?;
        Ok(shot)
    }) {
        Ok(shot) => run.pass.push(format!(
            "{name}: captured {}x{} px to {} ({} colours sampled), tree in {}",
            shot.width,
            shot.height,
            png.display(),
            colours(&shot),
            tree.display()
        )),
        Err(e) => run.fail.push(format!("{name}: no capture: {e:#}")),
    }
    let view = window.viewport_size();
    let inside = |b: Bounds<Pixels>| {
        b.origin.x >= Pixels::ZERO
            && b.origin.y >= Pixels::ZERO
            && b.origin.x + b.size.width <= view.width + px(0.5)
            && b.origin.y + b.size.height <= view.height + px(0.5)
    };
    for label in buttons {
        match v
            .find_button(label)
            .and_then(|n| painted(v, n).map(|b| (n, b)))
        {
            Some((n, b)) if inside(b) => run.pass.push(format!(
                "{name}: \"{label}\" (#{n}) at {b:?} lies inside the {view:?} viewport"
            )),
            Some((n, b)) => run.fail.push(format!(
                "{name}: \"{label}\" (#{n}) at {b:?} runs outside the {view:?} viewport"
            )),
            None => run
                .fail
                .push(format!("{name}: no painted \"{label}\" button")),
        }
    }
    for (node, uri, image) in v.shown_images() {
        match image {
            Some(Ok((_, how))) => run.pass.push(format!("{name}: image #{node} {uri}: {how}")),
            // Not this platform's fault: the device's density split is
            // needed. Said, not failed, so a run without it still checks
            // the rest.
            Some(Err(miss)) if miss.apk_incomplete => run.pass.push(format!(
                "{name}: image #{node} {uri} NOT DRAWN, the APK set is incomplete: {}",
                miss.why
            )),
            Some(Err(miss)) => run.fail.push(format!(
                "{name}: image #{node} {uri} not drawn: {}",
                miss.why
            )),
            None => run
                .fail
                .push(format!("{name}: image #{node} {uri} was never drawn")),
        }
    }
}

/// How many distinct colours a sparse grid of the capture holds: a blank or
/// black capture has one.
fn colours(shot: &crate::capture::Shot) -> usize {
    let mut seen = std::collections::HashSet::new();
    for y in (0..shot.height).step_by(7) {
        for x in (0..shot.width).step_by(7) {
            seen.insert(shot.pixel(x, y));
        }
    }
    seen.len()
}

fn email(live: bool) -> &'static str {
    if live {
        FAKE_EMAIL
    } else {
        "phoenix@example.com"
    }
}

/// The texts on screen, in tree order.
fn texts(v: &PhoenixView) -> Vec<String> {
    v.shown()
        .into_iter()
        .filter_map(|n| v.text_of(n).map(str::to_string))
        .collect()
}

/// What the guest fetched on its own before the login: method, host, path
/// and status of each request.
fn unauthenticated(xs: &[crate::net::Exchange]) -> String {
    let answered = xs.iter().filter(|x| x.status.is_some()).count();
    format!(
        "{answered} of {} requests the guest made on its own were answered: {}",
        xs.len(),
        xs.iter()
            .map(|x| format!(
                "#{} {} {}{} -> {}",
                x.id,
                x.method,
                x.host,
                x.path,
                x.status.map_or_else(|| x.note.clone(), |s| s.to_string())
            ))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// With a theme running: the cells the engine recoloured on screen.
fn theme_check(v: &PhoenixView) -> Option<String> {
    if v.stats.themed_turns == 0 {
        return None;
    }
    let colours: Vec<u32> = v
        .shown()
        .into_iter()
        .filter_map(|n| {
            v.tree
                .props()
                .get(n, ids().background)
                .and_then(Value::as_color)
        })
        .collect();
    Some(format!(
        "rekindle's wire engine rewrote {} of the guest's turns; backgrounds on screen now: {}",
        v.stats.themed_turns,
        colours
            .iter()
            .map(|c| format!("{c:08x}"))
            .collect::<Vec<_>>()
            .join(" ")
    ))
}

/// A left click at `at`, through the window's own event dispatch.
fn click(window: &mut Window, cx: &mut App, at: Point<Pixels>) {
    let modifiers = Modifiers::default();
    window.dispatch_event(
        PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            pressed_button: None,
            modifiers,
        }),
        cx,
    );
    window.dispatch_event(
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers,
            click_count: 1,
            first_mouse: false,
        }),
        cx,
    );
    window.dispatch_event(
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers,
            click_count: 1,
        }),
        cx,
    );
}
