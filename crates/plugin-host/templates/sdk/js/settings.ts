// rekindle/settings: your plugin's settings, declared as a schema in
// package.json and kept by the client, which renders the settings page and
// validates every value (no capability; works in the render isolate too).
//
//   "rekindle": {
//     "settings": [
//       { "key": "keywords", "type": "string-list", "label": "Keywords", "default": ["rekindle"] },
//       { "key": "tint", "type": "toggle", "label": "Tint the row", "default": false }
//     ]
//   }
//
//   const words = settings.items('keywords');
//   settings.onChange('tint', () => refresh());
//
// Types: toggle, select, slider, number, text, secret, color, keybind,
// string-list, table, users, channels, guilds. A secret is text the settings
// page masks, such as an API key; read it with `text`. Changes arrive in `onSettingsChanged(key)`
// (and `onChange` callbacks); renders re-run by themselves.

import * as raw from 'rekindle:plugin/settings@0.11.0';
import type { Color } from 'rekindle:plugin/types@0.11.0';
import { __registry, call } from 'rekindle';

export type SettingValue = raw.SettingValue;

/** The current value (the schema default if the user never changed it). */
export function get(key: string): SettingValue | undefined {
  return raw.get(key);
}

/**
 * Set from code (validated against the schema; fires `onSettingsChanged`
 * once the current call returns). Booleans, numbers, strings and string
 * arrays convert to the matching value; pass a `SettingValue` for the rest.
 */
export function set(key: string, value: SettingValue | boolean | number | string | string[]): void {
  const v: SettingValue =
    typeof value === 'boolean'
      ? { tag: 'boolean', val: value }
      : typeof value === 'number'
        ? { tag: 'number', val: value }
        : typeof value === 'string'
          ? { tag: 'text', val: value }
          : Array.isArray(value)
            ? { tag: 'items', val: value }
            : value;
  call('settings.set', () => raw.set(key, v));
}

/** All keys in the schema. */
export function keys(): string[] {
  return raw.keys();
}

/** Call `handler(key)` when `key` changes, before `onSettingsChanged`. */
export function onChange(key: string, handler: (key: string) => void): void {
  __registry.settings.set(key, handler);
}

/** A toggle (`false` if unset or not a toggle). */
export function bool(key: string): boolean {
  const v = raw.get(key);
  return v?.tag === 'boolean' ? v.val : false;
}

export function number(key: string): number | undefined {
  const v = raw.get(key);
  return v?.tag === 'number' ? v.val : undefined;
}

/** A text or keybind setting (`''` if unset). */
export function text(key: string): string {
  const v = raw.get(key);
  return v?.tag === 'text' || v?.tag === 'keybind' ? v.val : '';
}

/** A select's chosen index. */
export function selected(key: string): number {
  const v = raw.get(key);
  return v?.tag === 'selected' ? v.val : 0;
}

export function color(key: string): Color | undefined {
  const v = raw.get(key);
  return v?.tag === 'color' ? v.val : undefined;
}

/** A string-list. */
export function items(key: string): string[] {
  const v = raw.get(key);
  return v?.tag === 'items' ? v.val : [];
}

/** A table's rows. */
export function rows(key: string): string[][] {
  const v = raw.get(key);
  return v?.tag === 'rows' ? v.val : [];
}

/** A users / channels / guilds picker's ids. */
export function ids(key: string): bigint[] {
  const v = raw.get(key);
  return v?.tag === 'ids' ? [...v.val] : [];
}
