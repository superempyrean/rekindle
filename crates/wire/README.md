# rekindle-wire

The wire a split GPUI client speaks between its sandboxed wasm guest and its
native host. The guest's runtime reconciles its views against a retained
element tree and encodes them as one op buffer per turn. On Phoenix's Android
build the platform's views apply those ops and paint. Whether a desktop build
paints them in the guest's runtime instead is not known yet. rekindle's host
links this crate to read and rewrite those turns.

The recovered parts are read out of Phoenix's Android host library
(`liblibdiscord_wasm_host.so`, build 356207), which compiles the guest in,
and the guest's writer functions were emulated to check the bytes. The
modelled parts are rekindle's stand-in, kept inside what the recovered parts
allow.

| Piece | Status | Where |
|---|---|---|
| The 13-tag value codec, fixed-width little-endian (the decoder at VA `0x172bc4c`, the encoder at `0x1751a88`) | recovered | `Value`, `decode` |
| Integers as tag 3 when they fit an `i32`, colours as integers `0xAARRGGBB`, tag 9 as the insets of `padding`, `margin` and `hitSlopEdges`, left, top, right and bottom | recovered | `Value::int`, `Value::color`, `Value::Quad` |
| The 855 PropIds, 0 to 854, by name (tables at VA `0x5d9fcf0` and `0x5da3260`) | recovered | `PropId`, `propid-names.tsv` |
| The turn: the version byte `0x01` and ten ops, Create to ChildEdits, with `u16` property counts, Null for a removed property, `text` first and `on` last in a Create, and when the guest sends ChildEdits | recovered | `turn` |
| Kinds as `u16`-length strings: the Android platform's 29 interned view names, in its table's order, and any other kind as a plain string | recovered | `ElementKind::PLATFORM` |
| The rest of a child list after the last ChildEdits edit, which the platform copies | recovered | `turn::apply_edits` |
| The words table of tags 11 and 12: one per buffer, a tag 12 naming a list by its place in the same buffer | recovered | `Cursor`, `Turn::own_words` |
| What turns the words table on: `WireWords` in the `capabilities` of the platform's `DeviceInfo.constants` reply, which the Android platform always sends. Without it the guest sends `u32` arrays as tag 7, which `Value::words` writes; `Value::words_tabled` writes tag 11 | recovered | `Value::words` |
| UI events: the leading `u32` 0, the version byte, and ops 1 to 9 (`ui`, `back`, `keyCommand`, `insets`, `window`, `frameInterval`, `frame`, `motion`, `navSides`), with what the guest's app drops or clamps | recovered | `event::UiEvent` |
| What the pairs of event op 8 (`motion`) mean | open, kept as words | `event::UiEvent::Motion` |
| A reply to a consumed event, as one value (Phoenix's dispatch returns no data for an event) | modelled | `event::FILTER_CONSUME` |
| Host frames: the 16-byte header, CALL, REPLY, EVENT and BATCH, a REPLY echoing its CALL, an EVENT with module and call id 0, and a BATCH with an all-zero header | recovered | `frame` |
| `ldh_dispatch`'s shape: one event as a byte buffer from host to guest, null back when the guest took it and an error object otherwise | recovered | `event` |
| `view_props` keyed by node and PropId, and the guest's diff of two `u32` child lists | recovered in shape | `ViewProps`, `diff_children` |
| How `view_props` stores a cell | modelled | `ViewProps` |
| The accessibility PropIds, the 22 roles, the two `liveRegion` values and the value each is written with | recovered | `a11y` |
| How a platform turns accessibility cells into a tree | modelled | `a11y::semantics` |
| The motion engine's 26 channels, `declare`'s resting values and defaults, a per-view program through `view_motion`, the per-frame tick, `set_reduced` | recovered | `motion` |
| The op stream `apply` and `view_motion` take: 32 opcodes, their operands, and the spec grammar (timing with six easings, spring, decay, delay, sequence, repeat, reduced-motion-exempt). `viewMotion` holds it as a tag-7 array of integers, one per word, as the real guest's `[21, 9, 5, 0xffffffff, 0]` | recovered | `motion::op` |
| Ops 3 and 4 (bind and unbind a worklet), 21 (a value follows a platform input, such as a view's pressed state), 24 (the layout transition) and 28 (the size transition), and the platform's input kinds | recovered | `motion::op::code`, `Follow`, `LayoutTransition`, `motion::op::input` |
| What the other opcodes' operands mean | open, kept as words | `motion::op::Op` |
| The spring's closed form, its settle rules and the 64 ms cap on a frame, matched frame by frame to the engine under emulation | recovered | `motion::spring` |
| The `worklet` cell: a worklet id, then its arguments, sent to the engine as op 3. The `press_look` worklet on the real guest's buttons, with its colours | recovered | `motion::worklet` |
| Which ops a `Spec` is (one op 24 an entry, with words of rekindle's), when a timed transition settles, and the frame word stream's layout | modelled | `motion::Spec`, `motion::Curve`, `motion::FrameOut` |
| `spans` and `valueSpans`: arrays of pieces, each an object with a string `text` and style keys (`color`, `weight`, `font`, `bold`, `italic`, `underline`, `strike`, `code`, `bg`, `link`, `image` and the rest, with the types the Android platform reads them as), whose texts concatenate to the text shown | recovered | `text::Span` |
| An input's `forceValue` (`seq`, `text`, `start`, `end`, optional `eventCount`), applied once per `seq`, and `value` ignored while a `changeText` is unanswered | recovered | `text::ForceValue` |
| Which of tags 8 and 10 the guest writes for a span piece | modelled (tag 8) | `text::Span::to_value` |
| The filter verdicts (pass, replace, consume) | rekindle's own | `event::FILTER_PASS` and the rest |
| The capture format | rekindle's own | `capture` |

Each modelled item's docs start with "Modelled." and say what would replace
it.

The PropId table is compiled in from `propid-names.tsv`. PropIds are
appended in alphabetical batches and never renumbered, so a newer build's
table extends this one, and an id past 854 still decodes with no name.

UI ops must not be sent from the Android main thread. The platform's
calls that need the main thread post to it and block the calling thread
until it runs, and the guest can hold its UI lock while it waits on such a
call, so a main-thread sender would wait on a lock whose holder waits on
it.

The crate is pure std. Nothing in it is behind a cargo feature: the parts
only a mock client uses (`motion::Motion::frame`, `a11y::semantics`) cost
nothing in a host that doesn't call them.
