// rekindle/timer: named timers that call your `onTimer(id)` (no capability).
//
// `setTimeout`/`setInterval` work too, on the same host timers. The client
// caps you at 16 active timers, 50 ms minimum, and clears them on unload. A
// timer is never a user gesture: actions from one fail with 'no-gesture'.

import * as host from 'rekindle:plugin/host@0.9.0';
import { __registry, call, u64, type Id } from 'rekindle';

/**
 * Fire once, `ms` from now: calls `callback` if given, then `onTimer(id)`.
 * Re-using an id replaces it. Throws a RekindleError ('limit', 'too-fast').
 */
export function set(id: string, ms: Id, callback?: () => void): void {
  call('timer.set', () => host.setTimer(id, u64(ms), false));
  if (callback) __registry.timer.set(id, callback);
}

/** Fire every `ms` until cleared (see `set`). */
export function repeat(id: string, ms: Id, callback?: () => void): void {
  call('timer.repeat', () => host.setTimer(id, u64(ms), true));
  if (callback) __registry.timer.set(id, callback);
}

export function clear(id: string): void {
  host.clearTimer(id);
  __registry.timer.delete(id);
}
