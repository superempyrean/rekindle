use rekindle_sdk::prelude::*;

// Every host you contact must be listed under `network` in Cargo.toml; the
// plugin can reach those and no others.
const URL: &str = "https://httpbin.org/uuid";

struct Fetcher {
    last: String,
}

impl Fetcher {
    fn show(&self) {
        Panel::new("main", "{{TITLE}}")
            .text(&format!("Last response: {}", self.last))
            .button("fetch", "Fetch")
            .show();
    }
}

impl Plugin for Fetcher {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let f = Fetcher { last: "(nothing yet)".into() };
        f.show();
        Ok(f)
    }

    fn on_ui(&mut self, ev: UiEvent) {
        if ev.clicked("main", "fetch") {
            self.last = match http::get(URL) {
                Ok(r) if r.ok() => r.text(),
                Ok(r) => format!("HTTP {}", r.status()),
                Err(e) => format!("error: {e:?}"),
            };
            self.show();
        }
    }
}

rekindle_sdk::plugin!(Fetcher);
