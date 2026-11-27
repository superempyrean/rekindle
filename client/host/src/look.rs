//! A look: the cells each part of rekindle's rows gets on a client that is
//! split across a wire, read from the rows the client's guest builds.
//!
//! The wire hosts mount rows with these cells as they are (`rows`). A
//! client drawn in-process has no cells to write, and reads the numbers
//! and colours of the same look with [`Look::number`], [`Look::colour`]
//! and [`Look::edges`], so its rows can't drift from the phones'.

use std::collections::BTreeMap;

use rekindle_wire::{PropId, Value};
use rekindle_wire_inject::{Cell, Selector};
use serde::Deserialize;

/// The cells each part of a row gets, and where the menu, pages and
/// modals go. Read from the client's own rows, so a mount looks like them.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Look {
    /// The node the menu is mounted under.
    pub under: Selector,
    /// The child of that node the menu goes before, if not last.
    #[serde(default)]
    pub before: Option<Selector>,
    /// The child of that node the menu goes after, if `before` finds none.
    #[serde(default)]
    pub after: Option<Selector>,
    /// The node a page or modal is mounted under: one whose children are
    /// drawn over each other, so the last is on top. Without one, pages and
    /// modals aren't shown.
    #[serde(default)]
    pub modal_under: Option<Selector>,
    /// The node a page is mounted under instead, when the client shows its
    /// settings screen in a layer of its own that `modal_under` is behind,
    /// as iOS shows a sheet. Pages open from the settings screen.
    #[serde(default)]
    pub page_under: Option<Selector>,
    /// A group's header with the group under it.
    pub section: Vec<Cell>,
    /// A group's header text.
    pub header: Vec<Cell>,
    /// A group of rows.
    pub group: Vec<Cell>,
    /// A row, and what its group's first and last rows add.
    pub row: Vec<Cell>,
    #[serde(default)]
    pub first: Vec<Cell>,
    #[serde(default)]
    pub last: Vec<Cell>,
    /// The row's content: icon, the title column and what ends the row.
    pub inner: Vec<Cell>,
    pub icon: Vec<Cell>,
    pub column: Vec<Cell>,
    pub title: Vec<Cell>,
    pub subtitle: Vec<Cell>,
    /// A text field under the title. Without these it takes the title's
    /// cells, and its placeholder the subtitle's colour.
    #[serde(default)]
    pub input: Vec<Cell>,
    pub chevron_box: Vec<Cell>,
    pub chevron: Vec<Cell>,
    /// A switch: its box, its track and thumb, and what each adds when on
    /// and when off.
    pub switch_box: Vec<Cell>,
    pub track: Vec<Cell>,
    pub track_on: Vec<Cell>,
    pub track_off: Vec<Cell>,
    /// What a track adds when a press on it does something of its own.
    #[serde(default)]
    pub track_press: Vec<Cell>,
    pub thumb: Vec<Cell>,
    pub thumb_on: Vec<Cell>,
    pub thumb_off: Vec<Cell>,
    /// Between two rows: a box holding a rule.
    pub divider: Vec<Cell>,
    pub rule: Vec<Cell>,
    /// A modal: the layer over the screen, which lays the card out, and the
    /// card on it.
    #[serde(default)]
    pub scrim: Vec<Cell>,
    #[serde(default)]
    pub card: Vec<Cell>,
    /// A page: the layer over the screen, its bar, the bar's back button
    /// and its icon, the title, the scroll and the column in it.
    #[serde(default)]
    pub page: Vec<Cell>,
    #[serde(default)]
    pub page_bar: Vec<Cell>,
    #[serde(default)]
    pub page_back: Vec<Cell>,
    #[serde(default)]
    pub page_back_icon: Vec<Cell>,
    #[serde(default)]
    pub page_title: Vec<Cell>,
    #[serde(default)]
    pub page_scroll: Vec<Cell>,
    #[serde(default)]
    pub page_content: Vec<Cell>,
    /// The toasts: the layer over the screen that lays them out, takes no
    /// press and lets one through to the screen, a toast's card, its text,
    /// and the bar at its start, by style (`info`, `success`, `warning`,
    /// `error`). Without a layer, toasts aren't shown.
    #[serde(default)]
    pub toast_layer: Vec<Cell>,
    #[serde(default)]
    pub toast: Vec<Cell>,
    #[serde(default)]
    pub toast_text: Vec<Cell>,
    #[serde(default)]
    pub toast_bar: BTreeMap<String, Vec<Cell>>,
    /// The `uri` of each row's icon, by the row's key: the key itself, then
    /// `<verb>:*` for a key `<verb>:<id>`. In the menu, `*` is the icon of a
    /// row with neither. A row with none has no icon.
    #[serde(default)]
    pub icons: BTreeMap<String, String>,
}

impl Look {
    pub fn from_json(json: &str) -> anyhow::Result<Look> {
        Ok(serde_json::from_str(json)?)
    }
}

/// The value `cells` give `prop`, the last one if more than one does.
fn value<'a>(cells: &'a [Cell], prop: &str) -> Option<&'a Value> {
    let id = PropId::from_name(prop)?;
    cells.iter().rev().find(|c| c.prop == id).map(|c| &c.value)
}

impl Look {
    /// The number `cells` give `prop`, an `f64` or an integer cell.
    #[must_use]
    pub fn number(cells: &[Cell], prop: &str) -> Option<f32> {
        match value(cells, prop)? {
            Value::F64(v) => Some(*v as f32),
            v => v.as_int().map(|v| v as f32),
        }
    }

    /// The colour `cells` give `prop`, `0xAARRGGBB`.
    #[must_use]
    pub fn colour(cells: &[Cell], prop: &str) -> Option<u32> {
        value(cells, prop)?.as_color()
    }

    /// The edge insets `cells` give `prop` (`padding`, `margin`): left,
    /// top, right and bottom, as the wire orders them.
    #[must_use]
    pub fn edges(cells: &[Cell], prop: &str) -> Option<[f32; 4]> {
        match value(cells, prop)? {
            Value::Quad(q) => Some(*q),
            _ => None,
        }
    }

    /// The string `cells` give `prop`, such as an image's `uri`.
    #[must_use]
    pub fn text<'a>(cells: &'a [Cell], prop: &str) -> Option<&'a str> {
        match value(cells, prop)? {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The `uri` of the icon of a row with this key: the key's own, then
    /// `<verb>:*` for a key `<verb>:<id>`, then `*` if `fallback` is set.
    #[must_use]
    pub fn icon(&self, key: &str, fallback: bool) -> Option<&str> {
        let icons = &self.icons;
        icons
            .get(key)
            .or_else(|| {
                let (verb, _) = key.split_once(':')?;
                icons.get(&format!("{verb}:*"))
            })
            .or_else(|| fallback.then(|| icons.get("*")).flatten())
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_looks_numbers_colours_and_edges_read_back() {
        let cells: Vec<Cell> = serde_json::from_str(
            r#"[{"prop": "fontSize", "value": {"f64": 12.0}},
                {"prop": "fontWeight", "value": {"i32": 500}},
                {"prop": "background", "value": {"rgba": "97979f14"}},
                {"prop": "padding", "value": {"quad": [56, 0, 0, 0]}}]"#,
        )
        .unwrap();
        assert_eq!(Look::number(&cells, "fontSize"), Some(12.0));
        assert_eq!(Look::number(&cells, "fontWeight"), Some(500.0));
        assert_eq!(Look::colour(&cells, "background"), Some(0x1497_979f));
        assert_eq!(Look::edges(&cells, "padding"), Some([56.0, 0.0, 0.0, 0.0]));
        assert_eq!(Look::number(&cells, "minHeight"), None);
        let uri: Vec<Cell> =
            serde_json::from_str(r#"[{"prop": "uri", "value": {"str": "res://back"}}]"#).unwrap();
        assert_eq!(Look::text(&uri, "uri"), Some("res://back"));
        assert_eq!(Look::text(&cells, "fontSize"), None);
    }

    #[test]
    fn a_rows_icon_is_its_keys_then_its_verbs_then_the_fallback() {
        let look = crate::rows::test_look();
        assert_eq!(look.icon("developer-mode", false), Some("res://warning"));
        assert_eq!(look.icon("open:x", false), Some("res://card"));
        assert_eq!(look.icon("b", false), None);
        assert_eq!(look.icon("b", true), Some("res://any"));
    }
}
