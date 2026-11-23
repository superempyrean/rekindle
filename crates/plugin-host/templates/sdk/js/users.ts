// rekindle/users: other people, as your client sees them (needs
// `read-users`, or `read-messages`, which implies it). Presence and
// activities, members, typing, voice, your relationships and notes. Nothing
// here fetches.

import * as raw from 'rekindle:plugin/users@0.10.0';
import type { Color, Member } from 'rekindle:plugin/types@0.10.0';
import type { Presence, Relationship, RelationshipKind, VoiceState } from 'rekindle:plugin/records@0.10.0';
import { optU64, u64, type Id } from 'rekindle';

export type { Activity, ClientStatus, Presence, Relationship, RelationshipKind, VoiceState } from 'rekindle:plugin/records@0.10.0';
export type { Member };

/** Presence as the client knows it (guild-scoped if `guild` is given). */
export function presence(user: Id, guild?: Id): Presence | undefined {
  return raw.getPresence(u64(user), optU64(guild));
}

/** A loaded member (never triggers a member request). */
export function member(guild: Id, user: Id): Member | undefined {
  return raw.getMember(u64(guild), u64(user));
}

/** Loaded members of a guild (up to `limit`, max 1000) and how many are loaded. */
export function loadedMembers(guild: Id, limit = 1000): [Member[], number] {
  return raw.loadedMembers(u64(guild), limit);
}

/** Another member's effective permissions in a channel or guild. */
export function memberPermissions(user: Id, channelOrGuild: Id): bigint | undefined {
  return raw.memberPermissions(u64(user), u64(channelOrGuild));
}

/** A member's display color (highest colored role). */
export function memberColor(guild: Id, user: Id): Color | undefined {
  return raw.memberColor(u64(guild), u64(user));
}

/** Who's typing in a channel, oldest first. */
export function typing(channel: Id): bigint[] {
  return [...raw.typing(u64(channel))];
}

export function voiceStates(channel: Id): VoiceState[] {
  return raw.voiceStates(u64(channel));
}

export function voiceState(user: Id): VoiceState | undefined {
  return raw.getVoiceState(u64(user));
}

/** Speaking right now (only in your voice channel). */
export function speaking(user: Id): boolean {
  return raw.speaking(u64(user));
}

export function relationships(): Relationship[] {
  return raw.relationships();
}

export function relationship(user: Id): RelationshipKind | undefined {
  return raw.getRelationship(u64(user));
}

/** Your private note on a user. */
export function note(user: Id): string | undefined {
  return raw.note(u64(user));
}

/** Guilds you share with a user (from loaded members). */
export function mutualGuilds(user: Id): bigint[] {
  return [...raw.mutualGuilds(u64(user))];
}
