//! How a [`Work`](crate::Work) spells `PropIds`, element kinds and values in
//! JSON.
//!
//! A `PropId` is its name from the recovered table (`"background"`), or its
//! number when it has none. A kind is its name (`"view"`), and any name is
//! a kind. A value is an object with one key, its variant: `{"str":
//! "Mute"}`, `{"i32": 3}`, `{"quad": [8, 12, 8, 12]}`, `{"rgba":
//! "ff7a18ff"}` (read as the integer colour `0xFFFF7A18`),
//! `{"object": [["opacity", {"f64": 0}]]}`, and `"null"`. A
//! [`Recolor`](crate::Recolor)'s colours are hex strings: `"1e1f22"` and
//! `"050609ff"`. A [`PropRecolor`](crate::PropRecolor) is
//! `{"props": ["background"], "max_chroma": 24, "opaque": true,
//! "to": "ff00ffff"}`.

use rekindle_wire::{ElementKind, PropId, Value};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A `PropId` by name, or by number.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Prop {
    Name(String),
    Id(u16),
}

impl Prop {
    fn of(id: PropId) -> Self {
        id.name()
            .map_or(Prop::Id(id.0), |n| Prop::Name(n.to_string()))
    }

    fn id<E: serde::de::Error>(self) -> Result<PropId, E> {
        match self {
            Prop::Id(n) => Ok(PropId(n)),
            Prop::Name(n) => {
                PropId::from_name(&n).ok_or_else(|| E::custom(format!("no PropId named {n:?}")))
            }
        }
    }
}

/// `#[serde(with)]` for a [`PropId`].
pub(crate) mod prop {
    use super::{Deserialize, Deserializer, Prop, PropId, Serialize, Serializer};

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde's `with` passes a reference"
    )]
    pub(crate) fn serialize<S: Serializer>(id: &PropId, s: S) -> Result<S::Ok, S::Error> {
        Prop::of(*id).serialize(s)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PropId, D::Error> {
        Prop::deserialize(d)?.id()
    }
}

/// `#[serde(with)]` for a list of [`PropId`]s.
pub(crate) mod props {
    use super::{Deserialize, Deserializer, Prop, PropId, Serialize, Serializer};

    pub(crate) fn serialize<S: Serializer>(ids: &[PropId], s: S) -> Result<S::Ok, S::Error> {
        ids.iter()
            .map(|id| Prop::of(*id))
            .collect::<Vec<_>>()
            .serialize(s)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<PropId>, D::Error> {
        Vec::<Prop>::deserialize(d)?
            .into_iter()
            .map(Prop::id)
            .collect()
    }
}

/// `#[serde(with)]` for an [`ElementKind`].
pub(crate) mod kind {
    use super::{Deserialize, Deserializer, ElementKind, Serialize, Serializer};

    pub(crate) fn serialize<S: Serializer>(kind: &ElementKind, s: S) -> Result<S::Ok, S::Error> {
        kind.name().serialize(s)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<ElementKind, D::Error> {
        Ok(ElementKind::new(String::deserialize(d)?))
    }
}

/// A [`Value`] in JSON.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Json {
    Null,
    Bool(bool),
    I32(i32),
    I64(i64),
    F64(f64),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(Prop, Json)>),
    Quad([f32; 4]),
    /// `0xRRGGBBAA` as eight hex digits, read as the integer `0xAARRGGBB`.
    Rgba(String),
    Map(Vec<(String, Json)>),
    U32Array(Vec<u32>),
    Sparse {
        list: u32,
        pairs: Vec<(u16, u32)>,
    },
}

impl Json {
    fn of(v: &Value) -> Self {
        match v {
            Value::Null => Json::Null,
            Value::Bool(b) => Json::Bool(*b),
            Value::I32(n) => Json::I32(*n),
            Value::I64(n) => Json::I64(*n),
            Value::F64(n) => Json::F64(*n),
            Value::Str(s) => Json::Str(s.clone()),
            Value::Array(a) => Json::Array(a.iter().map(Json::of).collect()),
            Value::Object(o) => {
                Json::Object(o.iter().map(|(p, v)| (Prop::of(*p), Json::of(v))).collect())
            }
            Value::Quad(v) => Json::Quad(*v),
            Value::Map(m) => Json::Map(m.iter().map(|(k, v)| (k.clone(), Json::of(v))).collect()),
            Value::U32Array(a) => Json::U32Array(a.clone()),
            Value::Sparse { list, pairs } => Json::Sparse {
                list: *list,
                pairs: pairs.clone(),
            },
        }
    }

    fn value<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(match self {
            Json::Null => Value::Null,
            Json::Bool(b) => Value::Bool(b),
            Json::I32(n) => Value::I32(n),
            Json::I64(n) => Value::I64(n),
            Json::F64(n) => Value::F64(n),
            Json::Str(s) => Value::Str(s),
            Json::Array(a) => {
                Value::Array(a.into_iter().map(Json::value).collect::<Result<_, _>>()?)
            }
            Json::Object(o) => Value::Object(
                o.into_iter()
                    .map(|(p, v)| Ok((p.id()?, v.value()?)))
                    .collect::<Result<_, E>>()?,
            ),
            Json::Quad(v) => Value::Quad(v),
            Json::Rgba(hex) => {
                let n = u32::from_str_radix(&hex, 16)
                    .ok()
                    .filter(|_| hex.len() == 8)
                    .ok_or_else(|| E::custom(format!("rgba {hex:?} is not eight hex digits")))?;
                crate::rgba(n)
            }
            Json::Map(m) => Value::Map(
                m.into_iter()
                    .map(|(k, v)| Ok((k, v.value()?)))
                    .collect::<Result<_, E>>()?,
            ),
            Json::U32Array(a) => Value::U32Array(a),
            Json::Sparse { list, pairs } => Value::Sparse { list, pairs },
        })
    }
}

/// `#[serde(with)]` for a [`Value`].
pub(crate) mod value {
    use super::{Deserialize, Deserializer, Json, Serialize, Serializer, Value};

    pub(crate) fn serialize<S: Serializer>(v: &Value, s: S) -> Result<S::Ok, S::Error> {
        Json::of(v).serialize(s)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Value, D::Error> {
        Json::deserialize(d)?.value()
    }
}

/// A colour as a fixed number of hex digits.
fn hex_digits<S: Serializer>(n: u32, digits: usize, s: S) -> Result<S::Ok, S::Error> {
    format!("{n:0digits$x}").serialize(s)
}

fn from_hex_digits<'de, D: Deserializer<'de>>(d: D, digits: usize) -> Result<u32, D::Error> {
    let hex = String::deserialize(d)?;
    u32::from_str_radix(&hex, 16)
        .ok()
        .filter(|_| hex.len() == digits)
        .ok_or_else(|| serde::de::Error::custom(format!("{hex:?} is not {digits} hex digits")))
}

/// `#[serde(with)]` for `0xRRGGBB` as six hex digits.
pub(crate) mod rgb {
    use super::{Deserializer, Serializer};

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde's `with` passes a reference"
    )]
    pub(crate) fn serialize<S: Serializer>(n: &u32, s: S) -> Result<S::Ok, S::Error> {
        super::hex_digits(*n, 6, s)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
        super::from_hex_digits(d, 6)
    }
}

/// `#[serde(with)]` for `0xRRGGBBAA` as eight hex digits.
pub(crate) mod rgba {
    use super::{Deserializer, Serializer};

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "serde's `with` passes a reference"
    )]
    pub(crate) fn serialize<S: Serializer>(n: &u32, s: S) -> Result<S::Ok, S::Error> {
        super::hex_digits(*n, 8, s)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
        super::from_hex_digits(d, 8)
    }
}
