// rekindle/decorations: small client-drawn badges and text on a user,
// channel, guild or message, pushed ahead of time so the client never calls
// into your plugin while it paints a list; plus message accessories, ghost
// messages and pinned DM sections (needs `ui`).
//
//   decorations.set(decorations.NAME_BADGE, userId, { icon: 'shield', tooltip: 'Moderator' });
//   decorations.setAccessory(message, new Elements().text('Translated: ...'));
//
// Kinds (key in brackets): NAME_BADGE, PROFILE_BADGE, AVATAR_OVERLAY,
// MEMBER_LIST_TRAILING, MEMBER_LIST_SUBTEXT, DM_LIST_TRAILING [user];
// CHANNEL_LIST_TRAILING, CHANNEL_LIST_SUBTITLE [channel]; GUILD_LIST_OVERLAY
// [guild]; MESSAGE_TRAILING [message]. Up to 5000 per plugin. Clicks on a
// `clickable` one arrive as `onSlot` `decoration-clicked` (or `onClick`).

import * as raw from 'rekindle:plugin/ui@0.9.0';
import type { Color, Icon, MessageRef, Span, UserRef } from 'rekindle:plugin/types@0.9.0';
import { __registry, call, span, u64, type Id } from 'rekindle';
import { toElements, type Element, type Elements } from 'rekindle/elements';

export const NAME_BADGE = 'user.name-badge';
export const PROFILE_BADGE = 'user.profile-badge';
export const AVATAR_OVERLAY = 'user.avatar-overlay';
export const MEMBER_LIST_TRAILING = 'member-list.trailing';
export const MEMBER_LIST_SUBTEXT = 'member-list.subtext';
export const DM_LIST_TRAILING = 'dm-list.trailing';
export const CHANNEL_LIST_TRAILING = 'channel-list.trailing';
export const CHANNEL_LIST_SUBTITLE = 'channel-list.subtitle';
export const GUILD_LIST_OVERLAY = 'guild-list.overlay';
export const MESSAGE_TRAILING = 'message.trailing';

/** A decoration: any of an icon, a registered image, text; with a tooltip, color, clickability. */
export interface Decoration {
  icon?: Icon;
  image?: string;
  text?: string;
  tooltip?: string;
  color?: Color;
  clickable?: boolean;
}

function toRaw(d: Decoration): raw.Decoration {
  return { icon: d.icon, image: d.image, text: d.text, tooltip: d.tooltip, color: d.color, clickable: d.clickable ?? false };
}

/** Decoration kinds this client build draws. */
export function available(): string[] {
  return raw.availableDecorations();
}

/** Set (or with `undefined`, remove) one decoration. */
export function set(kind: string, key: Id, d: Decoration | undefined): void {
  call('decorations.set', () => raw.setDecoration(kind, u64(key), d && toRaw(d)));
}

/** Replace all of your decorations of a kind at once. */
export function setAll(kind: string, items: Array<[Id, Decoration]>): void {
  call('decorations.setAll', () => raw.setDecorations(kind, items.map(([k, d]): [bigint, raw.Decoration] => [u64(k), toRaw(d)])));
}

export function clear(kind: string): void {
  raw.clearDecorations(kind);
}

/** Call `handler(key)` when a clickable decoration of `kind` is clicked (before `onSlot`). */
export function onClick(kind: string, handler: (key: bigint) => void): void {
  __registry.decoration.set(kind, (key) => handler(key));
}

/**
 * Elements under a message (a translation, an edit history); `undefined`
 * removes them. Button clicks arrive via `onUi` with surface `message:<id>`.
 */
export function setAccessory(message: MessageRef, elements: Elements | Element[] | undefined): void {
  call('decorations.setAccessory', () => raw.setMessageAccessory(message, elements && toElements(elements)));
}

/**
 * A row where a deleted (or edited) message was, styled as the client's own
 * system row and labelled with your plugin's name. Needs `read-messages`.
 * `style`: `'deleted'`, `'edited'` or `'note'`.
 */
export function setGhost(channel: Id, id: Id, author: UserRef, content: string | Span[], style = 'deleted'): void {
  const spans = typeof content === 'string' ? [span(content)] : content;
  call('decorations.setGhost', () => raw.setGhostMessage({ channel: u64(channel), id: u64(id), author, content: spans, style }));
}

export function removeGhost(channel: Id, id: Id): void {
  raw.removeGhostMessage(u64(channel), u64(id));
}

export interface DmSection {
  id: string;
  name: string;
  color?: Color;
  channels: Id[];
  collapsed?: boolean;
}

/** Replace your pinned DM sections at the top of the DM list. */
export function setDmSections(sections: DmSection[]): void {
  call('decorations.setDmSections', () =>
    raw.setDmSections(
      sections.map((s) => ({
        id: s.id,
        name: s.name,
        color: s.color,
        // jco types a list<u64> as a BigUint64Array, but the QuickJS
        // backend lowers only a plain array of bigints (a typed array traps).
        channels: s.channels.map(u64) as unknown as BigUint64Array,
        collapsed: s.collapsed ?? false,
      })),
    ),
  );
}
