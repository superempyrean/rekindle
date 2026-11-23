// rekindle/state: client structure and your own state (needs `read-client`).
//
// Queries answer from what the client already holds: they never cause a
// network request, and `undefined` means "not loaded or not visible to you".
// Records are modelled on the client's stores (GuildStore, ChannelStore,
// ...), never contain secrets, and carry ids as `bigint`s.
//
//   const sel = state.selection();
//   const ch = sel.channel !== undefined ? state.channel(sel.channel) : undefined;
//   if (ch) log.info(`you're in #${ch.name}`);

import * as raw from 'rekindle:plugin/client-state@0.10.0';
import * as inspect from 'rekindle:plugin/inspect@0.10.0';
import type { Member, Target } from 'rekindle:plugin/types@0.10.0';
import type {
  Channel,
  CustomStatus,
  Emoji,
  Guild,
  GuildFolder,
  GuildReadState,
  MyVoice,
  Overwrite,
  ReadState,
  Role,
  Selection,
  Sticker,
  User,
} from 'rekindle:plugin/records@0.10.0';
import { u64, type Id } from 'rekindle';

export type {
  Channel,
  CustomStatus,
  Emoji,
  ForumTag,
  Guild,
  GuildFolder,
  GuildReadState,
  MyVoice,
  Overwrite,
  ReadState,
  Role,
  Selection,
  Sticker,
  ThreadMeta,
  User,
} from 'rekindle:plugin/records@0.10.0';
export type { Member };

/** Where the user is: guild, channel, voice channel, side thread, focus, idle. */
export function selection(): Selection {
  return raw.getSelection();
}

export function me(): User {
  return raw.me();
}

/** `'online'`, `'idle'`, `'dnd'` or `'invisible'`. */
export function myStatus(): string {
  return raw.myStatus();
}

export function myCustomStatus(): CustomStatus | undefined {
  return raw.myCustomStatus();
}

/** A user the client knows. */
export function user(id: Id): User | undefined {
  return raw.getUser(u64(id));
}

/** The name the client shows for a user (global name, else username). */
export function displayName(id: Id): string | undefined {
  const u = user(id);
  return u && (u.globalName ?? u.username);
}

export function guild(id: Id): Guild | undefined {
  return raw.getGuild(u64(id));
}

/** Guild ids in sidebar order. */
export function guildIds(): bigint[] {
  return [...raw.guildIds()];
}

export function guildFolders(): GuildFolder[] {
  return raw.guildFolders();
}

export function channel(id: Id): Channel | undefined {
  return raw.getChannel(u64(id));
}

/** A guild's channels in sidebar order; `includeHidden` lists the ones you can't view too (with `visible: false`). */
export function channels(guild: Id, includeHidden = false): Channel[] {
  return raw.channels(u64(guild), includeHidden);
}

/** Active threads the client knows under a channel. */
export function threads(parent: Id): Channel[] {
  return raw.threads(u64(parent));
}

/** DMs and group DMs, most recent first. */
export function privateChannels(): Channel[] {
  return raw.privateChannels();
}

export function dmChannel(user: Id): bigint | undefined {
  return raw.dmChannel(u64(user));
}

export function role(guild: Id, id: Id): Role | undefined {
  return raw.getRole(u64(guild), u64(id));
}

/** Highest position first. */
export function roles(guild: Id): Role[] {
  return raw.roles(u64(guild));
}

export function guildEmoji(guild: Id): Emoji[] {
  return raw.guildEmoji(u64(guild));
}

/** Most-used first; Unicode emoji have id 0 and the emoji in `name`. */
export function frequentEmoji(limit: number): Emoji[] {
  return raw.frequentEmoji(limit);
}

export function guildStickers(guild: Id): Sticker[] {
  return raw.guildStickers(u64(guild));
}

export function myMember(guild: Id): Member | undefined {
  return raw.myMember(u64(guild));
}

/** Your permission bits in a channel (or guild); see `perms` in `rekindle/consts`. */
export function myPermissions(channelOrGuild: Id): bigint {
  return raw.myPermissions(u64(channelOrGuild));
}

/** You have all of `bits` in a channel (or guild). */
export function can(bits: bigint, channelOrGuild: Id): boolean {
  return raw.can(bits, u64(channelOrGuild));
}

export function permissionOverwrites(channel: Id): Overwrite[] {
  return raw.permissionOverwrites(u64(channel));
}

export function readState(channel: Id): ReadState | undefined {
  return raw.getReadState(u64(channel));
}

export function guildReadState(guild: Id): GuildReadState {
  return raw.getGuildReadState(u64(guild));
}

/** Channels with unread messages, most recent first. */
export function unreadChannels(limit = 200): ReadState[] {
  return raw.unreadChannels(limit);
}

/** Your voice connection, if any (channel, since, self mute/deaf, ping). */
export function myVoice(): MyVoice | undefined {
  return raw.getMyVoice();
}

/** One of your client settings by key (`'appearance.message-display-compact'`, ...): a curated, read-only allowlist. */
export function mySetting(key: string): string | undefined {
  return raw.mySetting(key);
}

export function mySettingKeys(): string[] {
  return raw.mySettingKeys();
}

/**
 * A developer view of what the client holds about a target, as JSON with
 * secrets stripped. **Unstable by design**: the shape may change in any
 * build. Show it; don't parse it. (Messages need `read-messages`, users
 * `read-users`.)
 */
export function rawJson(target: Target): string | undefined {
  return inspect.rawJson(target);
}
