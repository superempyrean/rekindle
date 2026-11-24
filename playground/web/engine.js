// The wasm module: rekindle's wire engine, built from rekindle-wire-inject.
// See playground/src/abi.rs for the functions it exports.

const enc = new TextEncoder();
const dec = new TextDecoder();

export async function loadEngine(url = "playground.wasm") {
  const { instance } = await WebAssembly.instantiateStreaming(fetch(url), {});
  const x = instance.exports;
  const put = (bytes) => {
    const ptr = x.pg_alloc(bytes.length);
    new Uint8Array(x.memory.buffer, ptr, bytes.length).set(bytes);
    return [ptr, bytes.length];
  };
  const text = () => dec.decode(new Uint8Array(x.memory.buffer, x.pg_out_ptr(), x.pg_out_len()));
  const check = (ok) => {
    if (!ok) throw new Error(text());
  };

  class Session {
    // `capture`: the bytes of a wire capture (rekindle_wire::capture).
    constructor(capture) {
      this.h = x.pg_open(...put(capture));
      if (!this.h) throw new Error(text());
    }
    get turns() {
      return x.pg_turns(this.h);
    }
    replay(n) {
      check(x.pg_replay(this.h, n));
    }
    // A Work, as rekindle-wire-inject reads it from JSON.
    setWork(work) {
      check(x.pg_set_work(this.h, ...put(enc.encode(JSON.stringify(work)))));
    }
    // A theme: {binding, palette, tokens} (playground/src/theme.rs). Returns
    // each bound token's colour in the palette and the recolours made.
    setTheme(theme) {
      check(x.pg_set_theme(this.h, ...put(enc.encode(JSON.stringify(theme)))));
      return JSON.parse(text());
    }
    // The host's tree (what the client paints) or the guest's.
    tree(mirror = true) {
      check(x.pg_tree(this.h, mirror ? 1 : 0));
      return JSON.parse(text());
    }
    errors() {
      check(x.pg_errors(this.h));
      return JSON.parse(text());
    }
    close() {
      x.pg_close(this.h);
    }
  }
  return { Session };
}
