// The globals a rekindle plugin has, beyond the language itself. Deliberately
// not the DOM: there is no `fetch` (use `rekindle/http`), no wall clock
// (`Date.now()` throws; use `performance.now()`), and no `crypto` randomness.

declare var console: {
  log(...data: unknown[]): void;
  info(...data: unknown[]): void;
  warn(...data: unknown[]): void;
  error(...data: unknown[]): void;
  debug(...data: unknown[]): void;
  trace(...data: unknown[]): void;
};

/** On the client's timers: at most 16 active, 50 ms minimum. */
declare function setTimeout<A extends unknown[]>(callback: (...args: A) => void, ms?: number, ...args: A): number;
declare function setInterval<A extends unknown[]>(callback: (...args: A) => void, ms?: number, ...args: A): number;
declare function clearTimeout(handle?: number): void;
declare function clearInterval(handle?: number): void;

/** The client's monotonic clock. */
declare var performance: { now(): number; readonly timeOrigin: number };

declare class TextEncoder {
  encode(input?: string): Uint8Array;
}
declare class TextDecoder {
  constructor(label?: string);
  decode(input?: ArrayBufferView | ArrayBuffer): string;
}
