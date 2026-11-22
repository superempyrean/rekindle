//! Call Timer, after Vencord's `CallTimer`: how long you've been in voice, as
//! a line in the voice panel.
//!
//! `set-voice-status` takes the connection's start time and the host draws
//! the live elapsed time, so there's no one-second timer and no clock in the
//! plugin.

use rekindle_sdk::prelude::*;

struct CallTimer;

fn update() {
    let r = match state::my_voice() {
        Some(v) => {
            let where_ = state::channel(v.channel)
                .map(|c| format!("{} · ", c.name))
                .unwrap_or_default();
            slots::set_voice_status("timer", &format!("{where_}In call"), Some(v.since_ms))
        }
        None => slots::set_voice_status("timer", "", None),
    };
    if let Err(e) = r {
        debug!("voice panel status unavailable: {e:?}");
    }
}

impl Plugin for CallTimer {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        events::subscribe(&[EventKind::MyVoice]);
        update();
        Ok(CallTimer)
    }

    fn on_event(&mut self, event: Event) {
        if matches!(event, Event::MyVoice(_) | Event::MyVoiceLeft) {
            update();
        }
    }
}

rekindle_sdk::plugin!(CallTimer);
