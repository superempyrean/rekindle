use rekindle_sdk::prelude::*;

struct MyPlugin;

impl Plugin for MyPlugin {
    fn activate(granted: &Granted) -> Result<Self, String> {
        info!("{{TITLE}} activated with {:?}", granted.all());
        Ok(MyPlugin)
    }
}

rekindle_sdk::plugin!(MyPlugin);
