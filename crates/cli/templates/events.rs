use rekindle_sdk::prelude::*;

#[derive(Default)]
struct Watcher {
    switches: u32,
    last: Option<String>,
}

impl Watcher {
    fn show(&self) {
        let last = self.last.as_deref().unwrap_or("(none yet)");
        Panel::new("status", "{{TITLE}}")
            .text(&format!("Channel switches this session: {}", self.switches))
            .text(&format!("Current channel: {last}"))
            .show();
    }
}

impl Plugin for Watcher {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        // Only these kinds; without this you get every kind you're granted.
        events::subscribe(&[EventKind::SelectionChanged, EventKind::MyPresence]);
        let w = Watcher::default();
        w.show();
        Ok(w)
    }

    fn on_event(&mut self, event: Event) {
        match event {
            Event::SelectionChanged(sel) => {
                self.switches += 1;
                self.last = sel.channel.and_then(state::channel).map(|ch| format!("#{}", ch.name));
                self.show();
            }
            Event::MyPresence(status) => info!("presence is now {status}"),
            // Events are added as the contract grows; always keep a catch-all.
            _ => {}
        }
    }
}

rekindle_sdk::plugin!(Watcher);
