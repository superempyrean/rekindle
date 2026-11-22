//! Sounds (requires `audio`). Ship sounds of your own, play them, replace
//! the client's (`message1`, `deafen`, ...), and read text aloud with the
//! client's local text-to-speech. Nothing is ever recorded.

use crate::raw::rekindle::plugin::audio as raw;
pub use raw::AudioError;

/// Registers an Ogg/Opus or WAV sound under `id`, for [`play`] and
/// [`override_sound`]. At most 1 MiB and 30 s each, and 16 per plugin.
pub fn register(id: &str, data: &[u8]) -> Result<(), AudioError> {
    raw::register_sound(id, data)
}

/// Plays a registered sound at `volume` (0 to 1, scaled by the client's
/// notification volume).
pub fn play(id: &str, volume: f32) -> Result<(), AudioError> {
    raw::play(id, volume)
}

/// Stops a sound that's playing.
pub fn stop(id: &str) {
    raw::stop(id);
}

/// The client's own sound names.
pub fn client_sounds() -> Vec<String> {
    raw::client_sounds()
}

/// Plays a registered sound in place of a client sound; `None` restores the
/// client's.
pub fn override_sound(client_sound: &str, sound: Option<&str>) -> Result<(), AudioError> {
    raw::override_sound(client_sound, sound)
}

/// Reads `text` aloud with the client's text-to-speech voice. Nothing is
/// sent anywhere. Rate-limited.
pub fn speak(text: &str) -> Result<(), AudioError> {
    raw::speak(text)
}
