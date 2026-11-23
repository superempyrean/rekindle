// rekindle/history: a channel's history past what the client has loaded
// (needs `history`, and each call needs `read-messages`). Up to 1000
// messages a call; pass a page's `cursor` as the next `before`.
//
//   let page = history.before(channel, undefined, 500);
//   while (page.cursor !== undefined) page = history.before(channel, page.cursor, 500);

import * as raw from 'rekindle:plugin/history@0.9.0';
import { call, optU64, u64, type Id } from 'rekindle';

export type Page = raw.Page;

/** Newest first, older than `before`, or the newest page. */
export function before(channel: Id, before: Id | undefined, limit = 100): Page {
  return call('history.before', () =>
    raw.query({ channel: u64(channel), before: optU64(before), after: undefined, limit }),
  );
}

/** Oldest first, newer than `after`. */
export function after(channel: Id, after: Id, limit = 100): Page {
  return call('history.after', () =>
    raw.query({ channel: u64(channel), before: undefined, after: u64(after), limit }),
  );
}
