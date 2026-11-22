//! Does bad things when asked. Used by `plugin-host/tests/enforcement.rs`.

use rekindle_sdk::prelude::*;

struct Misbehave {
    hoard: Vec<Vec<u8>>,
    /// `spin-events` was run: the next event handler never returns.
    spin_events: bool,
}

impl Plugin for Misbehave {
    fn activate(granted: &Granted) -> Result<Self, String> {
        match storage::get("fail-activate").as_deref() {
            // A version whose manifest adds `audio` fails; this one doesn't
            // (reload rollback, in enforcement.rs).
            Some("with-audio") if !granted.has(Capability::Audio) => {}
            Some(_) => return Err("refusing to start, as asked".into()),
            None => {}
        }
        theme::set("accent", Color::rgb(1, 2, 3));
        // Not a user gesture: the host must refuse these.
        info!(
            "activate clipboard: {:?}",
            clipboard::write_text("swapped behind your back")
        );
        info!(
            "activate act: {:?} {:?}",
            actions::mark_all_read(),
            actions::react(&any_message(), "👍")
        );
        for id in [
            "spin",
            "spin-events",
            "split-now",
            "panic",
            "oom",
            "storage-flood",
            "log-flood",
            "theme-v2",
            "slots",
            "clipboard",
            "budget",
            "presence-later",
            "far-timers",
        ] {
            ui::register_command(id, id);
        }
        let _ = timer::set("from-timer", 50);
        Ok(Misbehave {
            hoard: Vec::new(),
            spin_events: false,
        })
    }

    fn on_event(&mut self, _event: Event) {
        if self.spin_events {
            let mut x: u64 = 0;
            loop {
                x = std::hint::black_box(x.wrapping_add(1));
            }
        }
    }

    /// Splits as the draft's text says, and reports what the host answered
    /// as the reason for cancelling, for the test to read.
    fn on_compose(&mut self, d: &compose::Draft) -> compose::ComposeResult {
        let parts: &[&str] = match d.text.as_str() {
            "split-two" => &["one", "two"],
            "split-one" => &["only"],
            "split-blank" => &["one", "  "],
            "split-eleven" => &["x"; 11],
            _ => return compose::ComposeResult::Keep,
        };
        match compose::split(parts) {
            Ok(()) if d.text == "split-two" => compose::ComposeResult::Keep,
            r => compose::cancel(&format!("{r:?}")),
        }
    }

    fn on_command(&mut self, id: &str) {
        match id {
            "spin-events" => self.spin_events = true,
            "split-now" => info!("split outside compose: {:?}", compose::split(&["a", "b"])),
            "spin" => {
                let mut x: u64 = 0;
                loop {
                    x = std::hint::black_box(x.wrapping_add(1));
                }
            }
            "panic" => panic!("deliberate panic"),
            "oom" => loop {
                self.hoard.push(vec![1u8; 1 << 20]);
            },
            "storage-flood" => {
                let big = "x".repeat(60 * 1024);
                for i in 0.. {
                    if let Err(e) = storage::set(&format!("k{i}"), &big) {
                        info!("storage refused write {i}: {e:?}");
                        return;
                    }
                }
            }
            "log-flood" => {
                let line = "y".repeat(100_000);
                for _ in 0..50 {
                    info!("{line}");
                }
            }
            "slots" => {
                // Not misbehaviour as such: probes the slot limits/validation.
                use slots::{CHAT_BAR, MESSAGE_MENU, MenuItem, UiError, add, button, is_available};
                let filled = (0..25)
                    .all(|i| button(CHAT_BAR, &format!("b{i}"), Icon::Star, "One", false).is_ok());
                let r = [
                    filled,
                    button(CHAT_BAR, "b25", Icon::Star, "One too many", false)
                        == Err(UiError::Limit),
                    button(CHAT_BAR, "b1", Icon::Bolt, "One again", true).is_ok(), // replace is fine
                    add(MESSAGE_MENU, MenuItem::new("m1", "")) == Err(UiError::Invalid),
                    add(MESSAGE_MENU, MenuItem::new("m1", &"x".repeat(49)))
                        == Err(UiError::Invalid),
                    add(MESSAGE_MENU, MenuItem::new("bad id!", "ok")) == Err(UiError::Invalid),
                    button(MESSAGE_MENU, "x", Icon::Star, "a button in a menu", false)
                        == Err(UiError::Invalid),
                    add("no.such-slot", MenuItem::new("x", "x")) == Err(UiError::Unavailable),
                    is_available(MESSAGE_MENU),
                ];
                for i in 0..5 {
                    ui::toast(&format!("toast {i}"));
                }
                info!("slots results: {r:?}");
            }
            "presence-later" => {
                let _ = timer::set("presence", 50);
            }
            "budget" => {
                // A gesture carries 3 reactions and 10 local actions.
                let m = any_message();
                // "Not refused for want of a gesture" (the mock may still
                // say the message isn't loaded).
                let allowed = |r: actions::Result| r != Err(actions::ActionError::NoGesture);
                let reacts: Vec<bool> = (0..4).map(|_| allowed(actions::react(&m, "👍"))).collect();
                let locals: Vec<bool> =
                    (0..11).map(|_| allowed(actions::mark_all_read())).collect();
                info!(
                    "budget results: {reacts:?} {}",
                    locals.iter().filter(|ok| **ok).count()
                );
            }
            "theme-v2" => {
                // Not misbehaviour: exercises every theming-v2 call end to end.
                let r = [
                    theme::set_radius("radius-md", 0.0).is_ok(),
                    theme::set_radius("radius-md", 1000.0).is_err(),
                    theme::set_density(theme::Density::Compact).is_ok(),
                    theme::set_value("density", theme::ThemeValue::Color(Color::rgb(0, 0, 0)))
                        .is_err(),
                    theme::set_font("font-primary", "Unregistered").is_err(),
                    theme::register_font("Fixture Sans", b"not a font").is_err(),
                    theme::register_font("Fixture Sans", &[0, 1, 0, 0, 0, 0, 0, 0]).is_ok(),
                    theme::set_font("font-primary", "Fixture Sans").is_ok(),
                    theme::set("background-primary", Color::rgb(9, 9, 9)), // legacy alias
                    theme::group("radius").len() == 5,
                ];
                theme::tint(Color::rgb(255, 0, 128), 0.25);
                info!("theme-v2 results: {r:?}");
            }
            "far-timers" => info!(
                "far timers: {:?} {:?}",
                timer::set("never", u64::MAX),
                timer::repeat("forever", u64::MAX)
            ),
            "clipboard" => info!(
                "command clipboard: {:?}",
                clipboard::write_text("from a gesture")
            ),
            _ => {}
        }
    }

    fn on_timer(&mut self, id: &str) {
        if id == "never" || id == "forever" {
            info!("fired {id}");
            return;
        }
        if id == "presence" {
            // Your own activity needs no gesture (rate-limited); status does.
            let rich: Vec<bool> = (0..6)
                .map(|i| {
                    rekindle_sdk::presence::set_rich_presence(Some(
                        &rekindle_sdk::presence::playing(&format!("game {i}")),
                    ))
                    .is_ok()
                })
                .collect();
            info!(
                "timer presence: {:?} {rich:?}",
                rekindle_sdk::presence::set_status("idle")
            );
            return;
        }
        // Timers are never gestures.
        info!("timer act: {:?}", actions::mark_all_read());
    }

    /// The render isolate can't write: this traps the isolate (not the
    /// plugin) and the message renders untransformed.
    fn render(input: &render::RenderInput) -> Vec<Transform> {
        if input.message.content.contains("exfiltrate") {
            let _ = storage::set("stolen", &input.message.content);
        }
        if input.message.content.contains("link") {
            return vec![render::append(vec![
                span("see").target(Target::Link("https://evil.example/?q=secret".into())),
            ])];
        }
        vec![render::collapse("probe")]
    }
}

/// The sample client's first message (id/channel only matter to the mock).
fn any_message() -> MessageRef {
    MessageRef {
        id: 1,
        channel: 1002,
        guild: Some(1000),
        author: 101,
    }
}

rekindle_sdk::plugin!(Misbehave);
