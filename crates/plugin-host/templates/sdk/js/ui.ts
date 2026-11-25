// rekindle/ui: panels, modals, the settings page's extras, palette commands,
// keybinds, toasts, notices, images and navigation (needs `ui`).
//
//   new Panel('main', 'My Plugin')
//     .text('Shown at the top.')
//     .toggle('enabled', 'Enabled', true)
//     .textInput('name', 'Name', '', 'your name')
//     .button('save', 'Save')
//     .on('save', () => save())
//     .show();
//
// React to an element right where you add it, with `.on(element, value =>
// ...)`, or in an `onUi` handler (`{ surface, element, value }`; `uiValue`
// reads the value). Panels are stateless: the client doesn't remember what
// was typed, so show the panel again with current values after a change.
// Text inputs report a change when it's committed, not per keystroke.
//
// Slots (menus, buttons) are `rekindle/slots`; badges and accessories
// `rekindle/decorations`; slash commands `rekindle/commands`.

import * as raw from 'rekindle:plugin/ui@0.11.0';
import type { Target, UiEvent } from 'rekindle:plugin/types@0.11.0';
import { __registry, call, u64, type Id } from 'rekindle';
import { Elements, on, toElements, type Element } from 'rekindle/elements';

export { element, Elements, on, uiValue } from 'rekindle/elements';
export { span, token, rgba } from 'rekindle';
export type { Element };
export type ToastKind = raw.ToastKind;
export type UiError = raw.UiError;

/** The `onUi` surface of your settings page's extra elements. */
export const SETTINGS_PAGE = 'settings-page';
/** The `onUi` surface of your notice's button (element `button`). */
export const NOTICE = 'notice';

/** A panel (or, with `showModal`, a dialog). Builder methods chain. */
export class Panel extends Elements {
  /** `id` is unique within your plugin; showing an id again replaces that panel. */
  constructor(
    readonly id: string,
    readonly title: string,
  ) {
    super();
  }

  /**
   * Call `handler` when the user changes `element` of this panel, with the
   * new value (a toggle gives a boolean, a text input a string, ...; a button
   * gives `undefined`). Replaces any earlier callback for that element.
   */
  on(element: string, handler: (value: any, event: UiEvent) => void): this {
    on(this.id, element, handler);
    return this;
  }

  /** The contract's panel record. */
  toRaw(): raw.Panel {
    return { id: this.id, title: this.title, elements: this.elements };
  }
  /** Show (or replace) the panel. */
  show(): this {
    raw.showPanel(this.toRaw());
    return this;
  }
  /** Show it as a modal dialog over the client (one per plugin; the user can always dismiss it). */
  showModal(): this {
    raw.showModal(this.toRaw());
    return this;
  }
  remove(): void {
    raw.removePanel(this.id);
  }
}

export function removePanel(id: string): void {
  raw.removePanel(id);
}

export function closeModal(): void {
  raw.closeModal();
}

/**
 * Extra elements under your generated settings page, for what the schema
 * can't express (a preview, a "test" button). Interactions arrive via `onUi`
 * with surface `settings-page` (or `on(SETTINGS_PAGE, element, ...)`).
 */
export function setSettingsExtra(elements: Elements | Element[]): void {
  raw.setSettingsExtra(toElements(elements));
}

/**
 * Add an entry to the command palette. Choosing it calls `run`, if given,
 * and then `onCommand(id)`.
 */
export function registerCommand(id: string, title: string, run?: () => void): void {
  raw.registerCommand(id, title);
  if (run) __registry.command.set(id, run);
}

export function unregisterCommand(id: string): void {
  raw.unregisterCommand(id);
  __registry.command.delete(id);
}

/**
 * A client keybind (Settings > Keybinds), user-rebindable, active only while
 * the client window has focus. `defaultKeys` like `'Ctrl+Shift+K'`. A press
 * calls `run`, if given, then `onKeybind(id)`.
 */
export function registerKeybind(id: string, label: string, defaultKeys?: string, run?: () => void): void {
  call('ui.registerKeybind', () => raw.registerKeybind(id, label, defaultKeys));
  if (run) __registry.keybind.set(id, run);
}

export function unregisterKeybind(id: string): void {
  raw.unregisterKeybind(id);
  __registry.keybind.delete(id);
}

/**
 * Show another avatar and/or banner for `user` on this client (registered
 * image ids). Nothing is sent: everyone else still sees theirs. Pass
 * `undefined` for both to show theirs again.
 */
export function setProfileArt(user: Id, avatar?: string, banner?: string): void {
  const art = avatar === undefined && banner === undefined ? undefined : { avatar, banner };
  call('ui.setProfileArt', () => raw.setProfileArt(u64(user), art));
}

/** Keep the client window above other windows (until the plugin unloads). */
export function setWindowOnTop(onTop: boolean): void {
  call('ui.setWindowOnTop', () => raw.setWindowOnTop(onTop));
}

/** The client window's opacity, 0.3 to 1 (until the plugin unloads). */
export function setWindowOpacity(opacity: number): void {
  call('ui.setWindowOpacity', () => raw.setWindowOpacity(opacity));
}

/** A transient notice (rate-limited by the client: a few per 10 s). */
export function toast(text: string, kind: ToastKind = 'info'): void {
  raw.showToast(text, kind);
}

/**
 * A banner across the top of the client (one per plugin) with an optional
 * button; a click calls `onClick`, if given, then `onUi` (surface `notice`).
 */
export function notice(text: string, kind: ToastKind = 'info', button?: string, onClick?: () => void): void {
  raw.showNotice({ text, kind, button });
  if (onClick) on(NOTICE, 'button', () => onClick());
}

export function dismissNotice(): void {
  raw.dismissNotice();
}

/** Register a PNG (at most 256 KiB, 256x256) for icons, badges, images and rich-presence assets. */
export function registerImage(id: string, png: Uint8Array): void {
  call('ui.registerImage', () => raw.registerImage(id, png));
}

/** Go to a channel, message, guild or profile, like clicking it. Only during a user gesture. */
export function navigate(to: Target): void {
  call('ui.navigate', () => raw.navigate(to));
}

/** Open an https URL in the browser (through the client's "leaving Discord" confirm). Only during a user gesture. */
export function openUrl(url: string): void {
  call('ui.openUrl', () => raw.openUrl(url));
}

/** Open the client's media viewer on an image it has. Only during a user gesture. */
export function openMedia(url: string): void {
  call('ui.openMedia', () => raw.openMedia(url));
}

