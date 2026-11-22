// rekindle/reactions: add and remove your own reactions (needs `react`,
// `act` implies it). Gesture-bound like
// `rekindle/local-actions`, up to 3 per gesture: only on the user's click.
//
// `emoji` is Unicode (`'👍'`) or `name:id` for a custom one.

import * as raw from 'rekindle:plugin/reactions@0.8.0';
import type { MessageRef } from 'rekindle:plugin/types@0.8.0';
import { call } from 'rekindle';

export function react(message: MessageRef, emoji: string): void {
  call('reactions.react', () => raw.react(message, emoji, false));
}

/** A super reaction. */
export function reactBurst(message: MessageRef, emoji: string): void {
  call('reactions.reactBurst', () => raw.react(message, emoji, true));
}

export function unreact(message: MessageRef, emoji: string, burst = false): void {
  call('reactions.unreact', () => raw.unreact(message, emoji, burst));
}
