//! Compact Mode: the smallest useful plugin.
//!
//! One palette command, "Toggle compact mode", switches the client between
//! compact and default density, and the choice is remembered across
//! restarts. It needs only `ui`, `storage` and `theming`. The same plugin
//! exists in TypeScript under `plugins/ts/compact-mode`.

use rekindle_sdk::prelude::*;
use rekindle_sdk::theme::Density;

fn apply(compact: bool) {
    let density = if compact {
        Density::Compact
    } else {
        Density::Default
    };
    if let Err(e) = theme::set_density(density) {
        warn!("couldn't set the density: {e:?}");
    }
}

struct CompactMode;

impl Plugin for CompactMode {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        apply(storage::get_or("compact", false));
        ui::register_command("toggle", "Toggle compact mode");
        Ok(CompactMode)
    }

    fn on_command(&mut self, id: &str) {
        if id != "toggle" {
            return;
        }
        let compact = !storage::get_or("compact", false);
        if let Err(e) = storage::set("compact", compact) {
            warn!("couldn't save the setting: {e:?}");
        }
        apply(compact);
        ui::toast(if compact {
            "Compact mode on"
        } else {
            "Compact mode off"
        });
    }
}

rekindle_sdk::plugin!(CompactMode);
