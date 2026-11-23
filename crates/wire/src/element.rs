//! Element kinds: the platform view name a Create op carries.
//!
//! A kind crosses the wire as a `u16`-length UTF-8 string, not a number. The
//! names are the platform's views, not `gpui-native`'s element modules.
//! Phoenix's Android platform interns 29 of them ([`ElementKind::PLATFORM`],
//! in the order of its table) and decodes any other kind as a plain string.
//! [`ElementKind::new`] does the same.
//!
//! The guest creates nodes through one constructor (`0x1745b24`). 22 kinds
//! reach it as literals, `sheet`, `modal` and `banner` come from a
//! three-entry table, and `scroll` or `hscroll` is chosen at one call site.
//! `flow` and `stack` are compared by the guest (`0x1759430`), but where it
//! builds them isn't read.

use std::borrow::Cow;
use std::fmt;

/// The kind of a node, as its Create op names it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElementKind(Cow<'static, str>);

impl ElementKind {
    pub const VIEW: ElementKind = ElementKind::fixed("view");
    pub const TEXT: ElementKind = ElementKind::fixed("text");
    pub const STACK: ElementKind = ElementKind::fixed("stack");
    pub const IMAGE: ElementKind = ElementKind::fixed("image");
    pub const LIST: ElementKind = ElementKind::fixed("list");
    pub const SCROLL: ElementKind = ElementKind::fixed("scroll");
    pub const HSCROLL: ElementKind = ElementKind::fixed("hscroll");
    pub const FLOW: ElementKind = ElementKind::fixed("flow");
    pub const NAVIGATOR: ElementKind = ElementKind::fixed("navigator");
    /// A native text field.
    pub const INPUT: ElementKind = ElementKind::fixed("input");
    pub const SHEET: ElementKind = ElementKind::fixed("sheet");
    pub const MODAL: ElementKind = ElementKind::fixed("modal");
    pub const BANNER: ElementKind = ElementKind::fixed("banner");
    pub const POPOVER: ElementKind = ElementKind::fixed("popover");
    pub const SYSTEM_OVERLAY: ElementKind = ElementKind::fixed("systemOverlay");
    pub const PLAYER: ElementKind = ElementKind::fixed("player");
    pub const LOTTIE: ElementKind = ElementKind::fixed("lottie");
    pub const RIVE: ElementKind = ElementKind::fixed("rive");
    pub const VIDEO: ElementKind = ElementKind::fixed("video");
    pub const WEBFRAME: ElementKind = ElementKind::fixed("webframe");
    pub const ZOOM: ElementKind = ElementKind::fixed("zoom");
    pub const RING: ElementKind = ElementKind::fixed("ring");
    pub const SPINNER: ElementKind = ElementKind::fixed("spinner");
    pub const DATE_PICKER: ElementKind = ElementKind::fixed("datePicker");
    pub const MARKUP: ElementKind = ElementKind::fixed("markup");
    pub const SLIDER: ElementKind = ElementKind::fixed("slider");
    pub const VECTOR_ANIM: ElementKind = ElementKind::fixed("vector_anim");
    pub const SWIPE_FILL: ElementKind = ElementKind::fixed("swipeFill");
    pub const CANVAS: ElementKind = ElementKind::fixed("canvas");

    /// The kinds the Android platform interns, in its table's order.
    pub const PLATFORM: [ElementKind; 29] = [
        ElementKind::VIEW,
        ElementKind::TEXT,
        ElementKind::STACK,
        ElementKind::IMAGE,
        ElementKind::LIST,
        ElementKind::SCROLL,
        ElementKind::HSCROLL,
        ElementKind::FLOW,
        ElementKind::NAVIGATOR,
        ElementKind::INPUT,
        ElementKind::SHEET,
        ElementKind::MODAL,
        ElementKind::BANNER,
        ElementKind::POPOVER,
        ElementKind::SYSTEM_OVERLAY,
        ElementKind::PLAYER,
        ElementKind::LOTTIE,
        ElementKind::RIVE,
        ElementKind::VIDEO,
        ElementKind::WEBFRAME,
        ElementKind::ZOOM,
        ElementKind::RING,
        ElementKind::SPINNER,
        ElementKind::DATE_PICKER,
        ElementKind::MARKUP,
        ElementKind::SLIDER,
        ElementKind::VECTOR_ANIM,
        ElementKind::SWIPE_FILL,
        ElementKind::CANVAS,
    ];

    const fn fixed(name: &'static str) -> ElementKind {
        ElementKind(Cow::Borrowed(name))
    }

    /// A kind by name. Any string is a kind. A platform kind comes back as
    /// its entry in [`ElementKind::PLATFORM`].
    pub fn new(name: impl Into<String>) -> ElementKind {
        let name = name.into();
        match ElementKind::PLATFORM.iter().find(|k| k.0 == name.as_str()) {
            Some(k) => k.clone(),
            None => ElementKind(Cow::Owned(name)),
        }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.0
    }

    /// The kind's index in the platform's table, or `None` for a kind the
    /// platform decodes as a plain string.
    #[must_use]
    pub fn platform_index(&self) -> Option<usize> {
        ElementKind::PLATFORM.iter().position(|k| k.0 == self.0)
    }
}

impl fmt::Display for ElementKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ElementKind {
    fn from(name: &str) -> Self {
        ElementKind::new(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_platform_table_is_in_its_order_and_any_other_kind_is_a_string() {
        let names: Vec<String> = ElementKind::PLATFORM.iter().map(ToString::to_string).collect();
        assert_eq!(
            names.join(" "),
            "view text stack image list scroll hscroll flow navigator input sheet modal \
             banner popover systemOverlay player lottie rive video webframe zoom ring \
             spinner datePicker markup slider vector_anim swipeFill canvas"
        );
        assert_eq!(ElementKind::new("input").platform_index(), Some(9));
        assert!(matches!(ElementKind::new("canvas").0, Cow::Borrowed(_)));
        let div = ElementKind::new("div");
        assert_eq!((div.name(), div.platform_index()), ("div", None));
    }
}
