// Split Long Messages: a message over the length limit is sent as several,
// split at paragraph breaks where it can, else at line breaks, else between
// words, else wherever the limit falls.
//
// `compose-control.split` sends the parts in order, with the reply and any
// attachments on the first. It takes at most 10 parts, so a message that
// needs more isn't sent: the draft stays in the composer and the user is
// told why.

import { definePlugin } from 'rekindle';
import * as compose from 'rekindle/compose';
import { split } from 'rekindle/compose-control';
import * as settings from 'rekindle/settings';

const LIMITS = [2000, 4000];
const MAX_PARTS = 10;

/**
 * Where to end a part of `cps` (code points) at or before `limit`: after the
 * last paragraph break, else line break, else space, as long as the part
 * isn't tiny; else at `limit`.
 */
function breakAt(cps: string[], limit: number): number {
  for (const sep of ['\n\n', '\n', ' ']) {
    for (let i = limit; i > limit / 4; i--) {
      if (cps.slice(i - sep.length, i).join('') === sep) return i;
    }
  }
  return limit;
}

/** `text` in parts of at most `limit` characters, with the breaks they were cut at trimmed. */
function chunks(text: string, limit: number): string[] {
  const out: string[] = [];
  let rest = [...text];
  while (rest.length > limit) {
    const end = breakAt(rest, limit);
    out.push(rest.slice(0, end).join('').trimEnd());
    rest = [...rest.slice(end).join('').trimStart()];
  }
  out.push(rest.join(''));
  return out.filter((p) => p.trim().length > 0);
}

export default definePlugin({
  onCompose(draft) {
    const limit = LIMITS[settings.selected('limit')] ?? LIMITS[0];
    if (draft.kind !== 'send' || [...draft.text].length <= limit) return compose.keep();
    const parts = chunks(draft.text, limit);
    if (parts.length > MAX_PARTS) {
      return compose.cancel(`That's too long to send as ${MAX_PARTS} messages. Shorten it first.`);
    }
    split(parts);
    return compose.keep();
  },
});
