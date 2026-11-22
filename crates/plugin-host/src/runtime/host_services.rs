//! `theming`, `overlay`, `audio`, `notify`, `http`, `clipboard`, `files`,
//! `tweaks`.

use crate::bindings::wit;
use crate::capability::Capability;
use crate::model::{self, UiError, UiOp};
use crate::net;

use super::convert::{
    self, color_from_wit, color_to_wit, theme_value_from_wit, theme_value_to_wit,
};
use super::state::{Budget, PluginState, clean, clean_multiline};

/// `x` clamped to `lo..=hi`, or `fallback` if it isn't finite.
fn clamp_or(x: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if x.is_finite() {
        x.clamp(lo, hi)
    } else {
        fallback
    }
}

/// Read an image's `(width, height)` from its header without decoding pixels.
/// Accepts PNG (IHDR) and GIF (logical screen descriptor), enough to size-check
/// a dropped-in sprite atlas of either kind. `None` if it's neither.
pub(super) fn image_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    const PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() >= 24 && bytes[..8] == PNG && &bytes[12..16] == b"IHDR" {
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return (w > 0 && h > 0).then_some((w, h));
    }
    if bytes.len() >= 10 && &bytes[0..3] == b"GIF" {
        let w = u32::from(u16::from_le_bytes([bytes[6], bytes[7]]));
        let h = u32::from(u16::from_le_bytes([bytes[8], bytes[9]]));
        return (w > 0 && h > 0).then_some((w, h));
    }
    None
}

/// Fonts a plugin may bundle: TTF/OTF/TTC, up to 8 MiB.
const MAX_FONT_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn check_font(family: &str, data: &[u8]) -> Result<(), String> {
    if family.trim().is_empty() || family.len() > 64 || family.chars().any(char::is_control) {
        return Err("font family must be 1-64 printable characters".into());
    }
    if data.len() > MAX_FONT_BYTES {
        return Err(format!(
            "font is {} bytes; the limit is {MAX_FONT_BYTES}",
            data.len()
        ));
    }
    let magic = data.get(..4).unwrap_or_default();
    if !matches!(magic, [0, 1, 0, 0] | b"OTTO" | b"true" | b"ttcf") {
        return Err("not a TrueType/OpenType font".into());
    }
    Ok(())
}

impl wit::theming::Host for PluginState {
    fn tokens(&mut self) -> wasmtime::Result<Vec<wit::theming::TokenInfo>> {
        use crate::tokens::TokenKind as K;
        use wit::theming::TokenKind as W;
        self.require(Capability::Theming)?;
        Ok(self
            .backend
            .lock()
            .unwrap()
            .theme_tokens()
            .into_iter()
            .map(|t| wit::theming::TokenInfo {
                kind: match t.kind {
                    K::Color => W::Color,
                    K::Length => W::Length,
                    K::Font => W::Font,
                    K::Choice => W::Choice,
                },
                min: t.range.map(|r| r.0),
                max: t.range.map(|r| r.1),
                name: t.name,
                group: t.group,
                aliases: t.aliases,
                choices: t.choices,
            })
            .collect())
    }

    fn get(&mut self, token: String) -> wasmtime::Result<Option<wit::theming::ThemeValue>> {
        self.require(Capability::Theming)?;
        let token = crate::tokens::canonical(&token);
        Ok(self
            .backend
            .lock()
            .unwrap()
            .get_value(token)
            .map(theme_value_to_wit))
    }

    fn set(
        &mut self,
        token: String,
        value: wit::theming::ThemeValue,
    ) -> wasmtime::Result<Result<(), wit::theming::ThemeError>> {
        use crate::tokens::ThemeError as E;
        use wit::theming::ThemeError as W;
        self.require(Capability::Theming)?;
        self.main_only("theming.set")?;
        let token = crate::tokens::canonical(&token);
        Ok(self
            .backend
            .lock()
            .unwrap()
            .set_value(&self.id, token, theme_value_from_wit(value))
            .map_err(|e| match e {
                E::UnknownToken => W::UnknownToken,
                E::WrongKind => W::WrongKind,
                E::OutOfRange => W::OutOfRange,
            }))
    }

    fn reset(&mut self, token: String) -> wasmtime::Result<()> {
        self.require(Capability::Theming)?;
        self.main_only("theming.reset")?;
        let token = crate::tokens::canonical(&token);
        self.backend.lock().unwrap().reset_value(&self.id, token);
        Ok(())
    }

    fn get_color(&mut self, token: String) -> wasmtime::Result<Option<wit::types::Color>> {
        self.require(Capability::Theming)?;
        let token = crate::tokens::canonical(&token);
        Ok(self
            .backend
            .lock()
            .unwrap()
            .get_color(token)
            .map(color_to_wit))
    }

    fn set_color(&mut self, token: String, value: wit::types::Color) -> wasmtime::Result<bool> {
        self.require(Capability::Theming)?;
        self.main_only("theming.set-color")?;
        let token = crate::tokens::canonical(&token);
        let v = model::ThemeValue::Color(color_from_wit(value));
        Ok(self
            .backend
            .lock()
            .unwrap()
            .set_value(&self.id, token, v)
            .is_ok())
    }

    fn set_tint(&mut self, tint: Option<wit::theming::Tint>) -> wasmtime::Result<()> {
        self.require(Capability::Theming)?;
        self.main_only("theming.set-tint")?;
        let tint = tint.map(|t| model::Tint {
            color: color_from_wit(t.color),
            amount: clamp_or(t.amount, 0.0, 1.0, 0.0),
            text_amount: clamp_or(t.text_amount, 0.0, 1.0, 0.0),
        });
        self.backend.lock().unwrap().set_tint(&self.id, tint);
        Ok(())
    }

    fn register_font(
        &mut self,
        family: String,
        data: Vec<u8>,
    ) -> wasmtime::Result<Result<(), String>> {
        self.require(Capability::Theming)?;
        self.main_only("theming.register-font")?;
        if let Err(e) = check_font(&family, &data) {
            return Ok(Err(e));
        }
        Ok(self
            .backend
            .lock()
            .unwrap()
            .register_font(&self.id, &family, data))
    }

    fn surfaces(&mut self) -> wasmtime::Result<Vec<String>> {
        self.require(Capability::Theming)?;
        Ok(self.backend.lock().unwrap().surfaces())
    }

    fn set_surface(
        &mut self,
        surface: String,
        style: wit::theming::SurfaceStyle,
    ) -> wasmtime::Result<Result<(), wit::theming::ThemeError>> {
        self.require(Capability::Theming)?;
        self.main_only("theming.set-surface")?;
        let fin =
            |x: Option<f32>, lo: f32, hi: f32| x.filter(|v| v.is_finite()).map(|v| v.clamp(lo, hi));
        let style = model::SurfaceStyle {
            hidden: style.hidden,
            opacity: fin(style.opacity, 0.0, 1.0),
            radius: fin(style.radius, 0.0, 32.0),
            background: style.background.map(color_from_wit),
        };
        let r = self
            .backend
            .lock()
            .unwrap()
            .set_surface(&self.id, &surface, style);
        Ok(r.map_err(|e| match e {
            UiError::Invalid => wit::theming::ThemeError::OutOfRange,
            _ => wit::theming::ThemeError::UnknownSurface,
        }))
    }

    fn reset_surface(&mut self, surface: String) -> wasmtime::Result<()> {
        self.require(Capability::Theming)?;
        self.main_only("theming.reset-surface")?;
        let _ = self
            .backend
            .lock()
            .unwrap()
            .ui_update(&self.id, UiOp::ResetSurface { surface });
        Ok(())
    }

    fn strings(&mut self) -> wasmtime::Result<Vec<String>> {
        self.require(Capability::Theming)?;
        Ok(self.backend.lock().unwrap().strings())
    }

    fn set_string(
        &mut self,
        key: String,
        text: Option<String>,
    ) -> wasmtime::Result<Result<(), wit::theming::StringError>> {
        use wit::theming::StringError as E;
        self.require(Capability::Theming)?;
        self.main_only("theming.set-string")?;
        if !self.backend.lock().unwrap().strings().contains(&key) {
            return Ok(Err(E::UnknownString));
        }
        // The client lays these out for its own text: one line, not much longer.
        let text = match text {
            Some(t) => match super::state::clean(&t, 120) {
                Some(t) => Some(t),
                None => return Ok(Err(E::Invalid)),
            },
            None => None,
        };
        let r = self
            .backend
            .lock()
            .unwrap()
            .ui_update(&self.id, model::UiOp::SetString { key, text });
        Ok(r.map_err(|_| E::UnknownString))
    }

    fn set_background(&mut self, bg: Option<wit::theming::Background>) -> wasmtime::Result<()> {
        self.require(Capability::Theming)?;
        self.main_only("theming.set-background")?;
        let background = match bg {
            Some(b) if self.image_registered(&b.image) => Some(model::Background {
                image: b.image,
                opacity: clamp_or(b.opacity, 0.0, 1.0, 1.0),
                blur: clamp_or(b.blur, 0.0, 64.0, 0.0),
            }),
            Some(b) => {
                self.warn(format!(
                    "set-background: `{}` isn't a registered image (ui.register-image)",
                    b.image
                ));
                return Ok(());
            }
            None => None,
        };
        let _ = self
            .backend
            .lock()
            .unwrap()
            .ui_update(&self.id, UiOp::SetBackground { background });
        Ok(())
    }
}

/// Overlay limits.
const MAX_OVERLAY_IMAGES: usize = 4;
const MAX_OVERLAY_SPRITES: usize = 16;
const MAX_OVERLAY_PNG_BYTES: usize = 256 * 1024;
const MAX_OVERLAY_DIM: u32 = 512;

impl wit::overlay::Host for PluginState {
    fn register_image(
        &mut self,
        id: String,
        png: Vec<u8>,
    ) -> wasmtime::Result<Result<(), wit::overlay::OverlayError>> {
        use wit::overlay::OverlayError as E;
        self.require(Capability::Overlay)?;
        if id.is_empty() || id.len() > 64 {
            return Ok(Err(E::Invalid));
        }
        if png.len() > MAX_OVERLAY_PNG_BYTES {
            return Ok(Err(E::ImageTooLarge));
        }
        let Some((w, h)) = image_dims(&png) else {
            return Ok(Err(E::DecodeFailed));
        };
        if w > MAX_OVERLAY_DIM || h > MAX_OVERLAY_DIM {
            return Ok(Err(E::ImageTooLarge));
        }
        if !self.overlay_images.contains(&id) && self.overlay_images.len() >= MAX_OVERLAY_IMAGES {
            return Ok(Err(E::TooManyImages));
        }
        self.overlay_images.insert(id.clone());
        self.backend
            .lock()
            .unwrap()
            .overlay_register_image(&self.id, &id, png, w, h);
        Ok(Ok(()))
    }

    fn remove_image(&mut self, id: String) -> wasmtime::Result<()> {
        self.require(Capability::Overlay)?;
        self.overlay_images.remove(&id);
        self.backend
            .lock()
            .unwrap()
            .overlay_remove_image(&self.id, &id);
        Ok(())
    }

    fn draw(
        &mut self,
        s: wit::overlay::Sprite,
    ) -> wasmtime::Result<Result<(), wit::overlay::OverlayError>> {
        use wit::overlay::OverlayError as E;
        self.require(Capability::Overlay)?;
        if s.instance.is_empty() || s.instance.len() > 64 || s.sw == 0 || s.sh == 0 {
            return Ok(Err(E::Invalid));
        }
        if !self.overlay_images.contains(&s.image) {
            return Ok(Err(E::Invalid));
        }
        if !self.overlay_sprites.contains(&s.instance)
            && self.overlay_sprites.len() >= MAX_OVERLAY_SPRITES
        {
            return Ok(Err(E::TooManySprites));
        }
        self.overlay_sprites.insert(s.instance.clone());
        self.backend
            .lock()
            .unwrap()
            .overlay_draw(model::OverlaySprite {
                plugin: self.id.clone(),
                instance: s.instance,
                image: s.image,
                sx: s.sx,
                sy: s.sy,
                sw: s.sw,
                sh: s.sh,
                x: s.x,
                y: s.y,
                w: s.w,
                h: s.h,
            });
        Ok(Ok(()))
    }

    fn clear(&mut self, instance: String) -> wasmtime::Result<()> {
        self.require(Capability::Overlay)?;
        self.overlay_sprites.remove(&instance);
        self.backend
            .lock()
            .unwrap()
            .overlay_clear(&self.id, &instance);
        Ok(())
    }

    fn viewport(&mut self) -> wasmtime::Result<(f32, f32)> {
        self.require(Capability::Overlay)?;
        Ok(self.backend.lock().unwrap().viewport())
    }
}

const MAX_SOUNDS: usize = 16;
const MAX_SOUND_BYTES: usize = 1024 * 1024;

fn is_audio(data: &[u8]) -> bool {
    data.starts_with(b"OggS")
        || (data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE")
}

impl wit::audio::Host for PluginState {
    fn register_sound(
        &mut self,
        id: String,
        data: Vec<u8>,
    ) -> wasmtime::Result<Result<(), wit::audio::AudioError>> {
        use wit::audio::AudioError as E;
        self.require(Capability::Audio)?;
        if !super::state::valid_id(&id) {
            return Ok(Err(E::Invalid));
        }
        if data.len() > MAX_SOUND_BYTES {
            return Ok(Err(E::TooLarge));
        }
        if !is_audio(&data) {
            return Ok(Err(E::DecodeFailed));
        }
        if !self.sounds.contains(&id) && self.sounds.len() >= MAX_SOUNDS {
            return Ok(Err(E::TooMany));
        }
        let r = self
            .backend
            .lock()
            .unwrap()
            .register_sound(&self.id, &id, data);
        Ok(match r {
            Ok(()) => {
                self.sounds.insert(id);
                Ok(())
            }
            Err(e) => {
                self.warn(format!("register-sound: {e}"));
                Err(E::DecodeFailed)
            }
        })
    }

    fn play(
        &mut self,
        id: String,
        volume: f32,
    ) -> wasmtime::Result<Result<(), wit::audio::AudioError>> {
        self.require(Capability::Audio)?;
        if !self.sounds.contains(&id) {
            return Ok(Err(wit::audio::AudioError::UnknownSound));
        }
        let volume = clamp_or(volume, 0.0, 1.0, 1.0);
        self.backend
            .lock()
            .unwrap()
            .play_sound(&self.id, &id, volume);
        Ok(Ok(()))
    }

    fn stop(&mut self, id: String) -> wasmtime::Result<()> {
        self.require(Capability::Audio)?;
        self.backend.lock().unwrap().stop_sound(&self.id, &id);
        Ok(())
    }

    fn client_sounds(&mut self) -> wasmtime::Result<Vec<String>> {
        self.require(Capability::Audio)?;
        Ok(self.backend.lock().unwrap().client_sounds())
    }

    fn override_sound(
        &mut self,
        client_sound: String,
        sound: Option<String>,
    ) -> wasmtime::Result<Result<(), wit::audio::AudioError>> {
        use wit::audio::AudioError as E;
        self.require(Capability::Audio)?;
        if sound.as_ref().is_some_and(|s| !self.sounds.contains(s)) {
            return Ok(Err(E::UnknownSound));
        }
        let ok =
            self.backend
                .lock()
                .unwrap()
                .override_sound(&self.id, &client_sound, sound.as_deref());
        Ok(if ok { Ok(()) } else { Err(E::Invalid) })
    }

    fn speak(&mut self, text: String) -> wasmtime::Result<Result<(), wit::audio::AudioError>> {
        use wit::audio::AudioError as E;
        self.require(Capability::Audio)?;
        let Some(text) = clean(&text, 500) else {
            return Ok(Err(E::Invalid));
        };
        let now = self.now();
        if !self.rates.speak.take(now, 3, 10_000) {
            return Ok(Err(E::TooMany));
        }
        let ok = self.backend.lock().unwrap().speak(&self.id, &text);
        Ok(if ok { Ok(()) } else { Err(E::Invalid) })
    }
}

impl wit::notify::Host for PluginState {
    fn post(
        &mut self,
        n: wit::notify::DesktopNotification,
    ) -> wasmtime::Result<Result<(), wit::notify::NotifyError>> {
        use wit::notify::NotifyError as E;
        self.require(Capability::Notifications)?;
        let Some(title) = clean(&n.title, 100) else {
            return Ok(Err(E::Invalid));
        };
        if n.icon.as_deref().is_some_and(|i| !self.image_registered(i)) {
            return Ok(Err(E::Invalid));
        }
        if self.backend.lock().unwrap().notifications_muted() {
            return Ok(Err(E::DisabledByUser));
        }
        let now = self.now();
        if !self.rates.notify.take(now, 5, 60_000) {
            return Ok(Err(E::RateLimited));
        }
        let posted = model::DesktopNotification {
            plugin: self.id.clone(),
            title: format!("{title} · {}", self.manifest.name),
            body: clean_multiline(&n.body, 500),
            icon: n.icon,
            on_click: n.on_click,
            sound: n.sound,
        };
        Ok(if self.backend.lock().unwrap().post_notification(posted) {
            Ok(())
        } else {
            Err(E::DisabledByUser)
        })
    }
}

impl wit::http::Host for PluginState {
    fn fetch(
        &mut self,
        req: wit::http::Request,
    ) -> wasmtime::Result<Result<wit::http::Response, wit::http::HttpError>> {
        use wit::http::{HttpError as W, Method as M};
        self.require(Capability::Network)?;
        let req = net::HttpRequest {
            method: match req.method {
                M::Get => net::Method::Get,
                M::Post => net::Method::Post,
                M::Put => net::Method::Put,
                M::Patch => net::Method::Patch,
                M::Delete => net::Method::Delete,
                M::Head => net::Method::Head,
            },
            url: req.url,
            headers: req.headers,
            body: req.body,
        };
        let result = net::check_request(&req, &self.grants.network_allowlist, &self.net_limits)
            .and_then(|host| {
                self.log(
                    model::LogLevel::Debug,
                    format!("[host] {} https://{host}…", req.method.as_str()),
                );
                let t = std::time::Instant::now();
                let sent = self.net.send(&req, &self.net_limits);
                if let Some(w) = &mut self.wall {
                    w.waited += t.elapsed();
                }
                sent
            });
        if let Err(e) = &result {
            self.warn(format!("http request refused/failed: {e}"));
        }
        Ok(result
            .map(|r| wit::http::Response {
                status: r.status,
                headers: r.headers,
                body: r.body,
            })
            .map_err(|e| match e {
                net::HttpError::HostNotAllowed(h) => W::HostNotAllowed(h),
                net::HttpError::InsecureScheme => W::InsecureScheme,
                net::HttpError::InvalidUrl(s) => W::InvalidUrl(s),
                net::HttpError::Timeout => W::Timeout,
                net::HttpError::TooLarge => W::TooLarge,
                net::HttpError::Transport(s) => W::Transport(s),
            }))
    }
}

const MAX_CLIPBOARD_BYTES: usize = 64 * 1024;
const MAX_CLIPBOARD_PNG: usize = 8 * 1024 * 1024;

impl wit::clipboard::Host for PluginState {
    fn write_text(
        &mut self,
        text: String,
    ) -> wasmtime::Result<Result<(), wit::clipboard::ClipboardError>> {
        use wit::clipboard::ClipboardError as E;
        self.require(Capability::Clipboard)?;
        if text.len() > MAX_CLIPBOARD_BYTES {
            return Ok(Err(E::TooLarge));
        }
        if !self.spend(Budget::Clipboard) {
            return Ok(Err(E::NoGesture));
        }
        self.backend
            .lock()
            .unwrap()
            .write_clipboard(&self.id, &text);
        Ok(Ok(()))
    }

    fn write_png(
        &mut self,
        png: Vec<u8>,
    ) -> wasmtime::Result<Result<(), wit::clipboard::ClipboardError>> {
        use wit::clipboard::ClipboardError as E;
        self.require(Capability::Clipboard)?;
        if png.len() > MAX_CLIPBOARD_PNG || image_dims(&png).is_none() {
            return Ok(Err(E::TooLarge));
        }
        if !self.spend(Budget::Clipboard) {
            return Ok(Err(E::NoGesture));
        }
        self.backend
            .lock()
            .unwrap()
            .write_clipboard_png(&self.id, png);
        Ok(Ok(()))
    }
}

const MAX_SAVE_BYTES: usize = 64 * 1024 * 1024;
const MAX_OPEN_BYTES: usize = 16 * 1024 * 1024;

/// A file name the Save dialog can suggest: no path, no control characters.
fn safe_file_name(n: &str) -> Option<String> {
    let base = n.rsplit(['/', '\\']).next().unwrap_or(n);
    let out: String = base
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .take(128)
        .collect();
    let out = out.trim().trim_start_matches('.').to_string();
    (!out.is_empty()).then_some(out)
}

impl wit::compose_control::Host for PluginState {
    fn split(
        &mut self,
        parts: Vec<String>,
    ) -> wasmtime::Result<Result<(), wit::compose_control::SplitError>> {
        use wit::compose_control::SplitError as E;
        self.require(Capability::Compose)?;
        let Some(c) = self.composing.as_mut().filter(|c| c.sending) else {
            return Ok(Err(E::NotSending));
        };
        if !(2..=super::compose::MAX_SPLIT_PARTS).contains(&parts.len()) {
            return Ok(Err(E::PartCount));
        }
        if parts
            .iter()
            .any(|p| super::compose::check_compose(p).is_some())
        {
            return Ok(Err(E::BadPart));
        }
        c.split = Some(parts);
        Ok(Ok(()))
    }
}

impl wit::files::Host for PluginState {
    fn save(
        &mut self,
        suggested_name: String,
        content_type: String,
        data: Vec<u8>,
    ) -> wasmtime::Result<Result<(), wit::files::FileError>> {
        use wit::files::FileError as E;
        self.require(Capability::Files)?;
        if data.len() > MAX_SAVE_BYTES {
            return Ok(Err(E::TooLarge));
        }
        let Some(name) = safe_file_name(&suggested_name) else {
            return Ok(Err(E::Failed));
        };
        if !self.spend(Budget::Files) {
            return Ok(Err(E::NoGesture));
        }
        let r = self
            .backend
            .lock()
            .unwrap()
            .save_file(&self.id, &name, &content_type, data);
        Ok(r.map_err(convert::file_error_to_wit))
    }

    fn open(
        &mut self,
        accept: Vec<String>,
    ) -> wasmtime::Result<Result<wit::files::Picked, wit::files::FileError>> {
        use wit::files::FileError as E;
        self.require(Capability::Files)?;
        if !self.spend(Budget::Files) {
            return Ok(Err(E::NoGesture));
        }
        let accept: Vec<String> = accept
            .into_iter()
            .take(16)
            .map(|a| a.trim_start_matches('.').to_ascii_lowercase())
            .collect();
        let r = self.backend.lock().unwrap().open_file(&self.id, &accept);
        Ok(match r {
            Ok(f) if f.data.len() > MAX_OPEN_BYTES => Err(E::TooLarge),
            Ok(f) => Ok(wit::files::Picked {
                name: f.name,
                content_type: f.content_type,
                data: f.data,
            }),
            Err(e) => Err(convert::file_error_to_wit(e)),
        })
    }
}

impl wit::tweaks::Host for PluginState {
    fn all(&mut self) -> wasmtime::Result<Vec<wit::tweaks::TweakInfo>> {
        self.require(Capability::Tweaks)?;
        Ok(self
            .backend
            .lock()
            .unwrap()
            .tweaks()
            .into_iter()
            .map(|t| wit::tweaks::TweakInfo {
                id: t.id,
                label: t.label,
                description: t.description,
                kind: convert::tweak_kind_to_wit(t.kind),
                choices: t.choices,
                value: t.value,
                available: t.available,
            })
            .collect())
    }

    fn set(
        &mut self,
        id: String,
        value: String,
    ) -> wasmtime::Result<Result<(), wit::tweaks::TweakError>> {
        self.require(Capability::Tweaks)?;
        let r = self
            .backend
            .lock()
            .unwrap()
            .set_tweak(&self.id, &id, &value);
        Ok(r.map_err(convert::tweak_error_to_wit))
    }

    fn reset(&mut self, id: String) -> wasmtime::Result<()> {
        self.require(Capability::Tweaks)?;
        self.backend.lock().unwrap().reset_tweak(&self.id, &id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_dims_reads_png_and_gif_headers() {
        let mut p = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        p.extend_from_slice(&[0, 0, 0, 13]);
        p.extend_from_slice(b"IHDR");
        p.extend_from_slice(&16u32.to_be_bytes());
        p.extend_from_slice(&8u32.to_be_bytes());
        assert_eq!(image_dims(&p), Some((16, 8)));
        let mut g = b"GIF89a".to_vec();
        g.extend_from_slice(&256u16.to_le_bytes());
        g.extend_from_slice(&128u16.to_le_bytes());
        assert_eq!(image_dims(&g), Some((256, 128)));
        assert_eq!(image_dims(b"not an image........."), None);
        assert_eq!(image_dims(&[]), None);
    }

    #[test]
    fn font_checks() {
        assert!(check_font("Inter", &[0, 1, 0, 0, 9, 9]).is_ok());
        assert!(check_font("Inter", b"OTTO....").is_ok());
        assert!(check_font("Inter", b"<svg").is_err());
        assert!(check_font("", b"OTTO").is_err());
        assert!(check_font("x", &vec![0u8; 9 * 1024 * 1024]).is_err());
    }

    #[test]
    fn file_names() {
        assert_eq!(
            safe_file_name("../../etc/passwd").as_deref(),
            Some("passwd")
        );
        assert_eq!(safe_file_name("C:\\x\\log.txt").as_deref(), Some("log.txt"));
        assert_eq!(safe_file_name(".hidden").as_deref(), Some("hidden"));
        assert_eq!(safe_file_name("a?b*c.json").as_deref(), Some("abc.json"));
        assert_eq!(safe_file_name("..").as_deref(), None);
    }
}
