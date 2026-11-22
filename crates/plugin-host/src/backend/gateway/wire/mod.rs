//! The gateway's JSON, as much of it as rekindle keeps.
//!
//! Each struct names only the fields the reducer reads. Everything else in a
//! dispatch, including the session id, tokens, analytics ids, voice session
//! ids and activity secrets, is skipped by serde and never reaches a store.
//! Shapes follow a user account's session with the capabilities the
//! official client identifies with (`DEDUPE_USER_OBJECTS`,
//! `CLIENT_STATE_V2`, versioned read states and guild settings).

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

mod dispatches;
mod messages;
mod objects;

pub use dispatches::*;
pub use messages::*;
pub use objects::*;

/// A dispatch's body as `T`; `None` if it doesn't decode.
pub fn parse<T: DeserializeOwned>(d: &Value) -> Option<T> {
    T::deserialize(d).ok()
}

/// A snowflake, which the gateway sends as a string (and older payloads as
/// a number).
pub fn snowflake<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum S {
        Str(String),
        Num(u64),
    }
    match S::deserialize(d)? {
        S::Str(s) => s.parse().map_err(serde::de::Error::custom),
        S::Num(n) => Ok(n),
    }
}

/// A snowflake on its own, for the helpers below.
#[derive(Deserialize)]
struct Snowflake(#[serde(deserialize_with = "snowflake")] u64);

/// An optional snowflake.
pub fn opt_snowflake<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Ok(Option::<Snowflake>::deserialize(d)?.map(|w| w.0))
}

/// A list of snowflakes; null and missing are empty.
pub fn snowflakes<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u64>, D::Error> {
    Ok(opt_snowflakes(d)?.unwrap_or_default())
}

/// An optional list of snowflakes.
pub fn opt_snowflakes<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u64>>, D::Error> {
    Ok(Option::<Vec<Snowflake>>::deserialize(d)?.map(|v| v.into_iter().map(|w| w.0).collect()))
}

/// A permission bit set, sent as a decimal string.
pub fn bits<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    snowflake(d)
}

/// Null and missing both mean the default.
pub fn nullable<'de, D: Deserializer<'de>, T: Default + Deserialize<'de>>(
    d: D,
) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}
