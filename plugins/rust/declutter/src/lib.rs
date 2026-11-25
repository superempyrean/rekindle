//! Declutter: hides the client's elements the `hide` setting names, through
//! `elements.hide`.
//!
//! The plugin's settings page lists the names this build has. A name the
//! build doesn't have is skipped and logged. A changed list brings back what
//! is no longer on it.

use rekindle_sdk::elements;
use rekindle_sdk::prelude::*;

#[derive(Default)]
struct Declutter {
    /// The names hidden now.
    hidden: Vec<String>,
}

impl Declutter {
    fn apply(&mut self) {
        let wanted: Vec<String> = settings::items("hide")
            .iter()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
        for name in self.hidden.iter().filter(|n| !wanted.contains(n)) {
            elements::reset(name);
        }
        self.hidden.clear();
        for name in wanted {
            match elements::hide(&name) {
                Ok(()) => self.hidden.push(name),
                Err(e) => warn!("couldn't hide `{name}`: {e:?}"),
            }
        }
        let names = elements::names();
        let listed = if names.is_empty() {
            "This build names none of its elements.".to_string()
        } else {
            names.join(", ")
        };
        ui::set_settings_extra(&Vec::new().heading("Names on this build").text(&listed));
    }
}

impl Plugin for Declutter {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        let mut d = Declutter::default();
        d.apply();
        Ok(d)
    }

    fn on_settings_changed(&mut self, key: &str) {
        if key == "hide" {
            self.apply();
        }
    }
}

rekindle_sdk::plugin!(Declutter);
