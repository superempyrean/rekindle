// rekindle/notify: desktop notifications (needs `notifications`). `post`
// shows one labelled with your plugin's name,
// rate-limited and subject to the client's quiet hours and Do Not Disturb.
//
// An `onNotification(n)` handler filters the client's own notifications. It
// runs in the render isolate (like `render`): it sees the notification's text,
// and its only output is the decision built here.

import * as raw from 'rekindle:plugin/notify@0.9.0';
import type { Target } from 'rekindle:plugin/types@0.9.0';
import type { ClientNotification, NotificationDecision, NotificationKind } from 'rekindle:plugin/notification-types@0.9.0';
import { call } from 'rekindle';

export type { ClientNotification, NotificationDecision, NotificationKind };
export type DesktopNotification = raw.DesktopNotification;

/** Post a desktop notification; clicking it goes to `onClick`, if given. */
export function post(title: string, body: string, onClick?: Target): void {
  postFull({ title, body, icon: undefined, onClick, sound: undefined });
}

/** Post with every option (an icon from your registered images, a sound). */
export function postFull(n: DesktopNotification): void {
  call('notify.post', () => raw.post(n));
}

/** Decisions for `onNotification`. */
export const decision = {
  show: (): NotificationDecision => ({ tag: 'show' }),
  /** Don't show it (and don't play its sound). */
  suppress: (): NotificationDecision => ({ tag: 'suppress' }),
  /** Show it without the sound. */
  silent: (): NotificationDecision => ({ tag: 'silent' }),
  /** Play the sound only. */
  soundOnly: (): NotificationDecision => ({ tag: 'sound-only' }),
  /** Show it with another client sound, or one of yours. */
  sound: (name: string): NotificationDecision => ({ tag: 'sound', val: name }),
};
