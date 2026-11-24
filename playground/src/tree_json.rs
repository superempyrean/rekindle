//! A tree as JSON for the page.
//!
//! ```json
//! {"root": 1,
//!  "nodes": {"1": {"k": "view", "c": [2, 3], "p": {"background": -15592428}}}}
//! ```
//!
//! `p` holds each cell by its `PropId`'s name, or its number when the
//! table has no name for it. A colour is the integer the guest wrote,
//! `0xAARRGGBB` as a signed `i32` when it came as one. Strings, numbers,
//! bools and arrays are themselves, a record (tag 8) is an object by field
//! name, and the rest are one-key objects: `{"$quad": [l, t, r, b]}`,
//! `{"$map": {...}}` and `{"$words": [...]}`.
//!
//! A node whose `worklet` is a button's press look also has `rest`: the
//! colours the look paints while the button isn't pressed, by the cell
//! they stand over (`background`, `borderColor`, `tint`, `color`). The
//! platform paints those every frame, over the node's own cells.

use rekindle_wire::motion::Channel;
use rekindle_wire::motion::ids::WORKLET;
use rekindle_wire::motion::worklet::{PressLook, Worklet};
use rekindle_wire::{PropId, Tree, Value};
use serde_json::{Map, Number, Value as Json};

/// `tree` as JSON: its root and every node.
pub fn tree_json(tree: &Tree) -> String {
    let mut nodes = Map::new();
    for n in tree.nodes() {
        let mut node = Map::new();
        node.insert("k".into(), Json::String(n.kind.name().to_string()));
        node.insert(
            "c".into(),
            Json::Array(n.children.iter().map(|c| Json::from(*c)).collect()),
        );
        let mut props = Map::new();
        for (p, v) in tree.props().node_props(n.id) {
            props.insert(prop_name(p), value_json(v));
        }
        node.insert("p".into(), Json::Object(props));
        if let Some(rest) = tree.props().get(n.id, WORKLET).and_then(rest_colors) {
            node.insert("rest".into(), Json::Object(rest));
        }
        nodes.insert(n.id.to_string(), Json::Object(node));
    }
    let mut out = Map::new();
    out.insert("root".into(), tree.root().map_or(Json::Null, Json::from));
    out.insert("nodes".into(), Json::Object(nodes));
    Json::Object(out).to_string()
}

fn prop_name(p: PropId) -> String {
    p.name().map_or_else(|| p.0.to_string(), str::to_string)
}

/// One value as JSON: see the module docs.
pub fn value_json(v: &Value) -> Json {
    let one = |k: &str, v: Json| {
        let mut m = Map::new();
        m.insert(k.into(), v);
        Json::Object(m)
    };
    match v {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::I32(i) => Json::from(*i),
        Value::I64(i) => Json::from(*i),
        Value::F64(f) => Number::from_f64(*f).map_or(Json::Null, Json::Number),
        Value::Str(s) => Json::String(s.clone()),
        Value::Array(a) => Json::Array(a.iter().map(value_json).collect()),
        Value::Object(fields) => Json::Object(
            fields
                .iter()
                .map(|(k, v)| (prop_name(*k), value_json(v)))
                .collect(),
        ),
        Value::Quad(q) => one(
            "$quad",
            Json::Array(
                q.iter()
                    .map(|f| Number::from_f64(f64::from(*f)).map_or(Json::Null, Json::Number))
                    .collect(),
            ),
        ),
        Value::Map(m) => one(
            "$map",
            Json::Object(m.iter().map(|(k, v)| (k.clone(), value_json(v))).collect()),
        ),
        Value::U32Array(w) => one(
            "$words",
            Json::Array(w.iter().map(|x| Json::from(*x)).collect()),
        ),
        Value::Sparse { list, .. } => one("$sparse", Json::from(*list)),
    }
}

/// The colours a press look paints at rest, by the cell each stands over.
fn rest_colors(worklet: &Value) -> Option<Map<String, Json>> {
    let look = PressLook::from_worklet(&Worklet::from_value(worklet)?)?;
    let mut out = Map::new();
    for l in look
        .looks
        .iter()
        .filter(|l| l.is_color() && l.only_pressed == 0)
    {
        let cell = match Channel::from_id(l.channel)? {
            Channel::Background => "background",
            Channel::BorderColor => "borderColor",
            Channel::Tint => "tint",
            Channel::TextColor => "color",
            _ => continue,
        };
        out.insert(cell.into(), Json::from(l.rest));
    }
    (!out.is_empty()).then_some(out)
}
