//! Accessibility as `PropId` cells, and the semantic tree a platform builds
//! from them.
//!
//! RECOVERED (from `liblibdiscord_wasm_host.so` build 356207 and the APK's
//! DEX strings): Phoenix has
//! no accessibility tree of its own and no accessibility export. The retained
//! element keeps accessibility as typed fields, and one `gpui-native`
//! function (`0x174a4a0`) flattens them into the view's `PropId` record next to
//! its visual cells. The Android layer reads those props and builds its
//! `AccessibilityNodeInfo`s. Recovered here:
//!
//! - the ids in [`ids`] and the internal value kind of each (bool, string,
//!   number);
//! - flags (`accessibilityHidden`, `accessible`, ...) are written only when
//!   true, and tri-states (`accessibilitySelected`, `accessibilityChecked`,
//!   `accessibilityExpanded`, `accessibilityDisabled`, `accessibilityBusy`)
//!   are skipped when unset and written as a bool otherwise;
//! - `role` (414) is a string from a closed enum of 22, [`Role`], indexed by
//!   the element's role byte (`0xff` is none);
//! - `liveRegion` (281) is `"polite"` or `"assertive"`, [`LiveRegion`].
//!
//! MODELLED: the wire value each kind travels as (a bool as tag 1/2, a string
//! as tag 6, a number as tag 4), since this crate leaves `PropIds` untyped
//! (see `prop_id.rs`); and [`semantics`], the walk from cells to a tree. The
//! platform's tree builder is obfuscated Kotlin. Its const-string tables show
//! which props it reads, not how it merges children, so the rules in
//! [`semantics`] are the usual ones (a hidden node drops its subtree, a node
//! with nothing to say is skipped and its children move up, a label can come
//! from `accessibilityLabelledBy`) and rekindle's model.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::{NodeId, PropId, Tree, Value};

/// The accessibility `PropIds`, by their recovered names.
pub mod ids {
    use crate::PropId;

    /// A string from [`Role`](super::Role).
    pub const ROLE: PropId = PropId(414);
    pub const LABEL: PropId = PropId(11);
    pub const HINT: PropId = PropId(8);
    /// The target of another node's [`LABELLED_BY`].
    pub const ID: PropId = PropId(9);
    pub const LABELLED_BY: PropId = PropId(722);
    pub const TEXT: PropId = PropId(15);
    pub const ROLE_DESCRIPTION: PropId = PropId(695);
    /// `"polite"` or `"assertive"`.
    pub const LIVE_REGION: PropId = PropId(281);
    /// Flags: written only when true.
    pub const HIDDEN: PropId = PropId(7);
    pub const ACCESSIBLE: PropId = PropId(17);
    pub const KEEPS_CHILDREN: PropId = PropId(10);
    pub const NAMED_BY_CHILDREN: PropId = PropId(777);
    pub const MODAL: PropId = PropId(12);
    /// Tri-states: absent, or a bool.
    pub const SELECTED: PropId = PropId(13);
    pub const CHECKED: PropId = PropId(3);
    pub const EXPANDED: PropId = PropId(5);
    pub const DISABLED: PropId = PropId(4);
    pub const BUSY: PropId = PropId(2);
}

/// The role enum `gpui-native` writes into `role` (414), in the order of the
/// element's role byte. These are React Native's `AccessibilityRole` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    Button,
    Header,
    Summary,
    Link,
    Tab,
    Image,
    Switch,
    Checkbox,
    Radio,
    ToggleButton,
    Search,
    ProgressBar,
    List,
    MenuItem,
    Alert,
    RadioGroup,
    KeyboardKey,
    ImageButton,
    Timer,
    Adjustable,
    Text,
    None,
}

impl Role {
    /// Every role, indexed by the element's role byte.
    pub const ALL: [Role; 22] = [
        Role::Button,
        Role::Header,
        Role::Summary,
        Role::Link,
        Role::Tab,
        Role::Image,
        Role::Switch,
        Role::Checkbox,
        Role::Radio,
        Role::ToggleButton,
        Role::Search,
        Role::ProgressBar,
        Role::List,
        Role::MenuItem,
        Role::Alert,
        Role::RadioGroup,
        Role::KeyboardKey,
        Role::ImageButton,
        Role::Timer,
        Role::Adjustable,
        Role::Text,
        Role::None,
    ];

    /// The string the host writes.
    pub fn name(self) -> &'static str {
        match self {
            Role::Button => "button",
            Role::Header => "header",
            Role::Summary => "summary",
            Role::Link => "link",
            Role::Tab => "tab",
            Role::Image => "image",
            Role::Switch => "switch",
            Role::Checkbox => "checkbox",
            Role::Radio => "radio",
            Role::ToggleButton => "togglebutton",
            Role::Search => "search",
            Role::ProgressBar => "progressbar",
            Role::List => "list",
            Role::MenuItem => "menuitem",
            Role::Alert => "alert",
            Role::RadioGroup => "radiogroup",
            Role::KeyboardKey => "keyboardkey",
            Role::ImageButton => "imagebutton",
            Role::Timer => "timer",
            Role::Adjustable => "adjustable",
            Role::Text => "text",
            Role::None => "none",
        }
    }

    pub fn from_name(name: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.name() == name)
    }

    /// The role for an element's role byte. `None` for `0xff` (no role) and
    /// any byte past the enum.
    pub fn from_byte(byte: u8) -> Option<Role> {
        Role::ALL.get(usize::from(byte)).copied()
    }

    /// A role a user acts on, which a screen reader can't announce usefully
    /// without a label.
    pub fn is_control(self) -> bool {
        matches!(
            self,
            Role::Button
                | Role::Link
                | Role::Tab
                | Role::Switch
                | Role::Checkbox
                | Role::Radio
                | Role::ToggleButton
                | Role::Search
                | Role::MenuItem
                | Role::KeyboardKey
                | Role::ImageButton
                | Role::Adjustable
        )
    }
}

/// `liveRegion` (281): how a platform announces a change to the node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LiveRegion {
    Polite,
    Assertive,
}

impl LiveRegion {
    pub fn name(self) -> &'static str {
        match self {
            LiveRegion::Polite => "polite",
            LiveRegion::Assertive => "assertive",
        }
    }

    pub fn from_name(name: &str) -> Option<LiveRegion> {
        match name {
            "polite" => Some(LiveRegion::Polite),
            "assertive" => Some(LiveRegion::Assertive),
            _ => None,
        }
    }
}

/// A `role` cell as the tree holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoleCell {
    Known(Role),
    /// A string outside the recovered enum. The host never writes one, so a
    /// tree holding one was written by something else.
    Unknown(String),
}

/// The state a node's tri-states and flags give it. `None` is unset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct States {
    pub selected: Option<bool>,
    pub checked: Option<bool>,
    pub expanded: Option<bool>,
    pub disabled: Option<bool>,
    pub busy: Option<bool>,
    pub modal: bool,
}

impl States {
    /// The states as words: `selected`, `unchecked`, `collapsed`, `modal`.
    pub fn words(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        let mut tri = |v: Option<bool>, on: &'static str, off: &'static str| match v {
            Some(true) => out.push(on),
            Some(false) if !off.is_empty() => out.push(off),
            _ => {}
        };
        tri(self.selected, "selected", "");
        tri(self.checked, "checked", "unchecked");
        tri(self.expanded, "expanded", "collapsed");
        tri(self.disabled, "disabled", "");
        tri(self.busy, "busy", "");
        if self.modal {
            out.push("modal");
        }
        out
    }
}

/// One node of the semantic tree: what a platform's accessibility node for
/// the element would hold, read from the element's cells alone.
#[derive(Clone, Debug, PartialEq)]
pub struct SemNode {
    /// The element it stands for.
    pub node: NodeId,
    pub role: Option<RoleCell>,
    /// `accessibilityLabel`, or the label of the node `accessibilityLabelledBy`
    /// names, or the children's labels joined for a node named by its
    /// children.
    pub label: Option<String>,
    pub hint: Option<String>,
    /// `accessibilityText`.
    pub text: Option<String>,
    pub role_description: Option<String>,
    pub live: Option<LiveRegion>,
    pub states: States,
    pub children: Vec<SemNode>,
}

impl SemNode {
    pub fn known_role(&self) -> Option<Role> {
        match self.role {
            Some(RoleCell::Known(r)) => Some(r),
            _ => None,
        }
    }

    /// This node and every node under it, depth first.
    pub fn walk(&self) -> Vec<&SemNode> {
        let mut out = vec![self];
        for c in &self.children {
            out.extend(c.walk());
        }
        out
    }

    /// The node as one line: role, label in quotes, then hint, text, live
    /// region and states.
    pub fn line(&self) -> String {
        let mut s = match &self.role {
            Some(RoleCell::Known(r)) => r.name().to_string(),
            Some(RoleCell::Unknown(r)) => format!("?{r}"),
            None => "-".to_string(),
        };
        if let Some(l) = &self.label {
            let _ = write!(s, " {l:?}");
        }
        if let Some(d) = &self.role_description {
            let _ = write!(s, " roleDescription={d:?}");
        }
        if let Some(h) = &self.hint {
            let _ = write!(s, " hint={h:?}");
        }
        if let Some(t) = &self.text {
            let _ = write!(s, " text={t:?}");
        }
        if let Some(l) = self.live {
            let _ = write!(s, " live={}", l.name());
        }
        let words = self.states.words();
        if !words.is_empty() {
            let _ = write!(s, " [{}]", words.join(","));
        }
        s
    }
}

/// Modelled. The semantic tree under each root of `tree` (a node no other
/// node lists as a child), from the accessibility cells alone. The platform's
/// own tree builder is obfuscated Kotlin, so the rules (see the module docs)
/// are the usual ones and would be replaced by a recovered builder's.
pub fn semantics(tree: &Tree) -> Vec<SemNode> {
    let listed: BTreeSet<NodeId> = tree
        .nodes()
        .flat_map(|n| n.children.iter().copied())
        .collect();
    let roots: Vec<NodeId> = tree
        .nodes()
        .map(|n| n.id)
        .filter(|id| !listed.contains(id))
        .collect();
    let reader = Reader::new(tree);
    roots.into_iter().flat_map(|r| reader.build(r)).collect()
}

/// The semantic tree under one node of `tree`.
pub fn semantics_from(tree: &Tree, root: NodeId) -> Vec<SemNode> {
    Reader::new(tree).build(root)
}

/// The semantic tree as text, one node per line, indented two spaces a level.
pub fn render(nodes: &[SemNode]) -> String {
    fn go(n: &SemNode, depth: usize, out: &mut String) {
        out.push_str(&"  ".repeat(depth));
        out.push_str(&n.line());
        out.push('\n');
        for c in &n.children {
            go(c, depth + 1, out);
        }
    }
    let mut out = String::new();
    for n in nodes {
        go(n, 0, &mut out);
    }
    out
}

struct Reader<'t> {
    tree: &'t Tree,
    /// `accessibilityId` to the node that carries it.
    by_id: BTreeMap<&'t str, NodeId>,
}

impl<'t> Reader<'t> {
    fn new(tree: &'t Tree) -> Self {
        let mut by_id = BTreeMap::new();
        for n in tree.nodes() {
            if let Some(Value::Str(id)) = tree.props().get(n.id, ids::ID) {
                by_id.entry(id.as_str()).or_insert(n.id);
            }
        }
        Reader { tree, by_id }
    }

    fn cell(&self, node: NodeId, prop: PropId) -> Option<&'t Value> {
        self.tree
            .props()
            .get(node, prop)
            .filter(|v| **v != Value::Null)
    }

    fn text(&self, node: NodeId, prop: PropId) -> Option<&'t str> {
        match self.cell(node, prop)? {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    fn tri(&self, node: NodeId, prop: PropId) -> Option<bool> {
        match self.cell(node, prop)? {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    fn flag(&self, node: NodeId, prop: PropId) -> bool {
        self.tri(node, prop) == Some(true)
    }

    fn children(&self, node: NodeId) -> &'t [NodeId] {
        self.tree.node(node).map_or(&[], |n| &n.children[..])
    }

    /// The semantic nodes `node` contributes: itself with its children, or
    /// its children alone when it has nothing to say, or nothing when it is
    /// hidden.
    fn build(&self, node: NodeId) -> Vec<SemNode> {
        if self.tree.node(node).is_none() || self.flag(node, ids::HIDDEN) {
            return Vec::new();
        }
        let children: Vec<SemNode> = self
            .children(node)
            .iter()
            .flat_map(|c| self.build(*c))
            .collect();
        let role = self.text(node, ids::ROLE).map(|r| {
            Role::from_name(r).map_or_else(|| RoleCell::Unknown(r.to_string()), RoleCell::Known)
        });
        let mut label = self.text(node, ids::LABEL).map(str::to_string);
        if label.is_none() {
            label = self
                .text(node, ids::LABELLED_BY)
                .and_then(|id| self.by_id.get(id))
                .and_then(|n| {
                    self.text(*n, ids::LABEL)
                        .or_else(|| self.text(*n, ids::TEXT))
                })
                .map(str::to_string);
        }
        if label.is_none() && self.flag(node, ids::NAMED_BY_CHILDREN) {
            let joined: Vec<&str> = children
                .iter()
                .flat_map(SemNode::walk)
                .filter_map(|c| c.label.as_deref())
                .collect();
            if !joined.is_empty() {
                label = Some(joined.join(", "));
            }
        }
        let states = States {
            selected: self.tri(node, ids::SELECTED),
            checked: self.tri(node, ids::CHECKED),
            expanded: self.tri(node, ids::EXPANDED),
            disabled: self.tri(node, ids::DISABLED),
            busy: self.tri(node, ids::BUSY),
            modal: self.flag(node, ids::MODAL),
        };
        let sem = SemNode {
            node,
            role,
            label,
            hint: self.text(node, ids::HINT).map(str::to_string),
            text: self.text(node, ids::TEXT).map(str::to_string),
            role_description: self.text(node, ids::ROLE_DESCRIPTION).map(str::to_string),
            live: self
                .text(node, ids::LIVE_REGION)
                .and_then(LiveRegion::from_name),
            states,
            children,
        };
        let says_something = matches!(&sem.role, Some(r) if *r != RoleCell::Known(Role::None))
            || sem.label.is_some()
            || sem.text.is_some()
            || sem.live.is_some()
            || sem.states != States::default()
            || self.flag(node, ids::ACCESSIBLE);
        if says_something {
            vec![sem]
        } else {
            sem.children
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ElementKind, NodeOp, Turn};

    fn s(v: &str) -> Value {
        Value::Str(v.into())
    }

    fn tree(ops: Vec<NodeOp>) -> Tree {
        let mut t = Tree::new();
        t.apply_bytes(&Turn { ops }.to_bytes()).unwrap();
        t
    }

    #[test]
    fn the_ids_are_the_recovered_names() {
        for (id, name) in [
            (ids::ROLE, "role"),
            (ids::LABEL, "accessibilityLabel"),
            (ids::HINT, "accessibilityHint"),
            (ids::ID, "accessibilityId"),
            (ids::LABELLED_BY, "accessibilityLabelledBy"),
            (ids::TEXT, "accessibilityText"),
            (ids::ROLE_DESCRIPTION, "accessibilityRoleDescription"),
            (ids::LIVE_REGION, "liveRegion"),
            (ids::HIDDEN, "accessibilityHidden"),
            (ids::ACCESSIBLE, "accessible"),
            (ids::KEEPS_CHILDREN, "accessibilityKeepsChildren"),
            (ids::NAMED_BY_CHILDREN, "accessibilityNamedByChildren"),
            (ids::MODAL, "accessibilityModal"),
            (ids::SELECTED, "accessibilitySelected"),
            (ids::CHECKED, "accessibilityChecked"),
            (ids::EXPANDED, "accessibilityExpanded"),
            (ids::DISABLED, "accessibilityDisabled"),
            (ids::BUSY, "accessibilityBusy"),
        ] {
            assert_eq!(id.name(), Some(name));
        }
    }

    #[test]
    fn the_role_bytes_are_the_recovered_table() {
        assert_eq!(Role::from_byte(0), Some(Role::Button));
        assert_eq!(Role::from_byte(12), Some(Role::List));
        assert_eq!(Role::from_byte(21), Some(Role::None));
        assert_eq!(Role::from_byte(0xff), None);
        for r in Role::ALL {
            assert_eq!(Role::from_name(r.name()), Some(r));
        }
    }

    #[test]
    fn a_tree_of_cells_becomes_the_semantic_tree() {
        let t = tree(vec![
            NodeOp::Create {
                node: 1,
                kind: ElementKind::Div,
            },
            NodeOp::Create {
                node: 2,
                kind: ElementKind::List,
            },
            NodeOp::Create {
                node: 3,
                kind: ElementKind::Div,
            },
            NodeOp::Create {
                node: 4,
                kind: ElementKind::Div,
            },
            NodeOp::Create {
                node: 5,
                kind: ElementKind::Text,
            },
            NodeOp::Create {
                node: 6,
                kind: ElementKind::Div,
            },
            NodeOp::Create {
                node: 7,
                kind: ElementKind::Text,
            },
            NodeOp::SetProps {
                node: 2,
                props: vec![(ids::ROLE, s("list")), (ids::LABEL, s("Messages"))],
            },
            // A wrapper with nothing to say: its button moves up to the list.
            NodeOp::SetProps {
                node: 4,
                props: vec![
                    (ids::ROLE, s("button")),
                    (ids::LABEL, s("Reply")),
                    (ids::SELECTED, Value::Bool(false)),
                ],
            },
            NodeOp::SetProps {
                node: 5,
                props: vec![(ids::HIDDEN, Value::Bool(true)), (ids::LABEL, s("decor"))],
            },
            NodeOp::SetProps {
                node: 6,
                props: vec![(ids::LIVE_REGION, s("polite")), (ids::LABELLED_BY, s("t"))],
            },
            NodeOp::SetProps {
                node: 7,
                props: vec![(ids::ID, s("t")), (ids::TEXT, s("Alice is typing"))],
            },
            NodeOp::SetChildren {
                node: 1,
                children: vec![2, 6],
            },
            NodeOp::SetChildren {
                node: 2,
                children: vec![3],
            },
            NodeOp::SetChildren {
                node: 3,
                children: vec![4, 5],
            },
            NodeOp::SetChildren {
                node: 6,
                children: vec![7],
            },
        ]);
        let sem = semantics(&t);
        assert_eq!(
            render(&sem),
            "list \"Messages\"\n  button \"Reply\"\n- \"Alice is typing\" live=polite\n  - text=\"Alice is typing\"\n"
        );
        assert_eq!(sem[0].children[0].states.selected, Some(false));
    }

    #[test]
    fn a_role_outside_the_enum_is_kept_and_marked() {
        let t = tree(vec![
            NodeOp::Create {
                node: 1,
                kind: ElementKind::Div,
            },
            NodeOp::SetProps {
                node: 1,
                props: vec![(ids::ROLE, s("listitem"))],
            },
        ]);
        assert_eq!(
            semantics(&t)[0].role,
            Some(RoleCell::Unknown("listitem".into()))
        );
        assert_eq!(render(&semantics(&t)), "?listitem\n");
    }

    #[test]
    fn a_node_named_by_its_children_joins_their_labels() {
        let t = tree(vec![
            NodeOp::Create {
                node: 1,
                kind: ElementKind::Div,
            },
            NodeOp::Create {
                node: 2,
                kind: ElementKind::Text,
            },
            NodeOp::Create {
                node: 3,
                kind: ElementKind::Text,
            },
            NodeOp::SetProps {
                node: 1,
                props: vec![
                    (ids::ROLE, s("button")),
                    (ids::NAMED_BY_CHILDREN, Value::Bool(true)),
                ],
            },
            NodeOp::SetProps {
                node: 2,
                props: vec![(ids::LABEL, s("Alice"))],
            },
            NodeOp::SetProps {
                node: 3,
                props: vec![(ids::LABEL, s("Online"))],
            },
            NodeOp::SetChildren {
                node: 1,
                children: vec![2, 3],
            },
        ]);
        assert_eq!(semantics(&t)[0].label.as_deref(), Some("Alice, Online"));
    }
}
