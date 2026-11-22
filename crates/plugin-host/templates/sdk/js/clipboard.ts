// rekindle/clipboard: write text or a PNG to the clipboard (needs `clipboard`).
//
// Only while handling one of the user's own gestures (a slot click, a palette
// command, a panel interaction); otherwise it throws a RekindleError with
// code 'no-gesture'. The clipboard is never readable.

import * as raw from 'rekindle:plugin/clipboard@0.8.0';
import { call } from 'rekindle';

export function writeText(text: string): void {
  call('clipboard.writeText', () => raw.writeText(text));
}

/** Put a PNG (at most 8 MiB) on the clipboard. */
export function writePng(png: Uint8Array): void {
  call('clipboard.writePng', () => raw.writePng(png));
}
