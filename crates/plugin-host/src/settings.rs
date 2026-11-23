//! Plugin settings declared as a schema in the manifest (design doc §10).
//!
//! The host renders the settings page from the schema, keeps the values
//! (outside the plugin's storage quota), validates every write against the
//! schema, and evaluates `visible-when` / `enabled-when` itself, so the
//! install screen and the settings page work without running plugin code.
//!
//! Authors write the schema as an array of tables (order is the page order):
//!
//! ```toml
//! [[package.metadata.rekindle.settings]]
//! key = "log-deletes"
//! type = "toggle"
//! label = "Keep deleted messages"
//! default = true
//! ```
//!
//! A table keyed by setting key is accepted too (then the page is sorted by
//! key, since TOML/JSON tables don't keep order through `cargo metadata`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::model::Color;

/// Max settings per plugin, and per-value caps.
const MAX_SETTINGS: usize = 64;
const MAX_TEXT: usize = 4096;
const MAX_ITEMS: usize = 500;
const MAX_COLUMNS: usize = 8;

/// What the settings page shows for a `secret` setting that has a value.
/// Writing it back leaves the value as it is, so a page that sends the
/// field unchanged doesn't overwrite the secret.
pub const SECRET_MASK: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SettingKind {
    Toggle,
    Select,
    Slider,
    Number,
    Text,
    /// Text the settings page masks: an API key, a token. The plugin reads
    /// it in full. It can't have a default.
    Secret,
    Color,
    Keybind,
    StringList,
    Table,
    /// Pickers; values are snowflake lists.
    Users,
    Channels,
    Guilds,
}

/// One declared setting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SettingSpec {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: SettingKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Json>,
    /// `select`: the options.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// `slider` / `number` bounds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// `slider`: labelled tick positions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<f64>,
    /// `text`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub multiline: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub placeholder: String,
    /// `table`: column headers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    /// `key == value` / `key != value`, evaluated by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_when: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_when: Option<String>,
}

/// A setting's value (mirrors `settings.setting-value`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum SettingValue {
    Boolean(bool),
    Number(f64),
    Text(String),
    Selected(u32),
    Color(Color),
    Items(Vec<String>),
    Rows(Vec<Vec<String>>),
    Keybind(String),
    Ids(Vec<u64>),
}

impl SettingValue {
    /// A comparable text form, for `visible-when` expressions.
    fn as_expr_text(&self, spec: &SettingSpec) -> String {
        match self {
            SettingValue::Boolean(b) => b.to_string(),
            SettingValue::Number(n) => trim_number(*n),
            SettingValue::Text(t) | SettingValue::Keybind(t) => t.clone(),
            SettingValue::Selected(i) => spec
                .options
                .get(*i as usize)
                .cloned()
                .unwrap_or_else(|| i.to_string()),
            SettingValue::Color(c) => c.to_hex(),
            SettingValue::Items(v) => v.len().to_string(),
            SettingValue::Rows(v) => v.len().to_string(),
            SettingValue::Ids(v) => v.len().to_string(),
        }
    }
}

fn trim_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        n.to_string()
    }
}

/// Parse the `settings` metadata (array of tables, or a table keyed by key).
pub fn parse_schema(raw: &Json) -> Result<Vec<SettingSpec>> {
    let specs: Vec<SettingSpec> = match raw {
        Json::Array(items) => items
            .iter()
            .map(|v| serde_json::from_value(v.clone()))
            .collect::<std::result::Result<_, _>>()
            .context("invalid settings entry")?,
        Json::Object(map) => {
            let mut out = Vec::new();
            for (k, v) in map {
                let mut v = v.clone();
                if let Json::Object(o) = &mut v {
                    o.entry("key").or_insert_with(|| Json::String(k.clone()));
                }
                out.push(
                    serde_json::from_value(v).with_context(|| format!("invalid setting `{k}`"))?,
                );
            }
            out
        }
        _ => bail!("`settings` must be an array of tables or a table"),
    };
    validate_schema(&specs)?;
    Ok(specs)
}

pub(crate) fn validate_schema(specs: &[SettingSpec]) -> Result<()> {
    if specs.len() > MAX_SETTINGS {
        bail!("at most {MAX_SETTINGS} settings");
    }
    let mut seen = std::collections::BTreeSet::new();
    for s in specs {
        let ok_key = !s.key.is_empty()
            && s.key.len() <= 64
            && s.key.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.')
            });
        if !ok_key {
            bail!(
                "invalid setting key `{}` (lowercase letters, digits, `-`, `_`, `.`)",
                s.key
            );
        }
        if !seen.insert(&s.key) {
            bail!("setting `{}` declared twice", s.key);
        }
        if s.label.trim().is_empty() {
            bail!("setting `{}` has no label", s.key);
        }
        match s.kind {
            SettingKind::Select if s.options.is_empty() => {
                bail!("select setting `{}` has no options", s.key)
            }
            SettingKind::Table if s.columns.is_empty() || s.columns.len() > MAX_COLUMNS => {
                bail!("table setting `{}` needs 1-{MAX_COLUMNS} columns", s.key)
            }
            SettingKind::Slider if s.min.is_none() || s.max.is_none() => {
                bail!("slider setting `{}` needs min and max", s.key)
            }
            SettingKind::Secret if s.default.is_some() => {
                bail!("secret setting `{}` can't have a default", s.key)
            }
            SettingKind::Secret if s.multiline => {
                bail!("secret setting `{}` can't be multiline", s.key)
            }
            _ => {}
        }
        if let (Some(lo), Some(hi)) = (s.min, s.max)
            && lo > hi
        {
            bail!("setting `{}` has min > max", s.key);
        }
        for expr in [&s.visible_when, &s.enabled_when].into_iter().flatten() {
            let (k, _, _) = parse_expr(expr)
                .with_context(|| format!("setting `{}`: bad expression `{expr}`", s.key))?;
            if !specs.iter().any(|o| o.key == k) {
                bail!(
                    "setting `{}`: `{expr}` refers to unknown setting `{k}`",
                    s.key
                );
            }
        }
        if let Some(d) = &s.default {
            from_json(s, d)
                .with_context(|| format!("setting `{}` has an invalid default", s.key))?;
        }
    }
    Ok(())
}

/// The value used when the user never set one.
pub fn default_value(spec: &SettingSpec) -> SettingValue {
    if let Some(v) = spec.default.as_ref().and_then(|d| from_json(spec, d).ok()) {
        return v;
    }
    match spec.kind {
        SettingKind::Toggle => SettingValue::Boolean(false),
        SettingKind::Select => SettingValue::Selected(0),
        SettingKind::Slider | SettingKind::Number => SettingValue::Number(
            spec.min
                .unwrap_or(0.0)
                .max(0.0)
                .min(spec.max.unwrap_or(f64::MAX)),
        ),
        SettingKind::Text | SettingKind::Secret => SettingValue::Text(String::new()),
        SettingKind::Color => SettingValue::Color(Color::hex(0x5865f2)),
        SettingKind::Keybind => SettingValue::Keybind(String::new()),
        SettingKind::StringList => SettingValue::Items(Vec::new()),
        SettingKind::Table => SettingValue::Rows(Vec::new()),
        SettingKind::Users | SettingKind::Channels | SettingKind::Guilds => {
            SettingValue::Ids(Vec::new())
        }
    }
}

/// A manifest default (TOML/JSON) as a typed value.
fn from_json(spec: &SettingSpec, v: &Json) -> Result<SettingValue> {
    let value = match (spec.kind, v) {
        (SettingKind::Toggle, Json::Bool(b)) => SettingValue::Boolean(*b),
        (SettingKind::Select, Json::Number(n)) => {
            SettingValue::Selected(n.as_u64().context("index")? as u32)
        }
        (SettingKind::Select, Json::String(s)) => SettingValue::Selected(
            spec.options
                .iter()
                .position(|o| o == s)
                .with_context(|| format!("`{s}` isn't an option"))? as u32,
        ),
        (SettingKind::Slider | SettingKind::Number, Json::Number(n)) => {
            SettingValue::Number(n.as_f64().context("number")?)
        }
        (SettingKind::Text | SettingKind::Secret, Json::String(s)) => SettingValue::Text(s.clone()),
        (SettingKind::Keybind, Json::String(s)) => SettingValue::Keybind(s.clone()),
        (SettingKind::Color, Json::String(s)) => {
            SettingValue::Color(Color::parse(s).with_context(|| format!("`{s}` isn't #rrggbb"))?)
        }
        (SettingKind::StringList, Json::Array(a)) => SettingValue::Items(
            a.iter()
                .map(|x| x.as_str().map(str::to_string).context("strings"))
                .collect::<Result<_>>()?,
        ),
        (SettingKind::Table, Json::Array(rows)) => SettingValue::Rows(
            rows.iter()
                .map(|r| {
                    r.as_array()
                        .context("rows are arrays")?
                        .iter()
                        .map(|c| c.as_str().map(str::to_string).context("cells are strings"))
                        .collect::<Result<Vec<_>>>()
                })
                .collect::<Result<_>>()?,
        ),
        (SettingKind::Users | SettingKind::Channels | SettingKind::Guilds, Json::Array(a)) => {
            SettingValue::Ids(
                a.iter()
                    .map(|x| match x {
                        Json::Number(n) => n.as_u64().context("id"),
                        Json::String(s) => s.parse().context("id"),
                        _ => bail!("ids are numbers or strings"),
                    })
                    .collect::<Result<_>>()?,
            )
        }
        (kind, v) => bail!("{v} doesn't fit a {kind:?} setting"),
    };
    check_value(spec, &value).map_err(anyhow::Error::msg)?;
    Ok(value)
}

/// Check a value against its spec; `Err` is the user/plugin-facing reason.
fn check_value(spec: &SettingSpec, v: &SettingValue) -> std::result::Result<(), String> {
    let in_range = |n: f64| {
        n.is_finite() && spec.min.is_none_or(|lo| n >= lo) && spec.max.is_none_or(|hi| n <= hi)
    };
    let text_ok = |t: &str| {
        t.len() <= MAX_TEXT && !t.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
    };
    let ok = match (spec.kind, v) {
        (SettingKind::Toggle, SettingValue::Boolean(_))
        | (SettingKind::Color, SettingValue::Color(_)) => true,
        (SettingKind::Select, SettingValue::Selected(i)) => (*i as usize) < spec.options.len(),
        (SettingKind::Slider | SettingKind::Number, SettingValue::Number(n)) => in_range(*n),
        (SettingKind::Text | SettingKind::Secret, SettingValue::Text(t)) => {
            text_ok(t) && (spec.multiline || !t.contains('\n'))
        }
        (SettingKind::Keybind, SettingValue::Keybind(k)) => k.len() <= 64 && text_ok(k),
        (SettingKind::StringList, SettingValue::Items(items)) => {
            items.len() <= MAX_ITEMS && items.iter().all(|i| text_ok(i))
        }
        (SettingKind::Table, SettingValue::Rows(rows)) => {
            rows.len() <= MAX_ITEMS
                && rows
                    .iter()
                    .all(|r| r.len() == spec.columns.len() && r.iter().all(|c| text_ok(c)))
        }
        (
            SettingKind::Users | SettingKind::Channels | SettingKind::Guilds,
            SettingValue::Ids(ids),
        ) => ids.len() <= MAX_ITEMS,
        _ => {
            return Err(format!(
                "setting `{}` is a {:?}; that value is the wrong kind",
                spec.key, spec.kind
            ));
        }
    };
    if ok {
        Ok(())
    } else {
        Err(format!("value out of range for setting `{}`", spec.key))
    }
}

fn parse_expr(expr: &str) -> Result<(String, bool, String)> {
    let (k, eq, v) = if let Some((k, v)) = expr.split_once("!=") {
        (k, false, v)
    } else if let Some((k, v)) = expr.split_once("==") {
        (k, true, v)
    } else {
        bail!("expected `key == value` or `key != value`");
    };
    let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
    Ok((k.trim().to_string(), eq, v))
}

/// The values of one plugin's settings, persisted next to its storage.
#[derive(Debug)]
pub struct SettingsStore {
    specs: Vec<SettingSpec>,
    values: BTreeMap<String, SettingValue>,
    file: Option<PathBuf>,
}

impl SettingsStore {
    pub fn in_memory(specs: Vec<SettingSpec>) -> Self {
        Self {
            specs,
            values: BTreeMap::new(),
            file: None,
        }
    }

    /// Load from `<dir>/<id>.settings.json` (missing or stale values fall
    /// back to defaults; a value the schema no longer accepts is dropped).
    pub fn open(dir: &std::path::Path, id: &str, specs: Vec<SettingSpec>) -> Self {
        let file = dir.join(format!("{id}.settings.json"));
        let mut values = BTreeMap::new();
        if let Ok(bytes) = std::fs::read(&file) {
            let raw: BTreeMap<String, SettingValue> =
                serde_json::from_slice(&bytes).unwrap_or_default();
            for (k, v) in raw {
                if specs
                    .iter()
                    .any(|s| s.key == k && check_value(s, &v).is_ok())
                {
                    values.insert(k, v);
                }
            }
        }
        Self {
            specs,
            values,
            file: Some(file),
        }
    }

    /// The schema, in the manifest's order.
    pub fn specs(&self) -> &[SettingSpec] {
        &self.specs
    }

    /// Replace the schema (a reload with a new manifest), keeping values
    /// that still fit.
    pub fn set_specs(&mut self, specs: Vec<SettingSpec>) {
        self.values.retain(|k, v| {
            specs
                .iter()
                .any(|s| &s.key == k && check_value(s, v).is_ok())
        });
        self.specs = specs;
    }

    /// The stored values, for [`restore`](Self::restore).
    pub fn values(&self) -> BTreeMap<String, SettingValue> {
        self.values.clone()
    }

    /// Put back values taken with [`values`](Self::values) and save them,
    /// keeping those the current schema accepts.
    pub fn restore(&mut self, values: BTreeMap<String, SettingValue>) {
        self.values = values;
        self.values.retain(|k, v| {
            self.specs
                .iter()
                .any(|s| &s.key == k && check_value(s, v).is_ok())
        });
        self.save();
    }

    fn spec(&self, key: &str) -> Option<&SettingSpec> {
        self.specs.iter().find(|s| s.key == key)
    }

    /// The value of `key`, or its default. `None` if the schema has no `key`.
    pub fn get(&self, key: &str) -> Option<SettingValue> {
        let spec = self.spec(key)?;
        Some(
            self.values
                .get(key)
                .cloned()
                .unwrap_or_else(|| default_value(spec)),
        )
    }

    /// The value of `key` as the settings page shows it: a `secret` that has
    /// a value is [`SECRET_MASK`]. `None` if the schema has no `key`.
    pub fn shown(&self, key: &str) -> Option<SettingValue> {
        let v = self.get(key)?;
        Some(match &v {
            SettingValue::Text(t)
                if !t.is_empty() && self.spec(key)?.kind == SettingKind::Secret =>
            {
                SettingValue::Text(SECRET_MASK.into())
            }
            _ => v,
        })
    }

    /// Validate and store. `Ok(true)` if the value changed.
    pub fn set(&mut self, key: &str, value: SettingValue) -> std::result::Result<bool, String> {
        let spec = self
            .spec(key)
            .ok_or_else(|| format!("no setting `{key}` in the schema"))?;
        if spec.kind == SettingKind::Secret
            && matches!(&value, SettingValue::Text(t) if t == SECRET_MASK)
        {
            return Ok(false);
        }
        check_value(spec, &value)?;
        if self.get(key).as_ref() == Some(&value) {
            return Ok(false);
        }
        self.values.insert(key.to_string(), value);
        self.save();
        Ok(true)
    }

    /// Back to the schema default. `true` if that changed the value.
    pub fn reset(&mut self, key: &str) -> bool {
        let before = self.get(key);
        if self.values.remove(key).is_some() {
            self.save();
        }
        self.get(key) != before
    }

    /// Every key the schema declares.
    pub fn keys(&self) -> Vec<String> {
        self.specs.iter().map(|s| s.key.clone()).collect()
    }

    /// Whether `key`'s `visible-when` holds (true without one).
    pub fn visible(&self, key: &str) -> bool {
        self.eval(self.spec(key).and_then(|s| s.visible_when.as_deref()))
    }

    /// Whether `key`'s `enabled-when` holds (true without one).
    pub fn enabled(&self, key: &str) -> bool {
        self.eval(self.spec(key).and_then(|s| s.enabled_when.as_deref()))
    }

    fn eval(&self, expr: Option<&str>) -> bool {
        let Some(expr) = expr else { return true };
        let Ok((k, eq, want)) = parse_expr(expr) else {
            return true;
        };
        let (Some(spec), Some(v)) = (self.spec(&k), self.get(&k)) else {
            return true;
        };
        (v.as_expr_text(spec) == want) == eq
    }

    fn save(&self) {
        if let Some(file) = &self.file
            && let Ok(json) = serde_json::to_vec_pretty(&self.values)
        {
            // The directory may not exist yet: a plugin with no storage
            // (a theme file) is the first thing to write there.
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Err(e) = std::fs::write(file, json) {
                tracing::warn!("saving {}: {e}", file.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Vec<SettingSpec> {
        parse_schema(&json!([
            {"key": "log-deletes", "type": "toggle", "label": "Keep deleted", "default": true},
            {"key": "style", "type": "select", "label": "Style", "options": ["Red", "Strike", "Ghost"], "default": "Ghost",
             "visible-when": "log-deletes == true"},
            {"key": "size", "type": "slider", "label": "Size", "min": 1, "max": 10, "default": 4},
            {"key": "rules", "type": "table", "label": "Rules", "columns": ["Find", "Replace"], "default": [["a", "b"]]},
            {"key": "ignored", "type": "users", "label": "Ignore", "default": ["123", 456]},
        ]))
        .unwrap()
    }

    #[test]
    fn defaults_validation_and_expressions() {
        let mut s = SettingsStore::in_memory(schema());
        assert_eq!(s.get("log-deletes"), Some(SettingValue::Boolean(true)));
        assert_eq!(s.get("style"), Some(SettingValue::Selected(2)));
        assert_eq!(
            s.get("rules"),
            Some(SettingValue::Rows(vec![vec!["a".into(), "b".into()]]))
        );
        assert_eq!(s.get("ignored"), Some(SettingValue::Ids(vec![123, 456])));
        assert!(s.visible("style"));
        assert_eq!(s.set("log-deletes", SettingValue::Boolean(false)), Ok(true));
        assert!(!s.visible("style"));
        assert!(s.set("size", SettingValue::Number(11.0)).is_err());
        assert!(s.set("size", SettingValue::Text("x".into())).is_err());
        assert!(
            s.set("rules", SettingValue::Rows(vec![vec!["only one".into()]]))
                .is_err()
        );
        assert!(s.set("nope", SettingValue::Boolean(true)).is_err());
        assert_eq!(
            s.set("size", SettingValue::Number(4.0)),
            Ok(false),
            "unchanged"
        );
        assert!(s.reset("log-deletes"));
    }

    #[test]
    fn secrets_are_masked_on_the_page_and_read_in_full() {
        let specs = parse_schema(&json!([
            {"key": "api-key", "type": "secret", "label": "API key"},
        ]))
        .unwrap();
        let mut s = SettingsStore::in_memory(specs);
        assert_eq!(s.shown("api-key"), Some(SettingValue::Text(String::new())));
        assert_eq!(
            s.set("api-key", SettingValue::Text("sk-123".into())),
            Ok(true)
        );
        assert_eq!(s.get("api-key"), Some(SettingValue::Text("sk-123".into())));
        assert_eq!(
            s.shown("api-key"),
            Some(SettingValue::Text(SECRET_MASK.into()))
        );
        // The page sends the mask back unchanged: the secret stays.
        assert_eq!(
            s.set("api-key", SettingValue::Text(SECRET_MASK.into())),
            Ok(false)
        );
        assert_eq!(s.get("api-key"), Some(SettingValue::Text("sk-123".into())));
        assert!(
            parse_schema(&json!([{"key": "k", "type": "secret", "label": "K", "default": "x"}]))
                .is_err()
        );
        assert!(
            parse_schema(&json!([{"key": "k", "type": "secret", "label": "K", "multiline": true}]))
                .is_err()
        );
    }

    #[test]
    fn table_form_and_bad_schemas() {
        let t = parse_schema(
            &json!({"b": {"type": "toggle", "label": "B"}, "a": {"type": "text", "label": "A"}}),
        )
        .unwrap();
        assert_eq!(
            t.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(
            parse_schema(&json!([{"key": "x", "type": "select", "label": "X"}])).is_err(),
            "no options"
        );
        assert!(
            parse_schema(&json!([{"key": "x", "type": "toggle", "label": "X", "default": 3}]))
                .is_err()
        );
        assert!(
            parse_schema(
                &json!([{"key": "x", "type": "toggle", "label": "X", "visible-when": "y == 1"}])
            )
            .is_err()
        );
        assert!(
            parse_schema(&json!([{"key": "X", "type": "toggle", "label": "X"}])).is_err(),
            "uppercase key"
        );
        assert!(
            parse_schema(&json!([{"key": "x", "type": "toggle", "label": "X", "bogus": 1}]))
                .is_err()
        );
    }
}
