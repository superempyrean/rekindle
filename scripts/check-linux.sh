#!/usr/bin/env bash
# The root job of .github/workflows/ci.yml: format, clippy and tests of the
# root workspace on Linux, with the TypeScript toolchain required. On
# Windows, scripts/check.ps1 runs it in WSL.
#
#   --mirror   copy the working tree to target/linux/src and check the copy.
#              plugins/node_modules on a Windows checkout holds Windows
#              binaries, so WSL needs a tree with its own.
#   --no-fmt   skip the format check (check.ps1 has already run it).
set -euo pipefail

# wsl.exe -e starts no login shell, so rustup's PATH entry may be missing.
if ! command -v cargo >/dev/null && [[ -f $HOME/.cargo/env ]]; then
    . "$HOME/.cargo/env"
fi

repo=$(cd "$(dirname "$0")/.." && pwd)
mirror=0
fmt=1
for arg in "$@"; do
    case "$arg" in
        --mirror) mirror=1 ;;
        --no-fmt) fmt=0 ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

src=$repo
if [[ $mirror == 1 ]]; then
    src=$repo/target/linux/src
    export CARGO_TARGET_DIR=$repo/target/linux/target
    mkdir -p "$src"
    # -a keeps modification times, so cargo rebuilds only what changed.
    rsync -a --delete --exclude=/.git --exclude=target/ --exclude=node_modules/ \
        "$repo/" "$src/"
fi
cd "$src"

export REKINDLE_REQUIRE_GUEST_TOOLCHAINS=1
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}

# rust-toolchain.toml pins the compiler, its wasm target and clippy.
rustup toolchain install

# npm ci, as CI runs it, again only when the lockfile changes.
lock_sum=$(sha256sum plugins/package-lock.json | cut -d' ' -f1)
stamp=plugins/node_modules/.rekindle-lock
if [[ ! -f $stamp || $(cat "$stamp") != "$lock_sum" ]]; then
    (cd plugins && npm ci)
    echo "$lock_sum" > "$stamp"
fi

if [[ $fmt == 1 ]]; then
    cargo fmt --all --check
fi
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
