# rekindle

[rekindle.dev](https://rekindle.dev) · <a href="https://discord.gg/BHus4dw7g7"><img src="https://discord.com/api/guilds/1569161099725639781/widget.png?style=shield">

Compatibility matrix:

| Platform | Status | Notes |
| --- | --- | --- |
| Android | 🚧 WIP | Soon! |
| iOS | 🚧 WIP | Soon! |
| Windows | 🚧 WIP | Soon! |
| Linux | 🚧 WIP | Soon! |
| macOS | 🗓️ Planned | No public Phoenix build available yet. |

rekindle is a client mod and plugin toolkit for Discord's upcoming rewrite
in Rust/GPUI (codename Phoenix). Our goal is to restore themes, plugins, UI
additions, and quality-of-life improvements as quickly and robustly as possible.

rekindle works by loading alongside Discord's native binary as a DLL and
patching key functions to enable the types of behavior that exist in
the plugins you would find in Vencord, Equicord, BetterDiscord, etc.

Plugins are sandboxed WASM components. rekindle injects a host into the
target application which loads plugins and provides a stable SDK. We plan on
providing a rich suite of first-party plugins immediately, with a community index
that anyone can contribute to.

The Discord rewrite has, of course, not yet been released. However, we can
prepare for it by developing and testing against software that already uses
GPUI like Zed and test applications of our own.

> [!WARNING]
> The text below and the docs are still very AI sloppy. This will be remedied :3

## Workspace layout

| Path | Crate | Purpose |
|------|-------|---------|
| **Plugins** | | |
| `crates/plugin-api`   | `rekindle-plugin-api` | The `wit/` interface contract and generated Rust bindings. The stable boundary plugins target. |
| `crates/sdk`          | `rekindle-sdk` | The API plugin authors use. |
| `crates/test-client`  | `rekindle-test` | What a Rust plugin's tests use under `rekindle test`. |
| `crates/plugin-host`  | `rekindle-plugin-host` | Embeds wasmtime, loads plugin components and enforces per-plugin capabilities; first-party signing, the community index and store, developer mode and the install screen; the test harness. |
| `crates/cli`          | `rekindle-cli` | The `rekindle` command: scaffold, build, preview, test, sign, install, submit to the community index. |
| `playground/`         | `rekindle-playground` | The theme playground: the client's screens from wire captures, recoloured in the browser by `rekindle-wire-inject` built to wasm as a theme's tokens are edited ([playground/README.md](playground/README.md)). |
| `plugins/`            | | Example plugins in `rust/` and their TypeScript ports in `ts/`, and theme files in `themes/`. |
| **Shipping** | | |
| `crates/install`      | `rekindle-install` | The install's layout and `state.json`: which version runs, and the fallback from one that doesn't come up. Shared by the launcher, the shim and the host. |
| `crates/release`      | `rekindle-release` | Signing, the release channel's `framework` and `launcher` sections, and updating an install from the channel. Shared by the plugin host, the host and the launcher. |
| `crates/launcher`     | `rekindle-launcher` | The setup program and the installed `rekindle.exe`: installs, repairs and updates rekindle beside the client. |
| `crates/wire`         | `rekindle-wire` | The wire a split GPUI client speaks between guest and host, as read from Phoenix's Android host: the value codec, the PropId table, the turn's ten ops, UI events and host frames ([crates/wire/README.md](crates/wire/README.md)). |
| `crates/wire-inject`  | `rekindle-wire-inject` | rekindle's transforms on that wire, which the host runs from its detours: subtrees of its own mounted under the client's nodes, cells written by PropId, the guest's nodes dropped or rewritten, presses intercepted, and colours recoloured where the guest writes them, as a `Work` value says. |
| `client/`             | | What rekindle ships into the client (separate workspace): the side-load shim, the host DLL and the in-client runtime. [client/README.md](client/README.md). |
| **Finding hook points** | | |
| `crates/hook`         | `rekindle-hook` | The attach gate and reversible x64 inline detours. |
| `crates/anchor`       | `rekindle-anchor` | Hook points as anchors (redundant locators normalized to the function's entry), resolved against each client build. |
| `crates/sigmatch`     | `rekindle-sigmatch` | Relocation-masked byte signatures, and the shortest unique signature for an address. |
| `crates/analysis`     | `rekindle-analysis` | Binary triage: the Rust compiler version and vendored crate versions of a release binary, and its sections. |

## Building

```sh
cargo build --workspace                # the plugin platform, the CLI, the launcher
cargo test --workspace

cd client
cargo build                            # the shim and the host DLL
```

[CONTRIBUTING.md](CONTRIBUTING.md) has the setup and the rules for changes.

## Scope

rekindle is customization tooling: themes, UI additions and quality-of-life
plugins. For now, the community index refuses only plugins that are actively
malicious. The rest of its policy will be decided as plugins are submitted.

Modifying the client is against Discord's Terms of Service, and using any
client mod carries account risk.

## Status

Pre-alpha. The plugin platform works end to end against the mock client.
Plugins are scaffolded with `rekindle new`, previewed with `rekindle dev`, and
then signed and installed. Loading into Phoenix waits for a Phoenix build.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
