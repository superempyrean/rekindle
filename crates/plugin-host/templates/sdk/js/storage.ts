// rekindle/storage: per-plugin key/value data (needs `storage`).
//
// Values are strings; `getOr` and `getJson`/`setJson` convert. (Under QuickJS
// an absent value arrives as `null`; these functions return `undefined`.)
// Storage is per plugin *and per account*: switching accounts swaps it
// underneath you. User-facing options belong in the settings schema
// (`rekindle/settings`) instead. Quotas are enforced by the client: `set`
// throws a RekindleError ('quota-exceeded', 'key-too-long',
// 'value-too-large') when one is hit. The render isolate may read, not write.

import * as raw from 'rekindle:plugin/storage@0.10.0';
import { call } from 'rekindle';

export function get(key: string): string | undefined {
  return raw.get(key) ?? undefined;
}

/**
 * The stored value, converted to `fallback`'s type (number, boolean or
 * string); `fallback` if it's missing or doesn't convert.
 */
export function getOr<T extends string | number | boolean>(key: string, fallback: T): T {
  const v = raw.get(key);
  if (v == null) return fallback;
  switch (typeof fallback) {
    case 'number': {
      const n = Number(v);
      return (v.trim() !== '' && Number.isFinite(n) ? n : fallback) as T;
    }
    case 'boolean':
      return (v === 'true' ? true : v === 'false' ? false : fallback) as T;
    default:
      return v as T;
  }
}

/** Store a value (numbers and booleans as their text). */
export function set(key: string, value: string | number | boolean): void {
  call('storage.set', () => raw.set(key, String(value)));
}

/** A JSON value, or `fallback` if missing or not valid JSON. */
export function getJson<T>(key: string, fallback: T): T {
  const v = raw.get(key);
  if (v == null) return fallback;
  try {
    return JSON.parse(v) as T;
  } catch {
    return fallback;
  }
}

export function setJson(key: string, value: unknown): void {
  set(key, JSON.stringify(value));
}

/** Delete a key (`storage.delete` works too). */
export function remove(key: string): void {
  raw.delete(key);
}
export { remove as delete };

/** Every key you've stored, sorted; only those starting with `prefix`, if given. */
export function keys(prefix?: string): string[] {
  return raw.keys(prefix);
}

/** Several values at once, in order (`undefined` for a missing one). */
export function getMany(keys: string[]): Array<string | undefined> {
  return raw.getMany(keys).map((v) => v ?? undefined);
}

/** Bytes used and allowed. */
export function usage(): { used: number; quota: number } {
  const [used, quota] = raw.usage();
  return { used: Number(used), quota: Number(quota) };
}
