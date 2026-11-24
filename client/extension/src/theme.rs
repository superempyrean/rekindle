//! Theme colours by recovered offset: [`ThemeState`] holds the plugins'
//! overrides, and [`install`] detours the client's theme function to apply
//! them.
//!
//! The detour uses the raw sret ABI. It calls the client's function, records
//! the colours that produced (once, for reads), and writes each overridden
//! token's colour over the returned value at the offset a [`ThemeProfile`]
//! gives. [`ThemeState`] keeps a stack of overrides per token, and the last
//! one applies. For now, only colour tokens are applied.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, bail};
use rekindle_hook::Detour as _;
use rekindle_plugin_host::model::{Color, ThemeValue};
use rekindle_plugin_host::tokens::{ThemeError, TokenInfo};

/// The size of gpui's `Hsla`: four `f32`s.
const HSLA: usize = 16;

/// The client's theme function and where its colour tokens sit in the value
/// it returns.
#[derive(Clone)]
pub struct ThemeProfile {
    /// The theme function's address, when its anchor resolved. Without it,
    /// theming goes through framework recolouring only.
    pub addr: Option<usize>,
    /// Size of the returned value.
    pub return_size: usize,
    /// Token name → byte offset of its colour within the returned value.
    pub offsets: BTreeMap<String, usize>,
}

/// Overridden colours by offset: what the detour writes.
static OVERRIDES: Mutex<BTreeMap<usize, [u8; HSLA]>> = Mutex::new(BTreeMap::new());
/// The client's own colours by offset, as the detour first read them or
/// [`ThemeState::seed_base`] set them.
static BASE: Mutex<BTreeMap<usize, [u8; HSLA]>> = Mutex::new(BTreeMap::new());
/// Every token's offset, for the detour's first read.
static ALL_OFFSETS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
/// The client's original theme function.
static TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);

fn bytes_to_hsla(b: [u8; HSLA]) -> [f32; 4] {
    let f = |i: usize| f32::from_ne_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    [f(0), f(4), f(8), f(12)]
}

fn hsla_to_bytes(v: [f32; 4]) -> [u8; HSLA] {
    let mut b = [0u8; HSLA];
    for (i, x) in v.iter().enumerate() {
        b[i * 4..i * 4 + 4].copy_from_slice(&x.to_ne_bytes());
    }
    b
}

fn color_to_bytes(c: Color) -> [u8; HSLA] {
    let rgba = rekindle_native::Rgba {
        r: f32::from(c.r) / 255.0,
        g: f32::from(c.g) / 255.0,
        b: f32::from(c.b) / 255.0,
        a: f32::from(c.a) / 255.0,
    };
    hsla_to_bytes(rgba.to_hsla())
}

fn bytes_to_color(b: [u8; HSLA]) -> Color {
    let rgb = rekindle_native::Rgba::from_hsla(bytes_to_hsla(b));
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color {
        r: q(rgb.r),
        g: q(rgb.g),
        b: q(rgb.b),
        a: q(rgb.a),
    }
}

/// The plugins' colour overrides, layered over the client's own colours.
pub struct ThemeState {
    profile: ThemeProfile,
    /// token -> stack of (plugin, colour bytes); the last entry is effective.
    stacks: BTreeMap<String, Vec<(String, [u8; HSLA])>>,
    /// The tokens that aren't colours (lengths, fonts, density), with their
    /// defaults, when the host puts their values on the client's wire.
    others: BTreeMap<String, (TokenInfo, ThemeValue)>,
    /// token -> stack of (plugin, value) for those tokens.
    values: BTreeMap<String, Vec<(String, ThemeValue)>>,
}

impl ThemeState {
    pub fn new(profile: ThemeProfile) -> Self {
        *ALL_OFFSETS.lock().unwrap() = profile.offsets.values().copied().collect();
        Self {
            profile,
            stacks: BTreeMap::new(),
            others: BTreeMap::new(),
            values: BTreeMap::new(),
        }
    }

    /// Take the tokens that aren't colours too, for a host that reads them
    /// back with [`ThemeState::values`].
    pub fn take_values(&mut self) {
        self.others = rekindle_plugin_host::tokens::default_tokens()
            .into_iter()
            .filter(|(_, v)| !matches!(v, ThemeValue::Color(_)))
            .map(|(i, v)| (i.name.clone(), (i, v)))
            .collect();
    }

    /// The value in effect for each token that isn't a colour and a plugin
    /// set.
    pub fn values(&self) -> Vec<(String, ThemeValue)> {
        self.values
            .iter()
            .filter_map(|(t, s)| s.last().map(|(_, v)| (t.clone(), v.clone())))
            .collect()
    }

    fn recompute(&self) {
        let mut map = BTreeMap::new();
        for (token, stack) in &self.stacks {
            if let (Some(&offset), Some((_, color))) =
                (self.profile.offsets.get(token), stack.last())
            {
                map.insert(offset, *color);
            }
        }
        *OVERRIDES.lock().unwrap() = map;
    }

    pub fn token_names(&self) -> Vec<String> {
        self.profile
            .offsets
            .keys()
            .chain(self.others.keys())
            .cloned()
            .collect()
    }

    pub fn set_value(
        &mut self,
        plugin: &str,
        token: &str,
        value: &ThemeValue,
    ) -> Result<(), ThemeError> {
        let ThemeValue::Color(c) = *value else {
            let (info, _) = self.others.get(token).ok_or(ThemeError::WrongKind)?;
            rekindle_plugin_host::tokens::validate(info, value)?;
            let stack = self.values.entry(token.to_string()).or_default();
            stack.retain(|(p, _)| p != plugin);
            stack.push((plugin.to_string(), value.clone()));
            return Ok(());
        };
        if !self.profile.offsets.contains_key(token) {
            return Err(ThemeError::UnknownToken);
        }
        let stack = self.stacks.entry(token.to_string()).or_default();
        stack.retain(|(p, _)| p != plugin);
        stack.push((plugin.to_string(), color_to_bytes(c)));
        self.recompute();
        Ok(())
    }

    pub fn get_value(&self, token: &str) -> Option<ThemeValue> {
        if let Some((_, default)) = self.others.get(token) {
            let set = self.values.get(token).and_then(|s| s.last());
            return Some(set.map_or(default, |(_, v)| v).clone());
        }
        if let Some((_, color)) = self.stacks.get(token).and_then(|s| s.last()) {
            return Some(ThemeValue::Color(bytes_to_color(*color)));
        }
        let offset = *self.profile.offsets.get(token)?;
        let base = *BASE.lock().unwrap().get(&offset)?;
        Some(ThemeValue::Color(bytes_to_color(base)))
    }

    pub fn reset_value(&mut self, plugin: &str, token: &str) {
        if let Some(stack) = self.values.get_mut(token) {
            stack.retain(|(p, _)| p != plugin);
            if stack.is_empty() {
                self.values.remove(token);
            }
        }
        if let Some(stack) = self.stacks.get_mut(token) {
            stack.retain(|(p, _)| p != plugin);
            if stack.is_empty() {
                self.stacks.remove(token);
            }
            self.recompute();
        }
    }

    /// The effective override per token, as HSLA floats (h, s, l, a).
    pub fn overrides(&self) -> BTreeMap<String, [f32; 4]> {
        self.stacks
            .iter()
            .filter_map(|(t, s)| s.last().map(|(_, b)| (t.clone(), bytes_to_hsla(*b))))
            .collect()
    }

    /// Seed the client's own value of a token (normally captured by the theme
    /// detour), so reads still work when theming goes through the framework.
    pub fn seed_base(&self, token: &str, hsla: [f32; 4]) {
        if let Some(&off) = self.profile.offsets.get(token) {
            BASE.lock().unwrap().insert(off, hsla_to_bytes(hsla));
        }
    }

    pub fn override_owner(&self, token: &str) -> Option<String> {
        self.stacks
            .get(token)
            .and_then(|s| s.last())
            .map(|(p, _)| p.clone())
    }

    pub fn plugin_unloaded(&mut self, plugin: &str) {
        for stack in self.stacks.values_mut() {
            stack.retain(|(p, _)| p != plugin);
        }
        self.stacks.retain(|_, s| !s.is_empty());
        self.recompute();
    }
}

/// An installed theme detour. Dropping it restores the client's function and
/// forgets the colours it recorded.
pub struct ThemeHook {
    _detour: rekindle_hook::Installed,
}

impl Drop for ThemeHook {
    fn drop(&mut self) {
        BASE.lock().unwrap().clear();
        OVERRIDES.lock().unwrap().clear();
    }
}

/// Detour the theme function at `addr`, which returns a value of
/// `profile.return_size` bytes.
///
/// # Safety
///
/// `addr` is the entry of the client's theme function, an
/// `extern "C" fn(ret: *mut u8, this: *const u8) -> *mut u8` that fills a
/// value of `profile.return_size` bytes at `ret`, and every offset in
/// `profile.offsets` is at least [`HSLA`] bytes inside that value.
pub unsafe fn install(addr: usize, profile: &ThemeProfile) -> Result<ThemeHook> {
    if profile.return_size == 0 {
        bail!("recovered return size is zero");
    }
    if let Some(off) = profile
        .offsets
        .values()
        .find(|&&off| off + HSLA > profile.return_size)
    {
        bail!(
            "a colour at offset {off} is outside the {}-byte theme",
            profile.return_size
        );
    }
    // SAFETY: the caller guarantees `addr` is the theme function, whose ABI
    // `replacement` shares.
    let detour =
        unsafe { rekindle_hook::InlineDetour.install(addr, replacement as *const () as usize)? };
    TRAMPOLINE.store(detour.trampoline(), Ordering::SeqCst);
    Ok(ThemeHook { _detour: detour })
}

/// The client's theme function with the overrides written over its result.
///
/// # Safety
///
/// Called only by the client in place of its theme function, with that
/// function's arguments, after [`install`] stored the trampoline.
unsafe extern "C" fn replacement(ret: *mut u8, this: *const u8) -> *mut u8 {
    // SAFETY: TRAMPOLINE holds the original function, stored by `install`
    // before the detour went live.
    let original = unsafe {
        std::mem::transmute::<usize, unsafe extern "C" fn(*mut u8, *const u8) -> *mut u8>(
            TRAMPOLINE.load(Ordering::SeqCst),
        )
    };
    // SAFETY: the client's own arguments, passed on unchanged.
    unsafe { original(ret, this) };
    let mut base = BASE.lock().unwrap();
    if base.is_empty() {
        for &offset in ALL_OFFSETS.lock().unwrap().iter() {
            let mut bytes = [0u8; HSLA];
            // SAFETY: `install` checked every offset lies HSLA bytes inside
            // the returned value, which `original` just filled.
            unsafe { std::ptr::copy_nonoverlapping(ret.add(offset), bytes.as_mut_ptr(), HSLA) };
            base.insert(offset, bytes);
        }
    }
    drop(base);
    for (&offset, color) in OVERRIDES.lock().unwrap().iter() {
        // SAFETY: as above; OVERRIDES only holds offsets from the profile.
        unsafe { std::ptr::copy_nonoverlapping(color.as_ptr(), ret.add(offset), HSLA) };
    }
    ret
}
