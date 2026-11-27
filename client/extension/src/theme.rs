//! Theme colours by recovered offset: [`ThemeState`] holds the plugins'
//! overrides, and [`install`] detours the client's theme function to apply
//! them.
//!
//! The detour uses the raw sret ABI. It calls the client's function, records
//! the colours that produced (once, for reads), and writes each overridden
//! token's colour over the returned value at the offset a [`ThemeProfile`]
//! gives. [`ThemeState`] keeps a stack of overrides per token, and the last
//! one applies. For now, only colour tokens are applied.
//!
//! A contract token the client draws under tokens of its own
//! (`rekindle_plugin_host::tokens::DRAWN_AS`) can be set whenever the client
//! has one of them, and its colour applies to each of them a plugin didn't
//! set. Reads of those tokens give the colour that applies.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Result, bail};
use rekindle_hook::Detour as _;
use rekindle_plugin_host::model::{Color, ThemeValue};
use rekindle_plugin_host::tokens::{DRAWN_AS, OWN, ThemeError, TokenInfo};

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
        let map = self
            .profile
            .offsets
            .iter()
            .filter_map(|(token, &offset)| Some((offset, self.effective(token)?)))
            .collect();
        *OVERRIDES.lock().unwrap() = map;
    }

    /// The colour that applies to `token`: the last plugin's that set it, or
    /// else that of a contract token the client draws it for.
    fn effective(&self, token: &str) -> Option<[u8; HSLA]> {
        let own = |t: &str| self.stacks.get(t).and_then(|s| s.last()).map(|(_, c)| *c);
        own(token).or_else(|| {
            DRAWN_AS
                .iter()
                .filter(|(_, tos)| tos.contains(&token))
                .find_map(|(from, _)| own(from))
        })
    }

    /// Whether plugins can set `token` on this client: one of its own, a
    /// contract token it draws under one of its own, or one rekindle's own
    /// surfaces draw with.
    fn colour_token(&self, token: &str) -> bool {
        self.profile.offsets.contains_key(token)
            || OWN.contains(&token)
            || DRAWN_AS.iter().any(|(from, tos)| {
                *from == token && tos.iter().any(|t| self.profile.offsets.contains_key(*t))
            })
    }

    pub fn token_names(&self) -> Vec<String> {
        let drawn_as = DRAWN_AS
            .iter()
            .map(|(from, _)| *from)
            .chain(OWN.iter().copied())
            .filter(|t| !self.profile.offsets.contains_key(*t) && self.colour_token(t))
            .map(str::to_string);
        self.profile
            .offsets
            .keys()
            .chain(self.others.keys())
            .cloned()
            .chain(drawn_as)
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
        if !self.colour_token(token) {
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
        if let Some(color) = self.effective(token) {
            return Some(ThemeValue::Color(bytes_to_color(color)));
        }
        // A contract token the client has no colour of its own for reads
        // as the first of the client's tokens it draws under.
        let drawn = DRAWN_AS
            .iter()
            .find(|(from, _)| *from == token)
            .map_or(&[][..], |(_, tos)| *tos);
        let base = BASE.lock().unwrap();
        std::iter::once(token)
            .chain(drawn.iter().copied())
            .find_map(|t| base.get(self.profile.offsets.get(t)?).copied())
            .map(|b| ThemeValue::Color(bytes_to_color(b)))
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

    /// The effective override per token, as HSLA floats (h, s, l, a): each
    /// token a plugin set, and each client token a contract token a plugin
    /// set is drawn under.
    pub fn overrides(&self) -> BTreeMap<String, [f32; 4]> {
        let drawn = DRAWN_AS.iter().flat_map(|(_, tos)| tos.iter().copied());
        self.stacks
            .keys()
            .map(String::as_str)
            .chain(drawn)
            .filter_map(|t| Some((t.to_string(), bytes_to_hsla(self.effective(t)?))))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn state(tokens: &[&str]) -> ThemeState {
        ThemeState::new(ThemeProfile {
            addr: None,
            return_size: tokens.len() * HSLA,
            offsets: tokens
                .iter()
                .enumerate()
                .map(|(i, t)| ((*t).to_string(), i * HSLA))
                .collect(),
        })
    }

    fn colour(v: &ThemeValue) -> Color {
        match v {
            ThemeValue::Color(c) => *c,
            other => panic!("not a colour: {other:?}"),
        }
    }

    #[test]
    fn a_contract_token_applies_to_the_clients_tokens_it_is_drawn_as() {
        let mut s = state(&["chat-background-default", "text-default"]);
        let red = ThemeValue::Color(Color::hex(0x13161d));
        s.set_value("p", "channeltextarea-background", &red)
            .unwrap();
        assert_eq!(
            s.get_value("chat-background-default").map(|v| colour(&v)),
            Some(Color::hex(0x13161d))
        );
        assert!(s.overrides().contains_key("chat-background-default"));
        // A plugin that sets the client's token wins.
        let green = ThemeValue::Color(Color::hex(0x00ff00));
        s.set_value("q", "chat-background-default", &green).unwrap();
        assert_eq!(
            s.get_value("chat-background-default").map(|v| colour(&v)),
            Some(Color::hex(0x00ff00))
        );
        assert!(
            s.token_names()
                .contains(&"channeltextarea-background".to_string())
        );
    }

    #[test]
    fn rekindles_own_tokens_are_taken_on_any_client() {
        let mut s = state(&["text-default"]);
        let fill = ThemeValue::Color(Color::hex(0x2a1f3d));
        s.set_value("p", "toast-background", &fill).unwrap();
        assert!(s.overrides().contains_key("toast-background"));
        assert!(s.token_names().contains(&"toast-background".to_string()));
    }

    #[test]
    fn a_contract_token_without_a_client_token_is_refused() {
        let mut s = state(&["text-default"]);
        let red = ThemeValue::Color(Color::hex(0xff0000));
        assert_eq!(
            s.set_value("p", "channeltextarea-background", &red),
            Err(ThemeError::UnknownToken)
        );
        assert!(
            !s.token_names()
                .contains(&"channeltextarea-background".to_string())
        );
    }
}
