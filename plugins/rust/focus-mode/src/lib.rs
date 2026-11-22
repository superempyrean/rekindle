//! Focus Mode: hides the member list and gives the user area a background
//! of its own, through `theming.set-surface`.
//!
//! It starts on. The palette command "Toggle focus mode" turns it off and
//! on again, and the choice is remembered across restarts. A build that
//! can't restyle a surface leaves it as it is, and the plugin logs which.

use rekindle_sdk::prelude::*;
use rekindle_sdk::theme::SurfaceStyle;

/// The user area's background while focus mode is on.
const USER_AREA: Color = Color::rgb(0x2a, 0x1f, 0x3d);

fn apply(on: bool) {
    if !on {
        theme::reset_surface("member-list");
        theme::reset_surface("user-area");
        return;
    }
    if let Err(e) = theme::hide_surface("member-list") {
        warn!("couldn't hide the member list: {e:?}");
    }
    let style = SurfaceStyle {
        hidden: false,
        opacity: None,
        radius: None,
        background: Some(USER_AREA),
    };
    if let Err(e) = theme::set_surface("user-area", style) {
        warn!("couldn't restyle the user area: {e:?}");
    }
}

struct FocusMode;

impl Plugin for FocusMode {
    fn activate(_granted: &Granted) -> Result<Self, String> {
        apply(storage::get_or("on", true));
        ui::register_command("toggle", "Toggle focus mode");
        Ok(FocusMode)
    }

    fn on_command(&mut self, id: &str) {
        if id != "toggle" {
            return;
        }
        let on = !storage::get_or("on", true);
        if let Err(e) = storage::set("on", on) {
            warn!("couldn't save the setting: {e:?}");
        }
        apply(on);
        ui::toast(if on {
            "Focus mode on"
        } else {
            "Focus mode off"
        });
    }
}

rekindle_sdk::plugin!(FocusMode);
