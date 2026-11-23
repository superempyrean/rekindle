//! Styled text and an input's text: `spans`, `valueSpans` and `forceValue`.
//!
//! RECOVERED (from the Android platform's readers): a text node's `spans`
//! and an input's `valueSpans` are arrays of pieces. Each piece is an object
//! whose `text` is a string, and the pieces' texts concatenate to the text
//! shown. The platform reads each piece's style by name, with these types:
//!
//! | Key | Type | |
//! |---|---|---|
//! | `text` | string | the piece's text |
//! | `link` | string | makes the piece a link (`spans` only) |
//! | `color` | integer, `0xAARRGGBB` | |
//! | `weight` | integer | a font weight, 0 for none |
//! | `font` | string | a font family |
//! | `bold`, `italic`, `underline`, `strike`, `code` | bool | `code` is monospace |
//! | `scale` | number | the size relative to the node's |
//! | `bg`, `bgBorder` | integer colour | a background behind the piece |
//! | `bgRadius`, `bgBorderWidth`, `bgInsetY` | number | |
//! | `bgShadow`, `gradient`, `roleDot` | array | |
//! | `gradientShape` | object: `angle`, `length` (numbers), `clamp` (bool) | |
//! | `image` | object: `uri` (string), `size`, `radius` (numbers), `tint` (colour), `pool` (string) | an inline image |
//! | `advance`, `belowBaseline` | number | |
//! | `baseline`, `tabularNumbers` | bool | |
//!
//! An input's `valueSpans` style the text the field already holds: piece
//! `k` covers the characters after the first `k` pieces' texts, so the
//! pieces must add up to the field's text or the styles land in the wrong
//! place.
//!
//! An input's text is set with `forceValue`, an object of `seq`, `text`,
//! `start` and `end` (the selection), and optionally `eventCount`. The
//! platform applies it when `seq` differs from the last one it applied, and
//! never otherwise. With `eventCount` older than the last `changeText` the
//! platform sent, it asks the host to rebase the text over the edits made
//! since. A plain `value` is ignored while a `changeText` the platform sent
//! hasn't been answered, so a value written back while the user types
//! doesn't undo their typing.
//!
//! The platform turns both tag 8 (keys as `PropId`s) and tag 10 (keys as
//! strings) into the same map before reading it, so [`Span::from_value`]
//! and [`ForceValue::from_value`] read either.
//!
//! Modelled: which of the two tags the guest writes for a piece isn't
//! recorded, since no captured screen carried spans. [`Span::to_value`]
//! and [`ForceValue::to_value`] write tag 8, as for the other objects the
//! guest sends.

use crate::{PropId, Value};

/// One piece of styled text: its text, then its style as `(PropId, Value)`
/// pairs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Span {
    pub text: String,
    pub style: Vec<(PropId, Value)>,
}

impl Span {
    /// A piece with no style.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Vec::new(),
        }
    }

    /// The piece with one style key set. A name the `PropId` table lacks is
    /// dropped: the platform would drop it too.
    #[must_use]
    pub fn with(mut self, name: &str, value: Value) -> Self {
        if let Some(id) = PropId::from_name(name) {
            self.style.retain(|(p, _)| *p != id);
            self.style.push((id, value));
        }
        self
    }

    /// A style key's value, by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Value> {
        let id = PropId::from_name(name)?;
        self.style.iter().find(|(p, _)| *p == id).map(|(_, v)| v)
    }

    /// The piece's `color`, `0xAARRGGBB`.
    #[must_use]
    pub fn color(&self) -> Option<u32> {
        self.get("color").and_then(Value::as_color)
    }

    /// A bool style key (`bold`, `italic`, `underline`, `strike`, `code`),
    /// false when absent, as the platform reads it.
    #[must_use]
    pub fn flag(&self, name: &str) -> bool {
        matches!(self.get(name), Some(Value::Bool(true)))
    }

    /// The piece as tag 8, `text` first.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut props = Vec::with_capacity(self.style.len() + 1);
        props.push((text_id(), Value::Str(self.text.clone())));
        props.extend(self.style.iter().cloned());
        Value::Object(props)
    }

    /// A piece from a tag 8 or tag 10 object. A key outside the `PropId`
    /// table is dropped, and a missing `text` reads as empty.
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let mut span = Span::default();
        for (id, v) in entries(value)? {
            if id == text_id() {
                span.text = text_of(&v);
            } else {
                span.style.push((id, v));
            }
        }
        Some(span)
    }

    /// `spans` or `valueSpans` as an array of pieces.
    #[must_use]
    pub fn list_value(spans: &[Span]) -> Value {
        Value::Array(spans.iter().map(Span::to_value).collect())
    }

    /// The pieces of a `spans` or `valueSpans` array. An element that isn't
    /// an object is skipped, as the platform skips it.
    #[must_use]
    pub fn list_from_value(value: &Value) -> Option<Vec<Span>> {
        match value {
            Value::Array(items) => Some(items.iter().filter_map(Span::from_value).collect()),
            _ => None,
        }
    }

    /// The text the pieces show: their texts, concatenated.
    #[must_use]
    pub fn plain_text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.text.as_str()).collect()
    }
}

/// An input's `forceValue`: replace the field's text and selection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForceValue {
    /// Applied once per value: the platform skips a `forceValue` whose
    /// `seq` equals the last one it applied.
    pub seq: i64,
    pub text: String,
    /// The selection after the text is set, clamped to the text's length.
    pub start: i32,
    pub end: i32,
    /// The count of `changeText` events this text accounts for. When the
    /// platform has sent more since, it rebases the text over them.
    pub event_count: Option<i64>,
}

impl ForceValue {
    /// Set the text with the caret at its end.
    #[must_use]
    pub fn new(seq: i64, text: impl Into<String>) -> Self {
        let text = text.into();
        let end = i32::try_from(text.chars().count()).unwrap_or(i32::MAX);
        Self {
            seq,
            text,
            start: end,
            end,
            event_count: None,
        }
    }

    /// As tag 8: `seq`, `text`, `start`, `end`, then `eventCount` when set.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let id = |name: &str| PropId::from_name(name).expect("forceValue's keys are PropIds");
        let mut props = vec![
            (id("seq"), Value::int(self.seq)),
            (text_id(), Value::Str(self.text.clone())),
            (id("start"), Value::int(i64::from(self.start))),
            (id("end"), Value::int(i64::from(self.end))),
        ];
        if let Some(n) = self.event_count {
            props.push((id("eventCount"), Value::int(n)));
        }
        Value::Object(props)
    }

    /// A `forceValue` from a tag 8 or tag 10 object. Missing numbers read as
    /// 0 and a missing `text` as empty, as on the platform.
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let mut out = ForceValue::default();
        for (id, v) in entries(value)? {
            match id.name() {
                Some("seq") => out.seq = number(&v).unwrap_or(0),
                Some("text") => out.text = text_of(&v),
                Some("start") => out.start = number(&v).unwrap_or(0) as i32,
                Some("end") => out.end = number(&v).unwrap_or(0) as i32,
                Some("eventCount") => out.event_count = number(&v),
                _ => {}
            }
        }
        Some(out)
    }
}

fn text_id() -> PropId {
    PropId::from_name("text").expect("`text` is a PropId")
}

/// An object's entries keyed by `PropId`, from tag 8 or tag 10.
fn entries(value: &Value) -> Option<Vec<(PropId, Value)>> {
    match value {
        Value::Object(props) => Some(props.clone()),
        Value::Map(entries) => Some(
            entries
                .iter()
                .filter_map(|(k, v)| Some((PropId::from_name(k)?, v.clone())))
                .collect(),
        ),
        _ => None,
    }
}

/// A string the way the platform reads one: a string as is, Null as empty,
/// anything else as its text.
fn text_of(value: &Value) -> String {
    match value {
        Value::Str(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::I32(n) => n.to_string(),
        Value::I64(n) => n.to_string(),
        Value::F64(f) => f.to_string(),
        _ => String::new(),
    }
}

/// A number read as an integer, as the platform's long reader does.
fn number(value: &Value) -> Option<i64> {
    match value {
        Value::I32(n) => Some(i64::from(*n)),
        Value::I64(n) => Some(*n),
        Value::F64(f) => Some(*f as i64),
        Value::Str(s) => s.parse::<f64>().ok().map(|f| f as i64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode;

    #[test]
    fn spans_round_trip_through_the_codec() {
        let spans = vec![
            Span::new("hello ").with("bold", Value::Bool(true)),
            Span::new("world")
                .with("color", Value::color(0xff5865f2))
                .with("link", Value::Str("https://example.com".into())),
        ];
        let value = Span::list_value(&spans);
        let back = decode(&value.to_bytes()).unwrap();
        let read = Span::list_from_value(&back).unwrap();
        assert_eq!(read, spans);
        assert_eq!(Span::plain_text(&read), "hello world");
        assert!(read[0].flag("bold"));
        assert!(!read[0].flag("italic"));
        assert_eq!(read[1].color(), Some(0xff5865f2));
    }

    #[test]
    fn a_piece_writes_its_text_first() {
        let Value::Object(props) = Span::new("x").with("italic", Value::Bool(true)).to_value()
        else {
            panic!("a piece is tag 8");
        };
        assert_eq!(props[0], (text_id(), Value::Str("x".into())));
    }

    #[test]
    fn pieces_read_from_string_keyed_maps_too() {
        let map = Value::Array(vec![
            Value::Map(vec![
                ("text".into(), Value::Str("a".into())),
                ("weight".into(), Value::I32(700)),
                ("noSuchKey".into(), Value::Bool(true)),
            ]),
            Value::I32(3),
        ]);
        let read = Span::list_from_value(&map).unwrap();
        assert_eq!(read.len(), 1, "a non-object element is skipped");
        assert_eq!(read[0].get("weight"), Some(&Value::I32(700)));
        assert_eq!(read[0].style.len(), 1, "an unknown key is dropped");
    }

    #[test]
    fn force_value_puts_the_caret_at_the_end() {
        let f = ForceValue::new(7, "héllo");
        assert_eq!((f.start, f.end), (5, 5));
        let back = ForceValue::from_value(&decode(&f.to_value().to_bytes()).unwrap()).unwrap();
        assert_eq!(back, f);
        let counted = ForceValue {
            event_count: Some(3),
            ..f
        };
        let Value::Object(props) = counted.to_value() else {
            panic!("forceValue is tag 8");
        };
        assert_eq!(
            props
                .iter()
                .map(|(p, _)| p.name().unwrap())
                .collect::<Vec<_>>(),
            ["seq", "text", "start", "end", "eventCount"]
        );
    }

    #[test]
    fn force_value_reads_numbers_as_the_platform_does() {
        let v = Value::Map(vec![
            ("seq".into(), Value::F64(4.0)),
            ("text".into(), Value::Null),
            ("end".into(), Value::Str("2".into())),
        ]);
        let f = ForceValue::from_value(&v).unwrap();
        assert_eq!((f.seq, f.text.as_str(), f.start, f.end), (4, "", 0, 2));
        assert_eq!(f.event_count, None);
    }
}
