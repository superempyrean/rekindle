# rekindle-wire

The wire a split GPUI client speaks between its sandboxed wasm guest and its
native host. The guest describes its views as one op buffer per turn, and the
host decodes it, reconciles it against a retained element tree and paints.
rekindle's host links this crate to read and rewrite those turns.

The recovered parts are read out of Phoenix's Android host library
(`liblibdiscord_wasm_host.so`, build 356207) and copied byte for byte. The
modelled parts are rekindle's stand-in, kept inside what the recovered parts
allow. Reading the guest code would replace the modelled rows. No guest module
ships in the APK, but build 356207 compiles the guest into the host, so it is
recoverable from that binary rather than only over the air.

| Piece | Status | Where |
|---|---|---|
| The 13-tag value codec, fixed-width little-endian (decoder at VA `0x172bc4c`) | recovered | `Value`, `decode` |
| The 855 PropIds, 0 to 854, by name (tables at VA `0x5d9fcf0` and `0x5da3260`) | recovered | `PropId`, `propid-names.tsv` |
| The element set (`div`, `text`, `input`, `list`, `canvas`, `navigator`, `motion_events`), kinds as name strings | recovered | `ElementKind` |
| `view_props` keyed by node and PropId, and the keyed diff of two `u32` child lists | recovered in shape | `ViewProps`, `diff_children` |
| The accessibility PropIds, the 22 roles and the two `liveRegion` values | recovered | `a11y` |
| The turn framing: op-codes 0 to 3 (`Create`, `SetProps`, `SetChildren`, `Remove`) and the op-count header | modelled | `turn` |
| Which value tag each PropId carries | modelled, never paired | |
| How `view_props` stores a cell, and how the diff minimises moves | modelled | `ViewProps`, `diff_children` |
| How a platform turns accessibility cells into a tree | modelled | `a11y::semantics` |
| The motion engine's 26 channels, `declare`'s resting values and defaults, a per-view spec through `view_motion`, the per-frame tick, `set_reduced` | recovered | `motion` |
| The spec's carriage (`viewMotion` as an object keyed by channel PropId), the eased and spring curves, and the frame word stream's layout | modelled | `motion::Spec`, `motion::Curve`, `motion::FrameOut` |
| `ldh_dispatch`'s shape: one event as a byte buffer from host to guest, an optional reply back as bytes | recovered | `event` |
| The event buffer's layout (`u32` node, kind byte, a payload value), the press event, and the reply as one value | modelled | `event::Event` |
| The filter verdicts (pass, replace, consume) | rekindle's own | `event::FILTER_PASS` and the rest |
| The capture format | rekindle's own | `capture` |

Each modelled item's docs start with "Modelled." and say what would replace
it. A turn's bytes mix the two: the value codec inside each `SetProps` is
recovered, and the op codes and the op-count header around it are modelled.

The PropId table is compiled in from `propid-names.tsv`. PropIds are
appended in alphabetical batches and never renumbered, so a newer build's
table extends this one, and an id past 854 still decodes with no name.

The crate is pure std. Nothing in it is behind a cargo feature: the parts
only a mock client uses (`motion::Motion::frame`, `a11y::semantics`) cost
nothing in a host that doesn't call them.
