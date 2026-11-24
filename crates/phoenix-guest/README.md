# phoenix-guest

Runs the guest that is compiled into Phoenix's Android host library
(`liblibdiscord_wasm_host.so`, x86_64) on Windows or on glibc Linux, with no
Android userland. The library is a crate that `client/phoenix-view` links,
and `guest-headless` is a driver that records every buffer the guest emits.
You supply the host library from a Discord Android build you have a right to
use; none is distributed here.

```
cargo run -p phoenix-guest --bin guest-headless -- <workdir> <liblibdiscord_wasm_host.so> 30 PRESS=17
```

`guest-headless` copies the library into `<workdir>/lib` and writes each
run to `<workdir>/out/<time>`, named in `out/last.txt`:

| File | What it is |
|------|------------|
| `log.txt` | The loader's log: imports, files, threads, every emit in hex, the replies, and import call counts at the end |
| `turns.bin` | Every emit, as a `u32` length and the bytes |
| `capture.bin` | The render turns, in the capture format the wire tooling reads with property names |

`PRESS=<node>` sends a press after ten frames, `TRACE=0|1|2` sets import
logging (1 logs the first 20 calls of each import), `NO_REPLY=1` answers no
host call, and `NO_BATCH=1` leaves `UIManager.batchEvents` unanswered.

On Windows the guest's paths map into `<workdir>/root`, so `/etc/os-release`
is `<workdir>/root/etc/os-release`, and its `HOME` is `/sandbox`. On Linux
it reads the real file system with `HOME=<workdir>/sandbox`. Either way its
environment holds only `HOME` and `PATH`. A fault ends the run with a
`[fault]` line in the log, and on Windows no error dialog opens.

Nothing here contacts a server. `socket`, `connect`, `getaddrinfo` and the
network syscalls are refused and logged, and `Networking.request` calls stay
unanswered. Secure Storage keys are never answered.

For Linux, build inside WSL with a target directory of its own:

```
CARGO_TARGET_DIR=~/pg-target cargo +1.98 build -p phoenix-guest
```

| File | What it is |
|------|------------|
| `src/lib.rs` | The API: `load`, `Library::create`, `Guest::start` and `dispatch`, the `Transport` trait and `Reply` |
| `src/loader.rs` | Maps the segments, applies the APS2 packed relocations and `.rela.plt`, wraps every import in a logging trampoline, and runs `DT_INIT_ARRAY` |
| `src/elf.rs` | The ELF headers, APS2 decoding and the stack-guard rewrite |
| `src/valist.rs` | The System V `va_list`, and the naked functions that build one for a variadic import |
| `src/printf.rs`, `src/scanf.rs`, `src/num.rs` | bionic's printf, sscanf, `strtol` and `strtod`, for Windows |
| `src/win/` | Windows: guest stacks, faults, `setjmp`, and every import as a System V shim with bionic's types |
| `src/linux/` | Linux: the bionic symbols glibc lacks or defines differently, and the network refusals. Other imports resolve to glibc |
