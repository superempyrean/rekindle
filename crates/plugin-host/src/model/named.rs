//! The client's own elements by name (`elements` in the WIT): what each
//! plugin did to them, as a backend keeps it.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{Color, Element, UiError};

/// How a plugin restyles one of the client's elements. A field left `None`
/// keeps the client's own value.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ElementStyle {
    pub background: Option<Color>,
    /// Text and icons.
    pub foreground: Option<Color>,
    pub border: Option<Color>,
    /// 0 to 1.
    pub opacity: Option<f32>,
    pub radius: Option<f32>,
}

impl ElementStyle {
    pub fn is_empty(&self) -> bool {
        *self == ElementStyle::default()
    }
}

/// What a press on one of the client's elements does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PressMode {
    /// The client receives the press, and the plugin's `on-ui` does too.
    Observe,
    /// Only the plugin's `on-ui` receives it.
    Consume,
    /// The client receives it as a press on this other element.
    Retarget(String),
}

/// Where an insert goes, next to the element it names or inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Place {
    Before,
    After,
    /// After the element's own children.
    Inside,
}

/// Elements of a plugin's own, drawn next to or inside one of the client's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElementInsert {
    /// The plugin's id for it. Presses in it reach `on-ui` with surface
    /// `insert:<id>`.
    pub id: String,
    /// The client's element it goes next to or inside.
    pub name: String,
    pub place: Place,
    pub elements: Vec<Element>,
}

/// One call of the `elements` interface, checked by the runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ElementOp {
    Hide { name: String },
    Style { name: String, style: ElementStyle },
    OnPress { name: String, mode: PressMode },
    Insert(ElementInsert),
    Remove { id: String },
    Reset { name: String },
}

/// What one plugin did to the client's elements.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginElements {
    pub hidden: BTreeSet<String>,
    pub styles: BTreeMap<String, ElementStyle>,
    pub presses: BTreeMap<String, PressMode>,
    /// In the order they were first inserted.
    pub inserts: Vec<ElementInsert>,
}

impl PluginElements {
    pub fn is_empty(&self) -> bool {
        self.hidden.is_empty()
            && self.styles.is_empty()
            && self.presses.is_empty()
            && self.inserts.is_empty()
    }
}

/// The most inserts one plugin keeps at once.
pub const MAX_INSERTS: usize = 16;
/// The most elements one insert holds.
pub const MAX_INSERT_ELEMENTS: usize = 32;

/// The client's elements this build names, and what each plugin did to
/// them. A backend that reaches the client's elements holds one and hands
/// it every [`ElementOp`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NamedElements {
    /// The names, sorted. Empty when the build names none.
    pub available: Vec<String>,
    /// The selector of each name, as JSON, where the backend knows it.
    pub selectors: BTreeMap<String, String>,
    /// By plugin.
    pub plugins: BTreeMap<String, PluginElements>,
    /// The backend runs plugins' own work (`wire-raw`).
    pub raw_available: bool,
    /// Each `wire-raw` plugin's work, checked by [`raw_work`](super::raw_work).
    pub raw: BTreeMap<String, rekindle_wire_inject::Work>,
}

impl NamedElements {
    pub fn new(mut available: Vec<String>) -> Self {
        available.sort();
        available.dedup();
        NamedElements {
            available,
            ..NamedElements::default()
        }
    }

    /// The names a wire map gives, with their selectors as JSON. The
    /// backend runs plugins' own work too.
    pub fn with_selectors(selectors: BTreeMap<String, String>) -> Self {
        NamedElements {
            available: selectors.keys().cloned().collect(),
            selectors,
            raw_available: true,
            ..NamedElements::default()
        }
    }

    /// Replace `plugin`'s own work, or drop it with `None`.
    pub fn set_raw(
        &mut self,
        plugin: &str,
        work: Option<rekindle_wire_inject::Work>,
    ) -> Result<(), UiError> {
        if !self.raw_available {
            return Err(UiError::Unavailable);
        }
        match work {
            Some(w) if !w.is_empty() => {
                self.raw.insert(plugin.to_string(), w);
            }
            _ => {
                self.raw.remove(plugin);
            }
        }
        Ok(())
    }

    fn has(&self, name: &str) -> bool {
        self.available.binary_search_by(|n| n.as_str().cmp(name)).is_ok()
    }

    /// Apply `plugin`'s `op`. A name this build doesn't have is
    /// `unavailable`, a retarget at the element itself `invalid`, and an
    /// insert past [`MAX_INSERTS`] or [`MAX_INSERT_ELEMENTS`] `limit`.
    pub fn apply(&mut self, plugin: &str, op: ElementOp) -> Result<(), UiError> {
        let name = match &op {
            ElementOp::Hide { name }
            | ElementOp::Style { name, .. }
            | ElementOp::OnPress { name, .. }
            | ElementOp::Reset { name } => Some(name.as_str()),
            ElementOp::Insert(i) => Some(i.name.as_str()),
            ElementOp::Remove { .. } => None,
        };
        if name.is_some_and(|n| !self.has(n)) {
            return Err(UiError::Unavailable);
        }
        if let ElementOp::OnPress {
            name,
            mode: PressMode::Retarget(to),
        } = &op
        {
            if !self.has(to) {
                return Err(UiError::Unavailable);
            }
            if to == name {
                return Err(UiError::Invalid);
            }
        }
        let mine = self.plugins.entry(plugin.to_string()).or_default();
        match op {
            ElementOp::Hide { name } => {
                mine.hidden.insert(name);
            }
            ElementOp::Style { name, style } => {
                if style.is_empty() {
                    mine.styles.remove(&name);
                } else {
                    mine.styles.insert(name, style);
                }
            }
            ElementOp::OnPress { name, mode } => {
                mine.presses.insert(name, mode);
            }
            ElementOp::Insert(insert) => {
                if insert.elements.len() > MAX_INSERT_ELEMENTS {
                    return Err(UiError::Limit);
                }
                if let Some(old) = mine.inserts.iter_mut().find(|i| i.id == insert.id) {
                    *old = insert;
                } else if mine.inserts.len() >= MAX_INSERTS {
                    return Err(UiError::Limit);
                } else {
                    mine.inserts.push(insert);
                }
            }
            ElementOp::Remove { id } => mine.inserts.retain(|i| i.id != id),
            ElementOp::Reset { name } => {
                mine.hidden.remove(&name);
                mine.styles.remove(&name);
                mine.presses.remove(&name);
            }
        }
        self.plugins.retain(|_, p| !p.is_empty());
        Ok(())
    }

    pub fn plugin_unloaded(&mut self, plugin: &str) {
        self.plugins.remove(plugin);
        self.raw.remove(plugin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> NamedElements {
        NamedElements::new(vec!["composer.gift".into(), "guild-search".into()])
    }

    #[test]
    fn names_the_build_lacks_are_unavailable() {
        let mut s = state();
        let hide = |name: &str| ElementOp::Hide { name: name.into() };
        assert_eq!(s.apply("p", hide("composer.gift")), Ok(()));
        assert_eq!(s.apply("p", hide("nope")), Err(UiError::Unavailable));
        assert!(s.plugins["p"].hidden.contains("composer.gift"));
        let retarget = |to: &str| ElementOp::OnPress {
            name: "composer.gift".into(),
            mode: PressMode::Retarget(to.into()),
        };
        assert_eq!(s.apply("p", retarget("nope")), Err(UiError::Unavailable));
        assert_eq!(s.apply("p", retarget("composer.gift")), Err(UiError::Invalid));
        assert_eq!(s.apply("p", retarget("guild-search")), Ok(()));
        assert_eq!(
            NamedElements::default().apply("p", hide("composer.gift")),
            Err(UiError::Unavailable)
        );
    }

    #[test]
    fn reset_undoes_one_name_and_unloading_everything() {
        let mut s = state();
        let name = || "guild-search".to_string();
        s.apply("p", ElementOp::Hide { name: name() }).unwrap();
        s.apply(
            "p",
            ElementOp::Style {
                name: name(),
                style: ElementStyle {
                    opacity: Some(0.5),
                    ..ElementStyle::default()
                },
            },
        )
        .unwrap();
        s.apply("p", ElementOp::Reset { name: name() }).unwrap();
        assert!(s.plugins.is_empty());
        s.apply("p", ElementOp::Hide { name: name() }).unwrap();
        s.apply("q", ElementOp::Hide { name: name() }).unwrap();
        s.plugin_unloaded("p");
        assert_eq!(s.plugins.keys().collect::<Vec<_>>(), ["q"]);
    }

    #[test]
    fn an_insert_is_replaced_by_its_id_and_limited() {
        let mut s = state();
        let insert = |id: &str, n: usize| {
            ElementOp::Insert(ElementInsert {
                id: id.into(),
                name: "guild-search".into(),
                place: Place::After,
                elements: vec![
                    Element::Text {
                        text: "x".into()
                    };
                    n
                ],
            })
        };
        s.apply("p", insert("a", 1)).unwrap();
        s.apply("p", insert("a", 2)).unwrap();
        assert_eq!(s.plugins["p"].inserts.len(), 1);
        assert_eq!(s.plugins["p"].inserts[0].elements.len(), 2);
        assert_eq!(
            s.apply("p", insert("b", MAX_INSERT_ELEMENTS + 1)),
            Err(UiError::Limit)
        );
        for i in 1..MAX_INSERTS {
            s.apply("p", insert(&format!("i{i}"), 1)).unwrap();
        }
        assert_eq!(s.apply("p", insert("over", 1)), Err(UiError::Limit));
        s.apply("p", ElementOp::Remove { id: "a".into() }).unwrap();
        assert_eq!(s.plugins["p"].inserts.len(), MAX_INSERTS - 1);
    }
}
