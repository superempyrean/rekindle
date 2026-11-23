// rekindle/moderation: moderating other members (needs `moderate`): kick,
// ban, timeout, server mute, deafen and move, bulk delete. Gesture-bound
// like `rekindle/actions`. Each call needs the Discord permission for it,
// and the host asks the user to confirm.
//
//   moderation.timeout(guild, user, Date.now() + 600_000, 'spam');

import * as raw from 'rekindle:plugin/moderation@0.9.0';
import { call, optU64, u64, type Id } from 'rekindle';

export function kick(guild: Id, user: Id, reason?: string): void {
  call('moderation.kick', () => raw.kick(u64(guild), u64(user), reason));
}

/** Deletes the user's messages from the last `deleteMessageSecs` seconds (up to 7 days). */
export function ban(guild: Id, user: Id, deleteMessageSecs = 0, reason?: string): void {
  call('moderation.ban', () => raw.ban(u64(guild), u64(user), deleteMessageSecs, reason));
}

export function unban(guild: Id, user: Id): void {
  call('moderation.unban', () => raw.unban(u64(guild), u64(user)));
}

/** Time out until `untilMs` (epoch ms, up to 28 days ahead), or end a timeout with `undefined`. */
export function timeout(guild: Id, user: Id, untilMs: number | undefined, reason?: string): void {
  const until = untilMs === undefined ? undefined : BigInt(Math.floor(untilMs));
  call('moderation.timeout', () => raw.timeout(u64(guild), u64(user), until, reason));
}

export function setVoiceMute(guild: Id, user: Id, muted: boolean): void {
  call('moderation.setVoiceMute', () => raw.setVoiceMute(u64(guild), u64(user), muted));
}

export function setVoiceDeaf(guild: Id, user: Id, deafened: boolean): void {
  call('moderation.setVoiceDeaf', () => raw.setVoiceDeaf(u64(guild), u64(user), deafened));
}

/** Move to a voice channel, or disconnect with `undefined`. */
export function moveMember(guild: Id, user: Id, channel: Id | undefined): void {
  call('moderation.moveMember', () => raw.moveMember(u64(guild), u64(user), optU64(channel)));
}

/** 2 to 100 messages, each under 14 days old. */
export function bulkDelete(channel: Id, messages: Id[]): void {
  call('moderation.bulkDelete', () => raw.bulkDelete(u64(channel), messages.map(u64)));
}
