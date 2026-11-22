//! The user's settings protobuf (`PreloadedUserSettings`), as much of it as
//! rekindle reads.
//!
//! READY carries it base64-encoded in `user_settings_proto`, and
//! `USER_SETTINGS_PROTO_UPDATE` sends a new one (`settings.type` 1) whenever a
//! setting changes, with `partial` set when it holds only the changed
//! fields. Field numbers are Discord's (`discord_protos.PreloadedUserSettings`);
//! prost skips every field these structs don't name.

use base64::Engine;
use prost::Message;

use super::convert;
use crate::api;

#[derive(Clone, PartialEq, Message)]
pub struct PreloadedUserSettings {
    #[prost(message, optional, tag = "14")]
    pub guild_folders: Option<GuildFolders>,
}

#[derive(Clone, PartialEq, Message)]
pub struct GuildFolders {
    #[prost(message, repeated, tag = "1")]
    pub folders: Vec<GuildFolder>,
    /// Guilds outside any folder, in sidebar order (older clients).
    #[prost(fixed64, repeated, tag = "2")]
    pub guild_positions: Vec<u64>,
}

#[derive(Clone, PartialEq, Message)]
pub struct GuildFolder {
    #[prost(fixed64, repeated, tag = "1")]
    pub guild_ids: Vec<u64>,
    #[prost(message, optional, tag = "2")]
    pub id: Option<Int64Value>,
    #[prost(message, optional, tag = "3")]
    pub name: Option<StringValue>,
    #[prost(message, optional, tag = "4")]
    pub color: Option<UInt64Value>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Int64Value {
    #[prost(int64, tag = "1")]
    pub value: i64,
}

#[derive(Clone, PartialEq, Message)]
pub struct StringValue {
    #[prost(string, tag = "1")]
    pub value: String,
}

#[derive(Clone, PartialEq, Message)]
pub struct UInt64Value {
    #[prost(uint64, tag = "1")]
    pub value: u64,
}

/// Decode a base64 `PreloadedUserSettings`. `None` if it isn't one.
pub fn decode(b64: &str) -> Option<PreloadedUserSettings> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()?;
    PreloadedUserSettings::decode(bytes.as_slice()).ok()
}

/// The sidebar as folders, top to bottom. A guild outside any folder is a
/// folder of its own with no id, the way the client stores it. `None` when
/// the settings don't carry folders (a partial update that didn't change
/// them).
pub fn folders(settings: &PreloadedUserSettings) -> Option<Vec<api::GuildFolder>> {
    let gf = settings.guild_folders.as_ref()?;
    let mut out: Vec<api::GuildFolder> = gf
        .folders
        .iter()
        .map(|f| api::GuildFolder {
            id: f.id.as_ref().map(|i| i.value as u64),
            name: f
                .name
                .as_ref()
                .map(|n| n.value.clone())
                .filter(|n| !n.is_empty()),
            color: f.color.as_ref().map(|c| convert::rgb(c.value as u32)),
            guilds: f.guild_ids.clone(),
        })
        .collect();
    let placed: std::collections::BTreeSet<u64> =
        out.iter().flat_map(|f| f.guilds.iter().copied()).collect();
    for g in gf.guild_positions.iter().filter(|g| !placed.contains(g)) {
        out.push(api::GuildFolder {
            id: None,
            name: None,
            color: None,
            guilds: vec![*g],
        });
    }
    Some(out)
}
