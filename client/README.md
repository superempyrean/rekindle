# What ships into the client

These crates are what rekindle places beside the client and runs inside it.
They are a separate cargo workspace, pinned to the client's toolchain
(`rust-toolchain.toml`, rustc 1.98.1). rekindle builds values of the client's
own types and calls its compiled gpui, so it must use the same `std` layout
and calling conventions the client was built with.

| Path | Crate | What it is |
|---|---|---|
| `proxy/` | `rekindle-proxy` (`dwmapi.dll`) | The side-load shim. The client's own loader maps it from beside the exe, it forwards every dwmapi export to the system one, and it loads the host. |
| `host/` | `rekindle-host` (`rekindle_host.dll`) | rekindle inside the client. It resolves its anchors against the client's image and drives every surface from recovered layout. |
| `extension/` | `rekindle-extension` | The plugin runtime in the client: plugins, rekindle's panel, updates, and the live backend plugins read the client through. |
| `ui-hook/` | `rekindle-ui-hook` | The framework layer: GPUI `draw_roots` and `dispatch_event` hooks for overlays, toasts, chat-bar buttons and theming by recolouring. |
| `native/` | `rekindle-native` | rekindle's UI as real GPUI elements, built by calling the client's compiled gpui. It has no gpui dependency. |

None of these crates names the client or its types, and none depends on
gpui. Everything client-specific is data the host reads at run time, from
files beside it:

- `anchors.json`: the hook points, as anchors.
- `layout.json`: field offsets, sizes and value encodings in the client's
  types.
- `palette.json`: the client's token values for each of its themes, or,
  for a client that names its palette on the wire, where each token's
  colour is in that palette (`rekindle_wire_inject::theme`).

Those files are recovered from each client build and ship with each rekindle
version. The shim points the host at them with
`REKINDLE_HOST_ANCHORS`, `REKINDLE_HOST_LAYOUT` and `REKINDLE_HOST_PALETTE`,
unless they are set already.

## How rekindle loads

The shim and the host sit beside the client's exe. `dwmapi` isn't a
KnownDLL, so the client's loader maps the shim from its own directory during
process init, and the shim loads the host. The host starts a worker thread,
resolves its anchors, installs reversible detours and drives the client
through its own event dispatch.

On Linux the host is `librekindle_host.so`, and it is preloaded with
`LD_PRELOAD` instead. The shim is Windows-only. The host's initializer takes
itself out of `LD_PRELOAD`, so the processes the client starts don't load it,
and holds the client's `main` until the host is ready, as the shim's
`OleInitialize` gate does on Windows.

On Android the client is an app, and its image is the shared object it runs
from. The app's own loader brings the host in: a `DT_NEEDED` added to a
library the app loads early names `librekindle_host.so`, and the host's
`.init_array` starts its worker. The worker waits for the client's library
to map, resolves its anchors against a copy of it, and swaps the pointer in
the vtable slot of the guest's transport `emit` method
(`wire_transport_emit_slot`), so every turn of render ops and every host call
reaches rekindle's wire engine first. Slot swaps patch no code, so they work
on AArch64 as they do on x86-64. The client's library has no path `open` can
use (the app maps it from the APK), so the host reads its copy, its anchors
and a `Work` from a config dir (`REKINDLE_HOST_IMAGE` names the library, or
`/data/local/tmp/rekindle` by default), and takes the load bias from
`dl_iterate_phdr`. It logs its report and counts to logcat under the
`rekindle` tag.

On a phone the engine and the plugin runtime are the host's `app` module,
which the Android hooks hand turns, host calls and events to. On iOS the
host is a library instead. A tweak that hooks the client's functions links
it, calls `app::set_platform` with the config dir and a function that sends
a turn, then `app::start`, and hands `app` what passes through its hooks.
Plugins run on wasmtime's Pulley
interpreter on both, so the app never maps executable memory and iOS needs
no JIT entitlement.

## Build

```sh
cargo build                 # dwmapi.dll and rekindle_host.dll, in target/debug
cargo clippy --workspace
```

For Android, the host builds as `librekindle_host.so` with the NDK's clang as
the linker and C compiler. With `$NDK` set to the NDK's
`toolchains/llvm/prebuilt/<host>/bin`:

```sh
rustup target add aarch64-linux-android x86_64-linux-android
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$NDK/aarch64-linux-android26-clang
export CC_aarch64_linux_android=$NDK/aarch64-linux-android26-clang
export AR_aarch64_linux_android=$NDK/llvm-ar
cargo build --release -p rekindle-host --target aarch64-linux-android
```

The same variables with `x86_64` build it for x86-64 Android, which the
emulator runs. On Windows the NDK's compilers end in `.cmd`.

The live tests run against a mock GPUI client outside this repository. The
self-test places the shim and the host beside the mock, runs it off the
desktop and checks the host's report. It never activates the window or sends
OS input.

## Debug endpoint

With `REKINDLE_HOST_DEBUG_PORT` set to a port, the host serves what its
wire detours see as JSON on `127.0.0.1` at that port. It is off without
the variable, and it never listens on another interface.

| Request | What it returns |
|---|---|
| `GET /ops` | the last 200 turns the guest emitted, each op decoded with property names |
| `GET /events` | the last 200 UI events the client dispatched to the guest |
| `GET /tree` | the tree rekindle mirrors: each node's kind, children and cells |
| `GET /pick?wait=30` | waits for the next press, consumes it, and returns selectors for the pressed node |
| `GET /pick?node=N` | selectors for node `N` of the guest's tree, at once |

Host calls and host frames are never recorded, since they carry the
client's HTTP requests and responses with their headers and bodies. The
`value`, `forceValue` and `valueSpans` cells, and every string in a UI
event, appear as their length (`"<7 chars>"`), so text typed into a
password field doesn't show. The report's `debug_port` field says the
endpoint is up.

## Picking a selector

If you press an element after asking for a pick, the press doesn't reach
the guest. The host makes selectors for the pressed node from the guest's
tree instead, ranks them, and proposes a name for the element.
`rekindle pick` asks for one and prints the ranking:

```sh
rekindle pick --port 9229            # desktop, through the debug endpoint
rekindle pick --dir <config dir>     # a phone app's host
rekindle pick --capture guest.capture --node 233
```

A phone app's host has no port. It reads `pick.req` in its config dir
every second, where `press` asks for the next press, a number picks
that node and `cancel` stops waiting. The pick goes to `pick.json` beside it. On a device you reach
over SSH, write `press` to the file, tap the element, and read
`pick.json` back.

The node's own `accessibilityId` ranks first. A node without one is found
by its label, text or kind, `within` the nearest ancestors that have one.
`ChildOf` chains are offered only for an ancestor up to four levels up, and
rank lower than `within` from two levels up. Each selector comes with how
many of the tree's nodes it matches, and one that matches only the pressed
node ranks above any that doesn't. An id that ends in four digits or more
(`dm_row_1531461354488201407`) is offered whole and as a `prefix`, which
finds every row of that kind.

Phoenix draws some elements twice, with the same ids and the same subtree,
and no selector made from cells tells the copies apart. When the best
selector matches more than the pressed node, the pick leads with an `nth`
of it, which counts the matches in tree order from 0:

```json
{ "nth": { "of": { "cell": { "prop": "accessibilityId", "value": { "str": "guild-search" } } }, "n": 1 } }
```

It scores 10 points under the selector it counts, since a copy drawn ahead
of the node changes the number.

## Limitations

Built and tested on x86-64 Windows and Linux. The x86-64 Android build has run
in the app: the host loaded, resolved its anchors, swapped the emit slot and
logged render turns and host calls crossing it. The x64 inline detours the
desktop surfaces use return an error on AArch64, so the emit hook is a slot
swap and the engine runs from it. On Linux the clipboard and the
file picker are not implemented yet, and the native layer's check against the
window reads X11, so a client running on Wayland has to use XWayland.
