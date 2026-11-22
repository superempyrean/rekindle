# Contributing

This page covers setting up, building and testing rekindle, and how changes
are made. The crates are listed in [README.md](README.md), "Workspace
layout".

## Setup

- **Rust** through rustup. `rust-toolchain.toml` pins the compiler for the
  root workspace, with the wasm target and clippy, so the same source gives
  the same plugin file on every machine. `client/` pins its own
  (`client/rust-toolchain.toml`). rustup installs both on first use.
- **Node.js and npm**, for the TypeScript plugins and their tests:
  `npm install` in `plugins/`. Without it those tests print why and pass;
  `REKINDLE_REQUIRE_GUEST_TOOLCHAINS=1` makes that a failure, as CI does.
- Windows for the launcher. `client/` builds on Windows and Linux, and the
  root workspace also builds on
  Linux and macOS.

## Two workspaces

The root workspace is the plugin platform, the CLI, the launcher and the
hook-point resolvers. `client/` is a separate workspace for what ships into
the client, built with the client's toolchain.

```sh
cargo test --workspace
cargo clippy --workspace --all-targets

cd client
cargo build
cargo clippy --workspace
```

The plugin host's tests build the example plugins to wasm, so the first run
is slow. Run one target while working (`cargo test -p rekindle-plugin-host
--test updates`) and the whole workspace before opening a pull request. On a
machine with little memory, set `CARGO_BUILD_JOBS=1`.

The host DLL is tested live against a mock GPUI client, which isn't in this
repository. Its pull request checks build and lint `client/`, and a
maintainer runs the live self-test on changes there.

Never test a client by moving the mouse, sending keys or taking focus.

## Pull requests

- Keep each pull request to one change: a plugin, a host function, a fix.
  Small and scoped is easier to review than one large change.
- Run the tests and clippy for what you touched. CI lints both workspaces
  with `-D warnings`. On a pull request it also builds `client/` on
  Windows and tests the launcher and the hook crate there.
- Commits have one author, the person who made them. Don't add
  `Co-Authored-By` or "generated with" trailers for tools or AI assistants.
- A contribution is licensed under the repository's license,
  GPL-3.0-or-later. Commits need no sign-off, and there is no contributor
  agreement.
- A change to the plugin contract (`crates/plugin-api/wit/`) updates
  `crates/plugin-api/abi-snapshot.txt`.

To list a plugin in the community index, submit it to
`superempyrean/rekindle-index` instead.

## Rules that need the owner

These hold unless the owner decides otherwise. Open an issue before a pull
request that changes them.

- The plugin sandbox invariants and the trust model. In short: no ambient
  imports, every host call checked against a capability, network only to
  declared hosts, actions only during a user gesture, and the host never
  exposes the user's credentials.
- rekindle never evades or hides from antivirus.
- rekindle keeps no copy of the client's state. Plugins read the client.
- The project signing key stays on the machine that signs. CI never holds
  it.
