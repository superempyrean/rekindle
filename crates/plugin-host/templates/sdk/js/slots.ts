// rekindle/slots: put things in the client's own UI (needs `ui`).
//
// The client has a fixed set of named places; you fill them with small, typed
// items it draws natively: menu items in menus, icon buttons in bars, list
// items in lists.
//
//   slots.messageMenuItem(new MenuItem('bookmark', 'Bookmark').icon('bookmark'), (m) => save(m.id));
//   slots.button(slots.CHAT_BAR, 'toggle', 'eye', 'Toggle', { onClick: () => flip() });
//
// Or handle clicks in `onSlot`: `slots.clicked(ev)` is the click (`item`,
// `target`, `modifiers`). Message click bindings (`bindClick`) are gestures
// on messages themselves: double-click to edit, and so on.
//
// A slot can be missing on a client build; adding to it then throws a
// RekindleError with code 'unavailable'. Check `isAvailable` to degrade.

import * as raw from 'rekindle:plugin/ui@0.9.0';
import type { ChannelRef, Icon, MessageRef, Modifiers, Target, UserRef } from 'rekindle:plugin/types@0.9.0';
import type { SlotClick, SlotEvent } from 'rekindle:plugin/ui-handler@0.9.0';
import { __registry, call, optU64, u64, type Id } from 'rekindle';
import { toElements, type Element, type Elements } from 'rekindle/elements';

export { closeModal, toast } from 'rekindle/ui';
export type { Icon, SlotClick, SlotEvent };
export type MenuPredicate = raw.MenuPredicate;
export type ClickArea = raw.ClickArea;

// Menus (item: MenuItem; target in brackets).
/** [message] Right-click a message. */
export const MESSAGE_MENU = 'message.context-menu';
/** [user] User, member and DM user menus. */
export const USER_MENU = 'user.context-menu';
/** [channel] Channel and DM rows. */
export const CHANNEL_MENU = 'channel.context-menu';
export const THREAD_MENU = 'thread.context-menu';
/** [guild] The server icon menu and header popout. */
export const GUILD_MENU = 'guild.context-menu';
export const ROLE_MENU = 'role.context-menu';
/** [media] Images, videos, avatars. */
export const MEDIA_MENU = 'media.context-menu';
/** [media] The full-size image and video viewer's "More" menu. */
export const MEDIA_VIEWER_MENU = 'media-viewer.menu';
export const LINK_MENU = 'link.context-menu';
/** [selection] The composer/text menu. */
export const SELECTION_MENU = 'selection.context-menu';
/** [channel] The composer's "+" upload menu. */
export const ATTACH_MENU = 'composer.attach-menu';
/** [none] Right-click the settings cog. */
export const SETTINGS_COG_MENU = 'settings.cog-menu';
// Bars and buttons (item: icon button).
/** [message] The message hover action bar. */
export const MESSAGE_HOVER_BAR = 'message.hover-bar';
/** [channel] Next to the composer's emoji/GIF buttons. */
export const CHAT_BAR = 'chat-bar.button';
/** [channel] The channel header. */
export const CHANNEL_TOOLBAR = 'channel.toolbar';
/** [none] The bottom-left mute/deafen/settings row. */
export const USER_AREA = 'user-area.button';
/** [channel] The voice connection panel. */
export const VOICE_PANEL = 'voice-panel.button';
/** [none] The top of the server list. */
export const GUILD_LIST = 'guild-list.button';
/** [none] Above the DM list. */
export const DM_LIST = 'dm-list.button';
// Lists (item: list item).
export const VOICE_PANEL_STATUS = 'voice-panel.status';
export const PROFILE_SECTION = 'profile.section';

/** Slot names that work on this client build. */
export function available(): string[] {
  return raw.availableSlots();
}

export function isAvailable(slot: string): boolean {
  return available().includes(slot);
}

/** A menu item, built up: `new MenuItem('id', 'Label').icon('copy').group('copy')`. */
export class MenuItem {
  private readonly item: raw.MenuItem;

  constructor(id: string, label: string) {
    this.item = { id, label, icon: undefined, danger: false, group: undefined, checked: undefined, when: [] };
  }
  get id(): string {
    return this.item.id;
  }
  icon(icon: Icon): this {
    this.item.icon = icon;
    return this;
  }
  /** Destructive style. */
  danger(): this {
    this.item.danger = true;
    return this;
  }
  /** Join one of the menu's groups (`'copy'`, `'reply'`, `'moderation'`, `'developer'`, ...) instead of the plugin group at the end. */
  group(group: string): this {
    this.item.group = group;
    return this;
  }
  /** A check mark (a toggle item). */
  checked(on: boolean): this {
    this.item.checked = on;
    return this;
  }
  /** Only show when every predicate holds (the client evaluates them, so the menu opens without calling you). */
  when(...predicates: MenuPredicate[]): this {
    this.item.when.push(...predicates);
    return this;
  }
  toRaw(): raw.MenuItem {
    return { ...this.item, when: [...this.item.when] };
  }
}

/** Where an item's click handler is kept: the same id in two slots has two. */
export const slotKey = (slot: string, id: string): string => `${slot}\n${id}`;

/**
 * Add (or replace, by id) a menu item in a menu slot. `onClick`, if given,
 * runs on a click with what was clicked, before `onSlot`.
 */
export function add(slot: string, item: MenuItem, onClick?: (target: Target, click: SlotClick) => void): void {
  call('slots.add', () => raw.addItem(slot, { tag: 'menu-item', val: item.toRaw() }));
  if (onClick) __registry.slot.set(slotKey(slot, item.id), onClick);
}

/** A message menu item; `onClick` gets the message's ids (not its content). */
export function messageMenuItem(item: MenuItem, onClick?: (message: MessageRef, click: SlotClick) => void): void {
  add(MESSAGE_MENU, item, onClick && ((t, c) => (t.tag === 'message' ? onClick(t.val, c) : undefined)));
}

/** A user menu item; `onClick` gets the user and the guild the menu was opened in, if any. */
export function userMenuItem(item: MenuItem, onClick?: (user: UserRef, guild: bigint | undefined, click: SlotClick) => void): void {
  add(USER_MENU, item, onClick && ((t, c) => (t.tag === 'user' ? onClick(t.val[0], t.val[1], c) : undefined)));
}

/** A channel menu item; `onClick` gets the channel. */
export function channelMenuItem(item: MenuItem, onClick?: (channel: ChannelRef, click: SlotClick) => void): void {
  add(CHANNEL_MENU, item, onClick && ((t, c) => (t.tag === 'channel' ? onClick(t.val, c) : undefined)));
}

export interface ButtonOptions {
  /** Highlighted (a toggle that's on). */
  active?: boolean;
  /** One of your registered images instead of the icon. */
  image?: string;
  /** Runs on a click, before `onSlot`. */
  onClick?: (target: Target, click: SlotClick) => void;
}

/** An icon button in a bar slot (`CHAT_BAR`, `MESSAGE_HOVER_BAR`, `GUILD_LIST`, ...). Re-adding an id replaces it. */
export function button(slot: string, id: string, icon: Icon, tooltip: string, opts: ButtonOptions = {}): void {
  call('slots.button', () =>
    raw.addItem(slot, { tag: 'icon-button', val: { id, icon, image: opts.image, tooltip, active: opts.active ?? false } }),
  );
  if (opts.onClick) __registry.slot.set(slotKey(slot, id), opts.onClick);
}

/** Flip a button's `active` state without re-adding it. */
export function setActive(slot: string, id: string, active: boolean): void {
  call('slots.setActive', () => raw.setActive(slot, id, active));
}

/**
 * A section in user profiles. `onOpen` runs when someone's profile opens
 * (before `onSlot`): answer with `setProfileSection(id, user.id, elements)`.
 */
export function profileSection(id: string, title: string, onOpen?: (user: UserRef) => void): void {
  call('slots.profileSection', () => raw.addItem(PROFILE_SECTION, { tag: 'list-item', val: { id, title } }));
  if (onOpen) __registry.profile.set(id, (user) => onOpen(user));
}

/** Your profile section's content for one user. */
export function setProfileSection(sectionId: string, user: Id, elements: Elements | Element[]): void {
  raw.setProfileSection(sectionId, u64(user), toElements(elements));
}

/**
 * One line in the voice panel ("Call 1:02:33"). With `sinceMs`, the client
 * renders a live elapsed time after `text` (no timer needed). An empty `text`
 * removes it.
 */
export function setVoiceStatus(id: string, text: string, sinceMs?: Id): void {
  call('slots.setVoiceStatus', () => raw.setVoiceStatus(id, text, optU64(sinceMs)));
}

/** Remove an item you added. */
export function remove(slot: string, id: string): void {
  raw.removeItem(slot, id);
  __registry.slot.delete(slotKey(slot, id));
}

/** No modifier keys. */
export const NO_MODS: Modifiers = { shift: false, ctrl: false, alt: false, meta: false };

/** Modifier keys, by name: `mods('shift')`, `mods('ctrl', 'alt')`. */
export function mods(...keys: Array<keyof Modifiers>): Modifiers {
  const m = { ...NO_MODS };
  for (const k of keys) m[k] = true;
  return m;
}

export interface ClickBinding {
  /** Shown in rekindle's Message clicks settings ("Edit on double-click"). */
  label: string;
  area?: ClickArea;
  /** 1 or 2 (single clicks need a modifier). */
  clicks?: number;
  modifiers?: Modifiers;
  /** Only on messages where every predicate holds (own message, in a guild, ...). */
  when?: MenuPredicate[];
}

/**
 * A message click binding: `clicks` (1 or 2) with `modifiers` on an `area`
 * of a message (default: a plain double-click on the content). Each fired
 * binding is a user gesture: it calls `onClick`, if given, then `onSlot` with
 * `message-clicked`. If two plugins claim the same click, the user picks the
 * owner in rekindle's settings. Throws 'unavailable' if the client itself
 * uses that click.
 */
export function bindClick(id: string, binding: ClickBinding, onClick?: (message: MessageRef) => void): void {
  call('slots.bindClick', () =>
    raw.registerClick({
      id,
      label: binding.label,
      area: binding.area ?? 'content',
      clicks: binding.clicks ?? 2,
      modifiers: binding.modifiers ?? NO_MODS,
      when: binding.when ?? [],
    }),
  );
  if (onClick) __registry.click.set(id, (m) => onClick(m));
}

export function unbindClick(id: string): void {
  raw.unregisterClick(id);
  __registry.click.delete(id);
}

/** If `ev` is a click on one of your items: the click (`slot`, `item`, `target`, `modifiers`). */
export function clicked(ev: SlotEvent): SlotClick | undefined {
  return ev.tag === 'clicked' ? ev.val : undefined;
}

/** If `ev` is a fired click binding: its id and the message. */
export function messageClicked(ev: SlotEvent): { binding: string; message: MessageRef } | undefined {
  return ev.tag === 'message-clicked' ? { binding: ev.val[0], message: ev.val[1] } : undefined;
}

/** If `ev` is a profile with your section opening: the section id and whose profile. */
export function profileOpened(ev: SlotEvent): { section: string; user: UserRef } | undefined {
  return ev.tag === 'profile-opened' ? { section: ev.val[0], user: ev.val[1] } : undefined;
}
