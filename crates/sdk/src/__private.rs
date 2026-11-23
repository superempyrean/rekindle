//! Glue the `plugin!` macro expands to. Not public API.

use std::any::Any;
use std::cell::{Cell, RefCell};

pub use rekindle_plugin_api::exports::rekindle::plugin::compose_handler::Guest as ComposeHandler;
pub use rekindle_plugin_api::exports::rekindle::plugin::event_filter::Guest as EventFilter;
pub use rekindle_plugin_api::exports::rekindle::plugin::event_handler::Guest as EventHandler;
pub use rekindle_plugin_api::exports::rekindle::plugin::http_filter::Guest as HttpFilter;
pub use rekindle_plugin_api::exports::rekindle::plugin::lifecycle::Guest as Lifecycle;
pub use rekindle_plugin_api::exports::rekindle::plugin::notification_filter::Guest as NotificationFilter;
pub use rekindle_plugin_api::exports::rekindle::plugin::renderer::Guest as Renderer;
pub use rekindle_plugin_api::exports::rekindle::plugin::timer_handler::Guest as TimerHandler;
pub use rekindle_plugin_api::exports::rekindle::plugin::ui_handler::Guest as UiHandler;

use crate::render::{RenderInput, Transform};
use crate::{Capability, Granted, Plugin};

thread_local! {
    // Wasm guests are single-threaded and the host never re-enters a plugin
    // while it is running, so a RefCell is enough.
    static STATE: RefCell<Option<Box<dyn Any>>> = const { RefCell::new(None) };
    static HOOKED: Cell<bool> = const { Cell::new(false) };
}

pub fn activate<P: Plugin>(granted: Vec<Capability>) -> Result<(), String> {
    install_panic_hook();
    let plugin = P::activate(&Granted(granted))?;
    STATE.with(|s| *s.borrow_mut() = Some(Box::new(plugin)));
    Ok(())
}

pub fn deactivate<P: Plugin>() {
    with::<P>(Plugin::deactivate);
    STATE.with(|s| *s.borrow_mut() = None);
}

/// Runs `f` on the active plugin; does nothing before `activate`.
pub fn with<P: Plugin>(f: impl FnOnce(&mut P)) {
    with_ret(f);
}

/// Runs `f` on the active plugin and returns its value, or `None` before
/// `activate`.
pub fn with_ret<P: Plugin, R>(f: impl FnOnce(&mut P) -> R) -> Option<R> {
    STATE.with(|s| {
        s.borrow_mut()
            .as_mut()
            .and_then(|b| b.downcast_mut::<P>())
            .map(f)
    })
}

/// The render isolate never activates; this sets up what `activate` would
/// have.
pub fn isolate_init() {
    install_panic_hook();
}

/// One transform list per input, in order. The host discards a batch whose
/// length doesn't match.
pub fn render<P: Plugin>(batch: &[RenderInput]) -> Vec<Vec<Transform>> {
    isolate_init();
    batch.iter().map(P::render).collect()
}

fn install_panic_hook() {
    if HOOKED.with(|h| h.replace(true)) {
        return;
    }
    std::panic::set_hook(Box::new(|info| {
        crate::log(crate::LogLevel::Error, &format!("plugin panicked: {info}"));
    }));
}
