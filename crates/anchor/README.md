# rekindle-anchor

rekindle-anchor resolves durable **anchors** for integration points per build,
so a new build auto-reports which anchors **broke**. A release binary moves code and data between builds, so
rekindle never hardcodes addresses. Instead, locators that survive rebuilds
describe each integration point, and we re-resolve them against every build.

It is pure static analysis: it reads a file and reports locations. It never modifies a
binary and does no patching. Locating an anchor is the *input* to re-resolution,
not a change to the target.

## Anchors

An anchor is a named point with one or more **locators** (redundant descriptions
of the same point). Losing one locator before the others is an early warning that
the anchor is eroding.

| Locator | Resolves by | Survives |
|---|---|---|
| `string` | exact byte occurrence in the file | renames of code, as long as the message stays |
| `symbol` | a symbol table entry's address → file offset | only while the binary keeps a symbol table (dev builds) |
| `bytes` | a relocation-masked byte signature over code ([`rekindle-sigmatch`](../sigmatch) format) | code moving / references being re-linked |
| `string-ref` | the function whose code loads a string's address (RIP-relative, PE x64) | rewrites that break byte patterns, as long as the function still uses the string |
| `call-target` | the target of the first `call rel32` in a byte pattern over the call site (PE x64) | a callee with no distinctive bytes of its own, such as a generic instantiated for one client type |
| `panic-location` | the functions that load a `core::panic::Location` record of `file`, `line` and `col` (PE x64) | release builds, LTO, stripping and inlining; a web build and a native one of the same source share them |
| `callee` | the `nth` distinct function another anchor's function calls directly (PE x64) | the callee changing shape, as long as the caller still calls it |
| `caller` | the functions that directly call another anchor's function (PE x64) | the caller changing shape, as long as it still makes the call |
| `shape` | a run of instructions by mnemonic and operand kind, with registers as variables and no displacements; resolves to its `at`th instruction (PE x64) | register allocation, stack layout and field offsets changing, as long as the instructions stay in order |

A `shape` is written like `cmp d[r0+*],0; je *; movd x0,d[r0+*]`. Each
write to a register starts a new variable, so one build reusing a dead
register and another taking a free one still match. The syntax is in
[`src/shape.rs`](src/shape.rs). `suggest-operand` gives each instruction
two shapes, one with the context after it and one with the context before
it, and `suggest-shape` builds one that is unique in the whole binary, as a
structural locator for a function anchor.

Call-graph locators name another anchor (`from` / `of`) and resolve after it,
wherever it sits in the set (as with `within`). If it never resolves, they
report `broken`.

See [`examples/anchors.example.json`](examples/anchors.example.json).

## Use

```sh
# Resolve against one build (exit non-zero if any anchor is broken):
rekindle-anchor resolve anchors.json build/app.exe --out baseline.json

# Report what changed between two builds (exit non-zero on a break — a CI gate):
rekindle-anchor diff anchors.json old/app.exe new/app.exe

# Or diff two saved resolutions (store a baseline, compare each new build to it):
rekindle-anchor diff-report baseline.json new-resolution.json
```

Per anchor the resolver reports `resolved` (≥1 locator unique), `ambiguous`
(matches more than once, so specificity is lost) or `broken` (nothing found), plus
how many locators still resolve uniquely. The diff classifies each as `ok`,
`broke`, `recovered`, `weakened` (still resolved but fewer locators do),
`still-broken`, `new` or `dropped`.

## CI

1. Keep an anchor set per target in the repo.
2. Keep a baseline resolution for the last known-good build.
3. On each new build, run `diff-report baseline.json <new>` (or `diff` against
   the old binary). It fails if any anchor broke.

This catches a build that moved an integration point before anything tries to
use it.
