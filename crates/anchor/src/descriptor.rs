//! What an anchor set says: the anchors, their locators, and how they
//! appear in `anchors.json`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// One durable way to locate an integration point. An anchor may carry
/// several. They are redundant descriptions of the *same* point, so losing
/// one before the others is an early warning that the anchor is eroding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Locator {
    /// An exact string expected somewhere in the binary (a panic path, a
    /// token name, an assertion message).
    String { text: String },
    /// A symbol defined in the binary's symbol table, which release builds
    /// usually strip. Exports alone don't count.
    Symbol { name: String },
    /// A symbol an ELF image exports: a definition in its dynamic symbol
    /// table. Stripping keeps these, and other libraries link against them
    /// by name, so an export outlives any rebuild that doesn't remove it.
    Export { name: String },
    /// A masked byte signature over code, IDA-style: `48 8b ?? ?? e8 ?? ?? ?? ??`.
    Bytes { pattern: String },
    /// The function(s) whose code loads `text`'s address (a RIP-relative
    /// `lea`/`mov`, checked by decoding). Strings tend to outlive the code
    /// around them, so this survives rewrites that break byte patterns.
    /// Resolves to the referencing function's entry. On AArch64 the load is
    /// an `adrp` and the `add` or `ldr` that completes it.
    StringRef { text: String },
    /// The function a `call rel32` calls. The pattern (scoped, like any
    /// pattern, by the anchor's `within`) covers the call, starting at it or
    /// a few instructions before it when the bytes after it aren't
    /// distinctive, and the locator resolves to the target of the first call
    /// in the match. A callee with no distinguishing bytes of its own (a
    /// generic the client instantiates for one of its types) is identified
    /// by the client's call to it. PE (x64) only.
    CallTarget { pattern: String },
    /// The `nth` distinct function that anchor `from`'s function calls
    /// directly (`call rel32`, in code order): follows the call graph from a
    /// point that is easier to anchor. Resolves after `from`, wherever it
    /// sits in the set. PE (x64) only.
    Callee {
        from: String,
        #[serde(default)]
        nth: usize,
    },
    /// The function(s) that directly call anchor `of`'s function. Resolves
    /// after `of`, wherever it sits in the set. PE (x64) only.
    Caller { of: String },
    /// A run of instructions by shape (registers as variables, no
    /// displacements or large immediates), resolving to its `at`th
    /// instruction. Survives register allocation, stack layout and field
    /// offsets changing, which byte patterns don't: the locator for an
    /// operand anchor whose build changed compiler, or whose fork moved the
    /// entry. Scoped by the anchor's `within`, else every `.pdata` function.
    /// PE (x64) only.
    Shape {
        insns: String,
        #[serde(default)]
        at: usize,
    },
    /// The function(s) that load a panic location: a `core::panic::Location`
    /// record of `file`, `line` and `col`, which rustc emits for each panic
    /// site, `#[track_caller]` call and index expression. A site's location is
    /// unique, survives release builds, LTO and stripping, and moves with its
    /// code when that code is inlined. Two builds of the same source share
    /// them, a web build and a native one included. `file` matches on its
    /// last three path components (see [`file_key`]), so builds made in
    /// different directories match. PE (x64) only.
    PanicLocation { file: String, line: u32, col: u32 },
    /// The slot of a trait object's vtable that holds anchor `function`'s
    /// entry as its `method`th method. rustc lays a vtable out as
    /// `drop_in_place`, size, align, then the methods, so the slot is
    /// `3 + method` words in. `size` and `align`, when given, must match the
    /// header: the size and alignment of the type behind the trait object.
    /// Resolves to the slot (data, not code), after `function`. Swapping the
    /// pointer there hooks every call made through the vtable.
    VtableSlot {
        function: String,
        #[serde(default)]
        method: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        align: Option<u64>,
    },
}

impl Locator {
    /// The anchor a call-graph locator walks from, if it is one.
    pub fn depends_on(&self) -> Option<&str> {
        match self {
            Locator::Callee { from, .. } => Some(from),
            Locator::Caller { of } => Some(of),
            Locator::VtableSlot { function, .. } => Some(function),
            _ => None,
        }
    }

    /// A short label for reports, with long patterns and strings cut to 40
    /// characters.
    pub fn label(&self) -> String {
        match self {
            Locator::String { text } => format!("string {:?}", truncate(text, 40)),
            Locator::Symbol { name } => format!("symbol `{name}`"),
            Locator::Export { name } => format!("export `{name}`"),
            Locator::Bytes { pattern } => format!("bytes `{}`", truncate(pattern, 40)),
            Locator::StringRef { text } => format!("string-ref {:?}", truncate(text, 40)),
            Locator::CallTarget { pattern } => format!("call-target `{}`", truncate(pattern, 40)),
            Locator::Callee { from, nth } => format!("callee #{nth} of `{from}`"),
            Locator::Caller { of } => format!("caller of `{of}`"),
            Locator::Shape { insns, at } => format!("shape `{}` @{at}", truncate(insns, 40)),
            Locator::PanicLocation { file, line, col } => {
                format!("panic location {}:{line}:{col}", file_key(file))
            }
            Locator::VtableSlot {
                function, method, ..
            } => format!("vtable slot of `{function}` as method #{method}"),
        }
    }
}

/// A source path by its last three components, lowercase, `/`-separated:
/// the same file in builds made in different directories, on Windows or
/// elsewhere.
pub fn file_key(path: &str) -> String {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    parts[parts.len().saturating_sub(3)..]
        .join("/")
        .to_lowercase()
}

fn truncate(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((end, _)) => format!("{}…", &s[..end]),
        None => s.to_string(),
    }
}

/// Which operand of a located instruction an operand anchor reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Operand {
    /// The memory operand's displacement (`[reg+disp]`): a field offset.
    Disp,
    /// The first immediate (`mov r8d, imm`, `add rcx, imm`): a size or offset.
    Imm,
}

/// A named integration point with one or more locators, in order of
/// preference.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The point is a function's entry: every locator's hit is normalized to
    /// the start of the function containing it (PE `.pdata`), so locators that
    /// land mid-function still agree, and disagreeing ones are a conflict.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub function: bool,
    /// The point is any one of a set of interchangeable functions (e.g. a
    /// generic instantiated for two types with one layout and delegating
    /// behaviour), so a locator matching several of them is a hit, not an
    /// ambiguity: it resolves to the first. The `note` should say why they're
    /// interchangeable.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub any_of: bool,
    /// Search only inside the function another anchor (by name) resolves
    /// into: its `.pdata` extent. Short patterns then suffice, and a match
    /// elsewhere in the binary can't count. Resolved after the anchor named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within: Option<String>,
    /// Makes this an **operand anchor**, whose point is a value rather than
    /// a place: this operand of the instruction each locator finds (a field
    /// offset in a `mov [reg+disp]`, a size in a `mov r8d, imm`). Locators
    /// vote on the value, so independent instructions using the same offset
    /// corroborate it. This is how layout is derived per build instead of
    /// stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operand: Option<Operand>,
    /// For an operand anchor: the value is this operand **minus** the value
    /// of another operand anchor (by name). A field offset an optimizer only
    /// ever addresses through a stack local (`[rsp+local+field]`) is the
    /// difference of two displacements: the field's and the local's base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_to: Option<String>,
    /// For an operand anchor: add this to the value (after `relative_to`).
    /// A size a build only ever moves as qword copies is the last copy's
    /// displacement plus 8. An anchor relative to this one uses its operand
    /// as read, without the addition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plus: Option<i64>,
    /// For a function anchor on open-source code (gpui): the function it
    /// locates, as its symbol with crate hashes stripped (`Cs<hash>_` ->
    /// `Cs_`), which stays the same across compilers. Re-recovery compiles
    /// that source with the target's compiler and finds the function by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub locators: Vec<Locator>,
}

impl Anchor {
    /// The anchors this one resolves after: its scope, and those its
    /// call-graph locators walk from.
    pub fn depends_on(&self) -> impl Iterator<Item = &str> {
        self.within
            .as_deref()
            .into_iter()
            .chain(self.locators.iter().filter_map(Locator::depends_on))
    }
}

/// A set of anchors describing a target's integration points: the contents
/// of an `anchors.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorSet {
    /// Freeform hint, e.g. `"zed v1.27.0 / zed.exe"`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target: String,
    pub anchors: Vec<Anchor>,
}

impl AnchorSet {
    /// Parse an `anchors.json`.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).context("anchor set is not valid JSON")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_target_locators_are_kebab_kind_in_json() {
        let loc = Locator::CallTarget {
            pattern: "e8 ?? ?? ?? ?? 90".into(),
        };
        let j = serde_json::to_value(&loc).unwrap();
        assert_eq!(
            j,
            serde_json::json!({ "kind": "call-target", "pattern": "e8 ?? ?? ?? ?? 90" })
        );
        assert_eq!(serde_json::from_value::<Locator>(j).unwrap(), loc);
    }

    #[test]
    fn exports_and_vtable_slots_are_kebab_kind_in_json() {
        let slot = Locator::VtableSlot {
            function: "emit".into(),
            method: 0,
            size: Some(64),
            align: Some(8),
        };
        let j = serde_json::to_value(&slot).unwrap();
        assert_eq!(
            j,
            serde_json::json!({
                "kind": "vtable-slot", "function": "emit", "method": 0, "size": 64, "align": 8
            })
        );
        let bare: Locator =
            serde_json::from_value(serde_json::json!({ "kind": "vtable-slot", "function": "f" }))
                .unwrap();
        assert_eq!(
            bare,
            Locator::VtableSlot {
                function: "f".into(),
                method: 0,
                size: None,
                align: None
            }
        );
        assert_eq!(bare.depends_on(), Some("f"));
        let export: Locator =
            serde_json::from_value(serde_json::json!({ "kind": "export", "name": "ld_event" }))
                .unwrap();
        assert_eq!(export.label(), "export `ld_event`");
    }

    #[test]
    fn panic_locations_match_across_build_directories() {
        let wasm = r"C:\Users\x\.cargo\git\checkouts\zed-a70e\c84af5b\crates\gpui\src\window.rs";
        let native = "/home/ci/.cargo/git/checkouts/zed-a70e/c84af5b/crates/gpui/src/window.rs";
        assert_eq!(file_key(wasm), file_key(native));
        assert_eq!(file_key(wasm), "gpui/src/window.rs");
        assert_ne!(file_key(wasm), file_key(r"C:\x\crates\gpui\src\app.rs"));
        let loc = Locator::PanicLocation {
            file: wasm.into(),
            line: 3786,
            col: 31,
        };
        let json = serde_json::to_string(&loc).unwrap();
        assert!(json.contains(r#""kind":"panic-location""#));
        assert_eq!(serde_json::from_str::<Locator>(&json).unwrap(), loc);
    }

    #[test]
    fn labels_cut_long_text_at_a_character_boundary() {
        let text = "é".repeat(50);
        let label = Locator::String { text }.label();
        assert_eq!(label, format!("string \"{}…\"", "é".repeat(40)));
        let short = Locator::Bytes {
            pattern: "48 8b".into(),
        };
        assert_eq!(short.label(), "bytes `48 8b`");
    }

    #[test]
    fn anchors_serialize_without_their_defaults() {
        let anchor = Anchor {
            name: "f".into(),
            locators: vec![Locator::Symbol { name: "f".into() }],
            ..Anchor::default()
        };
        assert_eq!(
            serde_json::to_value(&anchor).unwrap(),
            serde_json::json!({
                "name": "f",
                "locators": [{ "kind": "symbol", "name": "f" }],
            })
        );
    }
}
