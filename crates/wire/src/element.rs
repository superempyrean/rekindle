//! The fixed element set. Kinds are interned name strings, not numeric opcodes.
//!
//! RECOVERED: the host renders a fixed, small set of primitives, and a node
//! carries its kind as an interned name string (the reconcile path compares a
//! 4-byte field against `"list"`). The guest cannot define a new native
//! element. The set is read from the module paths
//! (`crates/gpui-native/src/elements/*.rs`): `canvas`, `div`, `input`, `list`,
//! `motion_events`, `navigator`, `text`. `navigator` and `motion_events` are new
//! versus upstream gpui; `list` is far richer than `uniform_list`.

/// One of Phoenix's fixed host-rendered primitives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ElementKind {
    Canvas,
    Div,
    Input,
    List,
    MotionEvents,
    Navigator,
    Text,
}

impl ElementKind {
    /// Every kind, so a renderer can assert it handles the whole fixed set.
    pub const ALL: [ElementKind; 7] = [
        ElementKind::Canvas,
        ElementKind::Div,
        ElementKind::Input,
        ElementKind::List,
        ElementKind::MotionEvents,
        ElementKind::Navigator,
        ElementKind::Text,
    ];

    /// The interned name string the host keys on.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            ElementKind::Canvas => "canvas",
            ElementKind::Div => "div",
            ElementKind::Input => "input",
            ElementKind::List => "list",
            ElementKind::MotionEvents => "motion_events",
            ElementKind::Navigator => "navigator",
            ElementKind::Text => "text",
        }
    }

    /// Parse a kind from its name. `None` for any name outside the fixed set —
    /// the guest cannot invent a new element.
    #[must_use]
    pub fn from_name(name: &str) -> Option<ElementKind> {
        Some(match name {
            "canvas" => ElementKind::Canvas,
            "div" => ElementKind::Div,
            "input" => ElementKind::Input,
            "list" => ElementKind::List,
            "motion_events" => ElementKind::MotionEvents,
            "navigator" => ElementKind::Navigator,
            "text" => ElementKind::Text,
            _ => return None,
        })
    }
}
