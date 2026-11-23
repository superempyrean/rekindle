//! Element kinds: the platform view name a Create op carries.
//!
//! A kind crosses the wire as a `u16`-length UTF-8 string, not a number. The
//! names are the platform's views, not `gpui-native`'s element modules:
//! `0x1759430` compares a node's kind against `view`, `flow`, `text`, `ring`,
//! `stack`, `image` and `spinner`, and the reconcile at `0x17367d8` against
//! `list`. The guest's app code chooses the kinds, and the full set isn't read
//! yet, so a decoder takes any string.

use std::borrow::Cow;
use std::fmt;

/// The kind of a node, as its Create op names it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElementKind(Cow<'static, str>);

impl ElementKind {
    pub const VIEW: ElementKind = ElementKind::fixed("view");
    pub const FLOW: ElementKind = ElementKind::fixed("flow");
    pub const TEXT: ElementKind = ElementKind::fixed("text");
    pub const RING: ElementKind = ElementKind::fixed("ring");
    pub const STACK: ElementKind = ElementKind::fixed("stack");
    pub const IMAGE: ElementKind = ElementKind::fixed("image");
    pub const SPINNER: ElementKind = ElementKind::fixed("spinner");
    pub const LIST: ElementKind = ElementKind::fixed("list");
    /// A text field. Not among the kinds `gpui-native` compares against,
    /// though its `elements/input.rs` exists, so the name is the mock's.
    pub const INPUT: ElementKind = ElementKind::fixed("input");

    /// The kinds read from `gpui-native`'s comparisons, in the order above.
    pub const SEEN: [ElementKind; 8] = [
        ElementKind::VIEW,
        ElementKind::FLOW,
        ElementKind::TEXT,
        ElementKind::RING,
        ElementKind::STACK,
        ElementKind::IMAGE,
        ElementKind::SPINNER,
        ElementKind::LIST,
    ];

    const fn fixed(name: &'static str) -> ElementKind {
        ElementKind(Cow::Borrowed(name))
    }

    /// A kind by name. Any string is a kind.
    pub fn new(name: impl Into<String>) -> ElementKind {
        ElementKind(Cow::Owned(name.into()))
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.0
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
