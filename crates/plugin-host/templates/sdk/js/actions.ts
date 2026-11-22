// rekindle/actions: visible actions others see (needs `act`): send a
// message with files you made, pin, delete or edit your message, suppress
// embeds, vote, join voice, self mute/deafen, threads. Gesture-bound like
// `rekindle/local-actions`, one per gesture, rate-limited.
//
//   actions.sendMessage(channel, '', [actions.file('pet.gif', 'image/gif', gif)]);

import * as raw from 'rekindle:plugin/actions@0.8.0';
import type { OutgoingFile } from 'rekindle:plugin/action-types@0.8.0';
import type { MessageRef } from 'rekindle:plugin/types@0.8.0';
import { call, optU64, u64, type Id } from 'rekindle';

export type { OutgoingFile };

/** A file to send with `sendMessage`. */
export function file(filename: string, contentType: string | undefined, data: Uint8Array, spoiler = false): OutgoingFile {
  return { filename, contentType, data, spoiler };
}

/** Send a message as the user to `channel` (one they can send in): text, files you made, or both. Up to 10 files, 25 MiB in all; the text may be empty when there are files. */
export function sendMessage(channel: Id, content: string, files: OutgoingFile[] = []): void {
  call('actions.sendMessage', () => raw.sendMessage(u64(channel), content, files));
}

export function setPinned(message: MessageRef, pinned: boolean): void {
  call('actions.setPinned', () => raw.setPinned(message, pinned));
}

/** Delete your own message (or another's with Manage Messages). The client confirms unless the gesture held Shift. */
export function deleteMessage(message: MessageRef): void {
  call('actions.deleteMessage', () => raw.deleteMessage(message));
}

/** Replace the text of your own message. */
export function editMessage(message: MessageRef, content: string): void {
  call('actions.editMessage', () => raw.editMessage(message, content));
}

export function suppressEmbeds(message: MessageRef): void {
  call('actions.suppressEmbeds', () => raw.suppressEmbeds(message));
}

export function vote(message: MessageRef, answers: number[]): void {
  call('actions.vote', () => raw.vote(message, Uint32Array.from(answers)));
}

/** Join a voice channel (`undefined` leaves). */
export function joinVoice(channel: Id | undefined): void {
  call('actions.joinVoice', () => raw.joinVoice(optU64(channel)));
}

export function setSelfMute(muted: boolean): void {
  call('actions.setSelfMute', () => raw.setSelfMute(muted));
}

export function setSelfDeaf(deafened: boolean): void {
  call('actions.setSelfDeaf', () => raw.setSelfDeaf(deafened));
}

export function setThreadJoined(thread: Id, joined: boolean): void {
  call('actions.setThreadJoined', () => raw.setThreadJoined(u64(thread), joined));
}
