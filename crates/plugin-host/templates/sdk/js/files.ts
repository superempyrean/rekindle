// rekindle/files: files through the system's own dialogs (needs `files`;
// only during a user gesture). You never see a path: the user picks, the
// client reads or writes.

import * as raw from 'rekindle:plugin/files@0.8.0';
import { call } from 'rekindle';

export type Picked = raw.Picked;

/** Show a Save dialog suggesting `name`; write `data` (a string is UTF-8) where the user chose (at most 64 MiB). */
export function save(name: string, contentType: string, data: Uint8Array | string): void {
  const bytes = typeof data === 'string' ? new TextEncoder().encode(data) : data;
  call('files.save', () => raw.save(name, contentType, bytes));
}

/** Show an Open dialog for these extensions (`['json']`); the file's name, type and bytes (at most 16 MiB). */
export function open(accept: string[]): Picked {
  return call('files.open', () => raw.open(accept));
}
