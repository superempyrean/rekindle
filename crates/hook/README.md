# rekindle-hook

rekindle-hook decides whether rekindle may attach to a client and installs
reversible detours on the client's functions. The host DLL and the
in-client runtime (`client/`) use it from inside the client, which the
side-load shim loads them into. The host finds the functions to hook with
`rekindle-anchor`.

| Module | Job |
|---|---|
| `guard` | The attach gate: a SHA-256 allowlist (`REKINDLE_ATTACH_ALLOW`). An empty list refuses every image. |
| `hook` | A reversible x64 inline detour on a live function. |

```rust
use rekindle_hook::{Detour, InlineDetour, guard};

guard::verify(client, &guard::AllowList::from_env())?;
let installed = unsafe { InlineDetour.install(target, hooked as *const () as usize)? };
let original: extern "C" fn(u64) -> u64 =
    unsafe { std::mem::transmute(installed.trampoline()) };
```

## What it refuses

- No allowlist, or a fingerprint that is not on it.
- A prologue containing control flow or a RIP-relative operand. Moving one
  into a trampoline would need fixups, which aren't written.
- A hook out of `rel32` reach of its target when no executable memory is
  free within reach for a thunk.

A refusal is always an [`anyhow::Error`], never a partially applied patch.

## Platforms

x64 Windows and x64 Linux share the detour. Only `hook::mem` differs
(`VirtualAlloc`/`VirtualProtect` against `mmap`/`mprotect`). Other targets
compile, and `InlineDetour::install` returns an error.

## Tests

`cargo test -p rekindle-hook` includes a real round trip: it hooks a local
function, checks the replacement runs, unhooks, and checks the original is
back.
