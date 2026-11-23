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
- `palette.json`: the client's token values for each of its themes.

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

## Limitations

Built and tested on x86-64 Windows and Linux. The x86-64 Android build has run
in the app: the host loaded, resolved its anchors, swapped the emit slot and
logged render turns and host calls crossing it. The x64 inline detours the
desktop surfaces use return an error on AArch64, so the emit hook is a slot
swap and the engine runs from it. On Linux the clipboard and the
file picker are not implemented yet, and the native layer's check against the
window reads X11, so a client running on Wayland has to use XWayland.
