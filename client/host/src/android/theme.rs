//! The plugins' theme, bound to the palette the guest names.
//!
//! `palette.json` in the config dir is the build's
//! [`ThemeBinding`]: the host call that carries the guest's palette, and
//! where each token's colour is in it. [`on_call`] reads each palette the
//! guest names off the emit hook, at start and whenever the user's theme
//! changes. [`recolors`] makes the plugins' overrides into recolours for
//! the palette last named, so the engine follows the theme the client
//! shows from the turn after the call.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use rekindle_wire_inject::theme::{Bound, Palette, ThemeBinding, reach};
use rekindle_wire_inject::{Engine, Recolor};

use super::log;

/// The build's binding, read once before the emit slot is swapped.
static BINDING: OnceLock<Option<ThemeBinding>> = OnceLock::new();
/// The palette the guest named last.
static PALETTE: Mutex<Option<Palette>> = Mutex::new(None);
/// How many palettes the guest has named.
static NAMED: AtomicU64 = AtomicU64::new(0);
/// The plugins' overrides, token to `0xRRGGBBAA`, as the runtime last
/// read them.
static OVERRIDES: Mutex<BTreeMap<String, u32>> = Mutex::new(BTreeMap::new());

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Read the binding from `palette.json`. Logs and keeps `None` when the
/// file is missing or isn't a binding.
pub(crate) fn load() -> Option<&'static ThemeBinding> {
    BINDING
        .get_or_init(|| {
            let path = super::config_file(super::runtime::PALETTE);
            let json = std::fs::read_to_string(&path).ok()?;
            match ThemeBinding::from_json(&json) {
                Ok(b) => {
                    log(&format!(
                        "theme: binding of {} tokens, palette call {}",
                        b.tokens.len(),
                        b.method
                    ));
                    Some(b)
                }
                Err(e) => {
                    log(&format!("theme: {}: {e}", path.display()));
                    None
                }
            }
        })
        .as_ref()
}

pub(crate) fn binding() -> Option<&'static ThemeBinding> {
    BINDING.get().and_then(Option::as_ref)
}

/// Read a host call: true if it named a palette, which is then the one
/// [`recolors`] and [`values`] use. On the guest's thread.
pub(crate) fn on_call(buf: &[u8]) -> bool {
    let Some(palette) = binding().and_then(|b| b.palette(buf)) else {
        return false;
    };
    let n = NAMED.fetch_add(1, Ordering::SeqCst) + 1;
    let colors: Vec<String> = palette
        .colors
        .iter()
        .map(|(k, c)| format!("{}={c:08x}", k.0))
        .collect();
    log(&format!(
        "theme: palette #{n} dark={:?} theme={} turn={} {}",
        palette.dark,
        binding()
            .and_then(|b| b.theme_of(&palette))
            .unwrap_or("unnamed"),
        super::turns(),
        colors.join(" ")
    ));
    *lock(&PALETTE) = Some(palette);
    true
}

/// How many palettes the guest has named.
pub(crate) fn named() -> u64 {
    NAMED.load(Ordering::SeqCst)
}

/// Each token's colour in the palette last named.
pub(crate) fn values() -> Vec<Bound> {
    let Some(b) = binding() else {
        return Vec::new();
    };
    b.values(lock(&PALETTE).as_ref())
}

/// The theme the palette last named is, by the binding's name for it, or
/// `dark` or `light` for one the binding doesn't name.
pub(crate) fn shown() -> String {
    let palette = lock(&PALETTE);
    let Some(p) = palette.as_ref() else {
        return "none".into();
    };
    match binding().and_then(|b| b.theme_of(p)) {
        Some(name) => name.to_string(),
        None if p.is_dark() => "dark".into(),
        None => "light".into(),
    }
}

/// Set the plugins' overrides. True if they changed.
pub(crate) fn set_overrides(overrides: BTreeMap<String, u32>) -> bool {
    let mut o = lock(&OVERRIDES);
    if *o == overrides {
        return false;
    }
    *o = overrides;
    true
}

/// The recolours for the plugins' overrides in the palette last named.
pub(crate) fn recolors() -> Vec<Recolor> {
    let Some(b) = binding() else {
        return Vec::new();
    };
    let palette = lock(&PALETTE).clone();
    b.recolors(palette.as_ref(), &lock(&OVERRIDES))
}

/// One line on what the recolours reach in the host's tree.
pub(crate) fn report(engine: &Engine) -> String {
    format!(
        "theme: {} recolors={} {}",
        shown(),
        engine.work().recolors.len(),
        reach(engine, &engine.work().recolors, &values())
    )
}
