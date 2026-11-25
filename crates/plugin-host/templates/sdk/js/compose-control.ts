// rekindle/compose-control: send the user's message as several (needs
// `compose`). Only within `onCompose`, for a send.
//
//   onCompose(draft) {
//     if (draft.kind === 'send' && draft.text.length > 2000) split(chunks(draft.text, 2000));
//     return compose.keep();
//   }
//
// The host sends the parts in order instead of the one message, with the
// reply, attachments and stickers on the first. A text rewrite returned
// from the same call is ignored. If a plugin after yours changes the text,
// the split is dropped and that text goes as one message.

import * as raw from 'rekindle:plugin/compose-control@0.11.0';
import { call } from 'rekindle';

export type SplitError = raw.SplitError;

/** Send the message as `parts` (2 to 10, each one sendable message), in order. Throws a RekindleError (`not-sending`, `part-count`, `bad-part`). */
export function split(parts: string[]): void {
  call('compose.split', () => raw.split(parts));
}
