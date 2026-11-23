// rekindle/presence: your presence (needs `presence`): status, custom status,
// nickname, rich presence. `setStatus` and `setNickname` are gesture-bound
// like `rekindle/local-actions`; `setCustomStatus` and `setRichPresence` are
// your own activity and work any time (a timer, an event), at most 5 changes
// per 20 s.

import * as raw from 'rekindle:plugin/presence@0.10.0';
import { call, optU64, u64, type Id } from 'rekindle';

export type RichPresence = raw.RichPresence;

/** `'online'`, `'idle'`, `'dnd'` or `'invisible'`. */
export function setStatus(status: string): void {
  call('presence.setStatus', () => raw.setStatus(status));
}

export function setCustomStatus(text?: string, emoji?: string, expiresMs?: Id): void {
  call('presence.setCustomStatus', () => raw.setCustomStatus(text, emoji, optU64(expiresMs)));
}

export function setNickname(guild: Id, nick?: string): void {
  call('presence.setNickname', () => raw.setNickname(u64(guild), nick));
}

/** A rich-presence activity of your own (`undefined` clears it). Images are your registered ones. */
export function setRichPresence(activity: RichPresence | undefined): void {
  call('presence.setRichPresence', () => raw.setRichPresence(activity));
}

/** A "Playing <name>" activity with nothing else set. */
export function playing(name: string): RichPresence {
  return {
    kind: 0,
    name,
    details: undefined,
    state: undefined,
    startedMs: undefined,
    endsMs: undefined,
    largeImage: undefined,
    largeText: undefined,
    smallImage: undefined,
    smallText: undefined,
    buttons: [],
  };
}
