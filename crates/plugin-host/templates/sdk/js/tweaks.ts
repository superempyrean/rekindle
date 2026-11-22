// rekindle/tweaks: client tweaks rekindle maintains against each build
// (silent typing, always animate, no mosaic, ...; needs `tweaks`). A plugin
// can bundle a preset; a value the user set by hand wins ('user-locked').

import * as raw from 'rekindle:plugin/tweaks@0.8.0';
import { call } from 'rekindle';

export type TweakInfo = raw.TweakInfo;

export function all(): TweakInfo[] {
  return raw.all();
}

/** Set while your plugin is enabled (`'true'`, a choice, a number). */
export function set(id: string, value: string | boolean | number): void {
  call('tweaks.set', () => raw.set(id, String(value)));
}

export function reset(id: string): void {
  raw.reset(id);
}
