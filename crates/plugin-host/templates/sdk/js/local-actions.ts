// rekindle/local-actions: your own local and self-only state (needs
// `act-local`): mark read, open a reply or edit, insert
// text, per-user volume, notes, saved messages, mute. Nothing here is visible
// to other users.
//
// Every function works only **while the client is delivering one of the
// user's own gestures** to your plugin (a slot click, a menu item, a palette
// or slash command, a keybind, a panel interaction, a message click binding)
// and within that gesture's budget. From timers, events or `activate` it
// throws a RekindleError with code 'no-gesture'. A plugin can make a button
// do a thing; it can't do the thing on its own.
//
// Reactions are `rekindle/reactions`; visible actions `rekindle/actions`.

import * as raw from 'rekindle:plugin/local-actions@0.11.0';
import type { MessageRef } from 'rekindle:plugin/types@0.11.0';
import { call, u64, type Id } from 'rekindle';

export type ActionError = import('rekindle:plugin/action-types@0.11.0').ActionError;

export function markRead(channel: Id): void {
  call('localActions.markRead', () => raw.markRead(u64(channel)));
}

export function markGuildRead(guild: Id): void {
  call('localActions.markGuildRead', () => raw.markGuildRead(u64(guild)));
}

/** Mark everything read (ReadAllNotificationsButton). */
export function markAllRead(): void {
  call('localActions.markAllRead', () => raw.markAllRead());
}

/** Mark unread from a message (the Alt+click behaviour). */
export function markUnread(message: MessageRef): void {
  call('localActions.markUnread', () => raw.markUnread(message));
}

/** Open a reply in the composer; the user still writes and sends it. */
export function startReply(message: MessageRef, mention = true): void {
  call('localActions.startReply', () => raw.startReply(message, mention));
}

/** Open your own message for editing. */
export function startEdit(message: MessageRef): void {
  call('localActions.startEdit', () => raw.startEdit(message));
}

/** Put text into the composer at the cursor, for the user to send. */
export function insertText(text: string): void {
  call('localActions.insertText', () => raw.insertText(text));
}

/** Per-user local volume (0-200%). */
export function setUserVolume(user: Id, percent: number): void {
  call('localActions.setUserVolume', () => raw.setUserVolume(u64(user), percent));
}

export function setUserLocalMute(user: Id, muted: boolean): void {
  call('localActions.setUserLocalMute', () => raw.setUserLocalMute(u64(user), muted));
}

/** Your private note on a user (`''` clears it). */
export function setNote(user: Id, note: string): void {
  call('localActions.setNote', () => raw.setNote(u64(user), note));
}

/** Save a message for later (bookmark), or remove it. */
export function saveMessage(message: MessageRef, saved = true): void {
  call('localActions.saveMessage', () => raw.saveMessage(message, saved));
}

export function setCategoryCollapsed(category: Id, collapsed: boolean): void {
  call('localActions.setCategoryCollapsed', () => raw.setCategoryCollapsed(u64(category), collapsed));
}

/** Mute a channel's or guild's notifications for you. */
export function setMuted(channelOrGuild: Id, muted: boolean): void {
  call('localActions.setMuted', () => raw.setMuted(u64(channelOrGuild), muted));
}

/** Open the DM with `user`, creating it if needed; returns the DM channel. */
export function openDm(user: Id): bigint {
  return call('localActions.openDm', () => raw.openDm(u64(user)));
}
