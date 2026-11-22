//! The layout file: the offsets, sizes, enum tags and theme colours
//! recovered for the client build, checked against the build's own code
//! (operand anchors), and the palette for framework theming.

use std::collections::BTreeMap;

use rekindle_install::env;

use crate::hooks::Hooks;

/// `{"themes": {"dark": {"background-base-low": "#313338", ...}, ...}}`.
pub(crate) fn load_palette() -> Vec<(String, Vec<(String, u32)>)> {
    let Ok(path) = std::env::var(env::PALETTE) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let tokens = |m: &serde_json::Map<String, serde_json::Value>| -> Vec<(String, u32)> {
        m.iter()
            .filter_map(|(k, hex)| {
                let h = hex.as_str()?.trim_start_matches('#');
                u32::from_str_radix(h, 16).ok().map(|n| (k.clone(), n))
            })
            .collect()
    };
    v.get("themes")
        .and_then(|t| t.as_object())
        .map(|themes| {
            themes
                .iter()
                .filter_map(|(name, m)| Some((name.clone(), tokens(m.as_object()?))))
                .collect()
        })
        .unwrap_or_default()
}

/// The recovered layout, as the analysis crates emit them. No client types: a
/// signature is a `rekindle-sigmatch` masked pattern, an offset/size a
/// recovered number.
#[derive(Debug, Default)]
pub(crate) struct Layout {
    /// name -> recovered byte offset.
    pub(crate) offsets: BTreeMap<String, usize>,
    /// name -> byte size (the recovery probe, for enums returned by value).
    pub(crate) sizes: BTreeMap<String, usize>,
    /// name -> hex bytes (the recovery probe, for an enum-tag template).
    pub(crate) templates: BTreeMap<String, String>,
    /// name -> enum tag value (the recovery, for enums rekindle reads or writes).
    pub(crate) tags: BTreeMap<String, usize>,
    /// theme token -> absolute byte offset within the theme value.
    pub(crate) colors: BTreeMap<String, usize>,
}

impl Layout {
    /// The offset `name`, or an error naming it.
    pub(crate) fn off(&self, name: &str) -> anyhow::Result<usize> {
        self.offsets
            .get(name)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("no offset {name:?}"))
    }
    /// The size `name`, or an error naming it.
    pub(crate) fn size(&self, name: &str) -> anyhow::Result<usize> {
        self.sizes
            .get(name)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("no size {name:?}"))
    }
    /// The enum tag `name`, or an error naming it.
    pub(crate) fn tag(&self, name: &str) -> anyhow::Result<u64> {
        self.tags
            .get(name)
            .map(|&t| t as u64)
            .ok_or_else(|| anyhow::anyhow!("no tag {name:?}"))
    }
    /// The template `name` as bytes, or an error naming it.
    pub(crate) fn template(&self, name: &str) -> anyhow::Result<Vec<u8>> {
        let hex = self
            .templates
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("no template {name:?}"))?;
        let bad = || anyhow::anyhow!("template {name:?} isn't whole bytes of hex");
        if !hex.len().is_multiple_of(2) {
            return Err(bad());
        }
        hex.as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|d| u8::from_str_radix(d, 16).ok())
                    .ok_or_else(bad)
            })
            .collect()
    }

    /// The entry an operand anchor `layout:<section>:<key>` checks. For a
    /// template, its first qword: the tag (or niche) its variant is told
    /// apart by.
    pub(crate) fn entry(&self, section: &str, key: &str) -> Option<i64> {
        match section {
            "offsets" => self.offsets.get(key).map(|&s| s as i64),
            "sizes" => self.sizes.get(key).map(|&s| s as i64),
            "tags" => self.tags.get(key).map(|&s| s as i64),
            "templates" => self
                .template(key)
                .ok()
                .and_then(|t| Some(i64::from_le_bytes(t.get(..8)?.try_into().ok()?))),
            _ => None,
        }
    }

    /// Replace an entry with the value this build's code carries. False if it
    /// can't be (an unknown section, a negative value, a template too short).
    pub(crate) fn set_entry(&mut self, section: &str, key: &str, value: i64) -> bool {
        let Ok(v) = usize::try_from(value) else {
            return false;
        };
        let map = match section {
            "offsets" => &mut self.offsets,
            "sizes" => &mut self.sizes,
            "tags" => &mut self.tags,
            "templates" => {
                let Ok(mut t) = self.template(key) else {
                    return false;
                };
                if t.len() < 8 {
                    return false;
                }
                t[..8].copy_from_slice(&value.to_le_bytes());
                self.templates.insert(key.into(), crate::hex(&t));
                return true;
            }
            _ => return false,
        };
        map.insert(key.into(), v);
        true
    }

    /// Forget an entry, so whatever needs it stays off.
    pub(crate) fn remove_entry(&mut self, section: &str, key: &str) {
        match section {
            "offsets" => drop(self.offsets.remove(key)),
            "sizes" => drop(self.sizes.remove(key)),
            "tags" => drop(self.tags.remove(key)),
            "templates" => drop(self.templates.remove(key)),
            _ => {}
        }
    }

    /// Read the layout file: its `offsets`, `sizes`, `tags` and `colors`
    /// objects (name -> number) and its `templates` (name -> hex). An entry
    /// of the wrong type is left out.
    pub(crate) fn load(path: &str) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let v: serde_json::Value = serde_json::from_str(&text)?;
        let str_map = |key: &str| {
            let mut m = BTreeMap::new();
            if let Some(obj) = v.get(key).and_then(|s| s.as_object()) {
                for (k, val) in obj {
                    if let Some(s) = val.as_str() {
                        m.insert(k.clone(), s.to_string());
                    }
                }
            }
            m
        };
        let num_map = |key: &str| {
            let mut m = BTreeMap::new();
            if let Some(obj) = v.get(key).and_then(|s| s.as_object()) {
                for (k, val) in obj {
                    if let Some(n) = val.as_u64() {
                        m.insert(k.clone(), n as usize);
                    }
                }
            }
            m
        };
        Ok(Layout {
            offsets: num_map("offsets"),
            sizes: num_map("sizes"),
            templates: str_map("templates"),
            tags: num_map("tags"),
            colors: num_map("colors"),
        })
    }
}

/// The layout file checked against this build's own instructions (operand
/// anchors `layout:<section>:<key>`).
#[derive(Default)]
pub(crate) struct Reconciled {
    pub(crate) agreed: usize,
    pub(crate) total: usize,
    /// Entries the code carries a different value for, with at least two of
    /// the anchor's locators agreeing on it (`AnchorResult::corroborated`).
    /// The layout now has the code's value.
    pub(crate) corrected: Vec<String>,
    /// Entries that neither the file nor the code settles: the anchor didn't
    /// resolve, or one instruction disagrees with the file and nothing
    /// backs it up. They're removed from the layout, and whatever needs one
    /// stays off.
    pub(crate) unknown: Vec<String>,
}

/// Check the layout against this build's own instructions (operand
/// anchors). An entry the code corrects takes the code's value, and one
/// that nothing settles is removed, which turns off whatever needs it.
pub(crate) fn reconcile_layout(layout: &mut Layout, hooks: &Hooks) -> Reconciled {
    let mut r = Reconciled::default();
    for a in &hooks.report.anchors {
        let Some(rest) = a.name.strip_prefix("layout:") else {
            continue;
        };
        let (section, key) = rest.split_once(':').unwrap_or(("offsets", rest));
        r.total += 1;
        let stored = layout.entry(section, key);
        let was = stored.map_or("absent".to_string(), |s| s.to_string());
        match (hooks.values.get(&a.name), stored) {
            (Some(Ok(v)), Some(s)) if *v == s => r.agreed += 1,
            (Some(Ok(v)), _) if a.corroborated() && layout.set_entry(section, key, *v) => {
                r.corrected.push(format!("{key}:{was}->{v}"));
            }
            (Some(Ok(v)), _) => {
                layout.remove_entry(section, key);
                r.unknown.push(format!("{key}:{was}->{v}?"));
            }
            (Some(Err(e)), _) => {
                layout.remove_entry(section, key);
                r.unknown.push(format!("{key}:{e}"));
            }
            (None, _) => {}
        }
    }
    r
}
