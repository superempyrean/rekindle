// rekindle/events: react to client events by kind (no capability to
// register; each kind is delivered only with its capability: client structure
// and your own state with `read-client`, other people with `read-users`,
// message content with `read-messages`, the cursor with `pointer`).
//
//   events.on('message-created', (message) => { ... message.content ... });
//   events.on('selection-changed', (sel) => { ... sel.channel ... });
//   events.subscribe(['message']);   // only message events reach the plugin
//
// The callback gets the event's value; `onEvent(ev)` still sees every event.
// By default the plugin receives every kind it's granted; `subscribe`
// narrows that to the kinds it handles.

import * as raw from 'rekindle:plugin/host-events@0.10.0';
import type { Event, EventKind } from 'rekindle:plugin/events@0.10.0';
import { __registry } from 'rekindle';

export type { Event, EventKind };
export type {
  ConnectionState,
  DeletedMessage,
  GuildLeftReason,
  ReactionChange,
  RelationshipChange,
  SelectionChange,
} from 'rekindle:plugin/events@0.10.0';
export type SubscribeError = raw.SubscribeError;

/** An event's tag (`'message-created'`, `'typing'`, ...). */
export type EventTag = Event['tag'];
type Of<K extends EventTag> = Extract<Event, { tag: K }>;
type ValueOf<K extends EventTag> = Of<K> extends { val: infer V } ? V : undefined;

/** Call `handler` with the value of each event tagged `tag`, before `onEvent`. Replaces an earlier handler for it. */
export function on<K extends EventTag>(tag: K, handler: (value: ValueOf<K>, event: Of<K>) => void): void {
  __registry.event.set(tag, handler as (value: unknown, event: Event) => void);
}

export function off(tag: EventTag): void {
  __registry.event.delete(tag);
}

/**
 * Receive only these kinds (`'message'`, `'typing'`, `'read-state'`, ...).
 * Returns the kinds that were refused, with why (`'not-granted'`,
 * `'unavailable'`).
 */
export function subscribe(kinds: EventKind[]): Array<[EventKind, SubscribeError]> {
  const refused: Array<[EventKind, SubscribeError]> = [];
  for (const [kind, r] of raw.subscribe(kinds)) if (r.tag === 'err') refused.push([kind, r.val]);
  return refused;
}

/**
 * Have `onIntercept` see these kinds before the client processes them
 * (needs `intercept`, and each kind's own capability). Returns the kinds
 * that were refused: `'not-granted'`, or `'unavailable'` for kinds that
 * don't come from Discord.
 */
export function intercept(kinds: EventKind[]): Array<[EventKind, SubscribeError]> {
  const refused: Array<[EventKind, SubscribeError]> = [];
  for (const [kind, r] of raw.intercept(kinds)) if (r.tag === 'err') refused.push([kind, r.val]);
  return refused;
}

/**
 * Has `onRequest` and `onResponse` see the client's REST requests on routes
 * of these kinds (needs `intercept-http`, and each kind's own capability).
 * Returns the kinds that were refused.
 */
export function interceptHttp(kinds: EventKind[]): Array<[EventKind, SubscribeError]> {
  const refused: Array<[EventKind, SubscribeError]> = [];
  for (const [kind, r] of raw.interceptHttp(kinds)) if (r.tag === 'err') refused.push([kind, r.val]);
  return refused;
}
