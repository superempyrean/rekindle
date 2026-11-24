//! Fixtures out of captures: one screen of a capture as a single turn, with
//! the user's content replaced.
//!
//! [`Sanitizer::snapshot`] writes the tree a capture leaves as one turn
//! that creates it: a Create for every node reachable from the root, then
//! each node's children, then the root. Every string cell, and every string
//! inside one, goes through [`Sanitizer::string`]:
//!
//! - A drawable from the client's own resources (`res://...`) stays. Any
//!   other address (`https://`, `data:`) becomes empty, so no avatar,
//!   attachment or link survives.
//! - A string on the keep list stays. The list is the client's own labels
//!   ("Settings", "Messages"), checked by hand against the strings a
//!   capture holds.
//! - A short token with no spaces in a cell a renderer reads as a keyword
//!   (`"row"`, `"top|start"`, `"50%"`) or in an `accessibilityId` stays,
//!   with any run of five or more digits replaced. Those runs are ids.
//! - Anything else is text the user or other users wrote, or their names.
//!   It is replaced word by word with words from a fixed list, keeping
//!   each word's length class, capitals, digits and punctuation, so a line
//!   wraps about where the original did. The same string always gets the
//!   same replacement.
//!
//! [`strings`] lists what a capture holds, for checking a fixture before it
//! is committed.

use std::collections::{BTreeMap, BTreeSet};

use rekindle_wire::{NodeId, NodeOp, PropId, Tree, Turn, Value};

/// Cells and record fields whose strings are keywords a renderer reads
/// (`"row"`, `"top|start"`, `"50%"`). A short token there stays; a string
/// in any other cell is text.
const TOKEN_FIELDS: &[&str] = &[
    "accessibilityId",
    "align",
    "backgroundGradientAxis",
    "direction",
    "edge",
    "flexBasis",
    "fontFamily",
    "fullResolution",
    "gravity",
    "height",
    "justify",
    "keyboardShouldPersistTaps",
    "liveRegion",
    "longPressHaptic",
    "on",
    "panWorkletAxis",
    "placement",
    "pointerEvents",
    "pool",
    "presentation",
    "pressFeedback",
    "role",
    "scale",
    "textAlign",
    "textBreakStrategy",
    "textTransform",
    "transition",
    "width",
];

const WORDS: &[&str] = &[
    "a",
    "an",
    "at",
    "be",
    "by",
    "do",
    "go",
    "in",
    "is",
    "it",
    "me",
    "no",
    "of",
    "on",
    "or",
    "so",
    "to",
    "up",
    "we",
    "all",
    "and",
    "are",
    "but",
    "can",
    "day",
    "for",
    "get",
    "had",
    "has",
    "hey",
    "how",
    "new",
    "not",
    "now",
    "one",
    "our",
    "out",
    "see",
    "sun",
    "the",
    "two",
    "way",
    "who",
    "yes",
    "you",
    "also",
    "back",
    "been",
    "bird",
    "blue",
    "call",
    "come",
    "dawn",
    "done",
    "each",
    "even",
    "fern",
    "from",
    "good",
    "have",
    "here",
    "just",
    "kind",
    "like",
    "lime",
    "long",
    "look",
    "make",
    "mint",
    "more",
    "much",
    "nice",
    "only",
    "over",
    "pine",
    "rain",
    "said",
    "some",
    "soon",
    "take",
    "than",
    "that",
    "them",
    "then",
    "they",
    "this",
    "time",
    "very",
    "want",
    "well",
    "were",
    "what",
    "when",
    "will",
    "with",
    "work",
    "about",
    "after",
    "amber",
    "birch",
    "brook",
    "cedar",
    "clear",
    "could",
    "earth",
    "first",
    "found",
    "great",
    "green",
    "hello",
    "house",
    "later",
    "light",
    "maple",
    "might",
    "night",
    "ocean",
    "other",
    "place",
    "plant",
    "quiet",
    "river",
    "round",
    "small",
    "sound",
    "still",
    "stone",
    "think",
    "three",
    "today",
    "water",
    "where",
    "which",
    "while",
    "world",
    "would",
    "always",
    "around",
    "before",
    "bright",
    "button",
    "candle",
    "garden",
    "little",
    "meadow",
    "number",
    "orange",
    "people",
    "pocket",
    "purple",
    "really",
    "should",
    "silver",
    "spring",
    "summer",
    "window",
    "winter",
    "another",
    "because",
    "between",
    "evening",
    "harbour",
    "lantern",
    "morning",
    "outside",
    "picture",
    "running",
    "something",
    "tomorrow",
    "together",
    "question",
    "mountain",
    "starlight",
    "wonderful",
];

/// Turns a capture's screens into fixtures. See the module docs.
pub struct Sanitizer {
    keep: BTreeSet<String>,
    seen: BTreeMap<String, String>,
}

impl Sanitizer {
    /// A sanitizer that keeps the strings in `keep` as they are.
    pub fn new(keep: impl IntoIterator<Item = String>) -> Sanitizer {
        Sanitizer {
            keep: keep.into_iter().collect(),
            seen: BTreeMap::new(),
        }
    }

    /// One turn that creates the part of `tree` its root reaches, every
    /// string sanitized.
    pub fn snapshot(&mut self, tree: &Tree) -> Turn {
        let mut turn = Turn::new();
        let Some(root) = tree.root() else {
            return turn;
        };
        let order = reachable(tree, root);
        for &id in &order {
            let node = tree.node(id).expect("reachable nodes exist");
            let props = tree
                .props()
                .node_props(id)
                .map(|(p, v)| (p, self.value(field(p), v)))
                .collect();
            turn.push(NodeOp::Create {
                node: id,
                kind: node.kind.clone(),
                props,
            });
        }
        for &id in &order {
            let node = tree.node(id).expect("reachable nodes exist");
            if !node.children.is_empty() {
                turn.push(NodeOp::Children {
                    node: id,
                    children: node.children.clone(),
                });
            }
        }
        turn.push(NodeOp::Root { node: root });
        turn
    }

    fn value(&mut self, field: &str, v: &Value) -> Value {
        match v {
            Value::Str(s) => Value::Str(self.string(field, s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.value(field, x)).collect()),
            Value::Object(f) => Value::Object(
                f.iter()
                    .map(|(k, x)| (*k, self.value(self::field(*k), x)))
                    .collect(),
            ),
            Value::Map(m) => Value::Map(
                m.iter()
                    .map(|(k, x)| (self.string("", k), self.value(k, x)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// `s`, found in cell or field `field`, as the fixture keeps it.
    pub fn string(&mut self, field: &str, s: &str) -> String {
        if s.is_empty() || s.starts_with("res://") || self.keep.contains(s) {
            return s.to_string();
        }
        if s.contains("://") || s.starts_with("data:") || s.starts_with("www.") {
            return String::new();
        }
        if TOKEN_FIELDS.contains(&field) && is_token(s) {
            return fake_digits(s);
        }
        if let Some(r) = self.seen.get(s) {
            return r.clone();
        }
        let r = mock_text(s);
        self.seen.insert(s.to_string(), r.clone());
        r
    }
}

fn field(p: PropId) -> &'static str {
    p.name().unwrap_or("")
}

/// The nodes `root` reaches, in pre-order, each once.
fn reachable(tree: &Tree, root: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let Some(node) = tree.node(id) else { continue };
        if !seen.insert(id) {
            continue;
        }
        out.push(id);
        stack.extend(node.children.iter().rev());
    }
    out
}

/// A string a renderer reads as a keyword or a measure, not as text.
fn is_token(s: &str) -> bool {
    s.len() <= 40
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_|%.-:#/".contains(c))
}

fn hash(s: &str, salt: u64) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ salt;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// `s` with each run of five or more digits replaced by as many digits
/// made from its hash.
fn fake_digits(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        if run.len() >= 5 {
            let mut h = hash(run, 7);
            for _ in 0..run.len() {
                out.push(char::from(b'0' + (h % 10) as u8));
                h = h / 10 + hash(out, 3) % 7;
            }
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in s.chars() {
        if c.is_ascii_digit() {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// Text with each word replaced: see the module docs.
fn mock_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut word = String::new();
    let mut n = 0u64;
    let mut flush = |word: &mut String, out: &mut String| {
        if !word.is_empty() {
            out.push_str(&mock_word(word, hash(s, n)));
            n += 1;
            word.clear();
        }
    };
    for c in s.chars() {
        if c.is_alphabetic() {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    fake_digits(&out)
}

fn mock_word(word: &str, h: u64) -> String {
    let len = word.chars().count();
    let near: Vec<&str> = WORDS
        .iter()
        .copied()
        .filter(|w| w.len().abs_diff(len) <= usize::from(len > 6))
        .collect();
    let pool = if near.is_empty() { WORDS } else { &near[..] };
    let base = pool[(h % pool.len() as u64) as usize];
    let upper = word.chars().all(char::is_uppercase) && len > 1;
    let cap = word.chars().next().is_some_and(char::is_uppercase);
    if upper {
        base.to_uppercase()
    } else if cap {
        let mut c = base.chars();
        c.next()
            .map(|f| f.to_uppercase().chain(c).collect())
            .unwrap_or_default()
    } else {
        base.to_string()
    }
}

/// Every string in the tree its root reaches, with the cells and fields it
/// was found in.
pub fn strings(tree: &Tree) -> BTreeMap<String, BTreeSet<String>> {
    fn walk(field: &str, v: &Value, out: &mut BTreeMap<String, BTreeSet<String>>) {
        match v {
            Value::Str(s) if !s.is_empty() => {
                out.entry(s.clone()).or_default().insert(field.to_string());
            }
            Value::Array(a) => a.iter().for_each(|x| walk(field, x, out)),
            Value::Object(f) => f.iter().for_each(|(k, x)| walk(self::field(*k), x, out)),
            Value::Map(m) => m.iter().for_each(|(k, x)| {
                out.entry(k.clone()).or_default().insert("(map key)".into());
                walk(k, x, out);
            }),
            _ => {}
        }
    }
    let mut out = BTreeMap::new();
    if let Some(root) = tree.root() {
        for id in reachable(tree, root) {
            for (p, v) in tree.props().node_props(id) {
                walk(field(p), v, &mut out);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_go_and_resources_stay() {
        let mut s = Sanitizer::new([]);
        assert_eq!(s.string("uri", "res://ic_mana_gear"), "res://ic_mana_gear");
        assert_eq!(s.string("uri", "https://cdn.example/avatars/1.png"), "");
        assert_eq!(s.string("uri", "data:image/png;base64,AAAA"), "");
    }

    #[test]
    fn tokens_stay_with_their_ids_replaced() {
        let mut s = Sanitizer::new([]);
        assert_eq!(s.string("direction", "row"), "row");
        assert_eq!(s.string("gravity", "top|start"), "top|start");
        let key = s.string("accessibilityId", "channel-123456789012");
        assert!(key.starts_with("channel-"));
        assert_ne!(key, "channel-123456789012");
        assert_eq!(key.len(), "channel-123456789012".len());
        // Anywhere else a one-word name is text.
        assert_ne!(s.string("text", "alice"), "alice");
        assert_ne!(s.string("accessibilityText", "alice"), "alice");
    }

    #[test]
    fn text_is_replaced_the_same_way_each_time_and_keeps_its_shape() {
        let mut s = Sanitizer::new(["Settings".to_string()]);
        assert_eq!(s.string("text", "Settings"), "Settings");
        let a = s.string("text", "Hey Sam, see you at 10:30?");
        assert_eq!(a, s.string("text", "Hey Sam, see you at 10:30?"));
        assert!(!a.contains("Sam"));
        assert!(a.ends_with("10:30?"));
        assert!(a.chars().next().unwrap().is_uppercase());
    }
}
