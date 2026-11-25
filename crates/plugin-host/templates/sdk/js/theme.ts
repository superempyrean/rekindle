// rekindle/theme: read and override theme tokens (needs `theming`).
//
// Token names follow Discord's design tokens: `background-base-low`,
// `text-default`, `radius-md`, `font-primary`, `density`, ... (older names
// like `accent` are aliases). `tokens()` lists what the client has. Overrides
// are yours alone: unloading the plugin reveals what was underneath.
//
//   theme.set('background-base-low', hex('#0b0d12'));
//   theme.setRadius('radius-md', 0);
//   theme.tint(hex('#eb459e'), 0.12);
//   theme.hideSurface('chat.gift-button');

import * as raw from 'rekindle:plugin/theming@0.11.0';
import type { Color } from 'rekindle:plugin/types@0.11.0';
import { call } from 'rekindle';

export type TokenInfo = raw.TokenInfo;
export type ThemeValue = raw.ThemeValue;
export type SurfaceStyle = raw.SurfaceStyle;

/** Every token the client exposes (canonical names with kind, group and aliases). */
export function tokens(): TokenInfo[] {
  return raw.tokens();
}

/** Names of the tokens in a group (`'background'`, `'text'`, `'control'`, ...). */
export function group(name: string): string[] {
  return raw.tokens().filter((t) => t.group === name).map((t) => t.name);
}

/** A color token's effective color. */
export function get(token: string): Color | undefined {
  return raw.getColor(token) ?? undefined;
}

/** Any token's effective value. */
export function value(token: string): ThemeValue | undefined {
  return raw.get(token) ?? undefined;
}

/** Override a color token. `false` if the client has no such color token. */
export function set(token: string, color: Color): boolean {
  return raw.setColor(token, color);
}

/** Override many color tokens; returns the ones the client didn't accept. */
export function setMany(colors: Record<string, Color>): string[] {
  return Object.entries(colors)
    .filter(([token, color]) => !raw.setColor(token, color))
    .map(([token]) => token);
}

/** Override any token with a value of its kind (throws 'unknown-token', 'wrong-kind', 'out-of-range'). */
export function setValue(token: string, v: ThemeValue): void {
  call('theme.setValue', () => raw.set(token, v));
}

/** A length token (`radius-*`, `chat-avatar-size`), in px. */
export function setLength(token: string, px: number): void {
  setValue(token, { tag: 'length', val: px });
}

/** `setLength`, under its first name (most length tokens are radii). */
export const setRadius = setLength;

/** A font token (`font-primary`, ...): a family the client has, or one you `registerFont`ed. */
export function setFont(token: string, family: string): void {
  setValue(token, { tag: 'font', val: family });
}

export function setDensity(density: 'compact' | 'cozy' | 'default'): void {
  setValue('density', { tag: 'choice', val: density });
}

/** Drop your override of one token. */
export function reset(token: string): void {
  raw.reset(token);
}

/** Tint every surface toward `color` (0..1); text by `textAmount`. */
export function tint(color: Color, amount: number, textAmount = 0): void {
  raw.setTint({ color, amount, textAmount });
}

export function clearTint(): void {
  raw.setTint(undefined);
}

/** Make a font file (TTF/OTF bytes, e.g. a bundled asset) usable as `family`. */
export function registerFont(family: string, data: Uint8Array): void {
  call('theme.registerFont', () => raw.registerFont(family, data));
}

/** Named layout surfaces this build can restyle (`'guild-list'`, `'chat.gift-button'`, `'title-bar.help'`, ...). */
export function surfaces(): string[] {
  return raw.surfaces();
}

/** Hide a named surface (decluttering: the gift button, the help icon). */
export function hideSurface(surface: string): void {
  setSurface(surface, { hidden: true });
}

/** Restyle a named surface within bounds (it can't be moved or redrawn). Throws 'unknown-surface'. */
export function setSurface(surface: string, style: { hidden?: boolean; opacity?: number; radius?: number; background?: Color }): void {
  const s: SurfaceStyle = { hidden: style.hidden ?? false, opacity: style.opacity, radius: style.radius, background: style.background };
  call('theme.setSurface', () => raw.setSurface(surface, s));
}

export function resetSurface(surface: string): void {
  raw.resetSurface(surface);
}

/** The keys of the client strings this build lets plugins replace. */
export function strings(): string[] {
  return raw.strings();
}

/** Show `text` in place of one of the client's strings; `undefined` restores it. */
export function setString(key: string, text: string | undefined): void {
  call('theme.setString', () => raw.setString(key, text));
}

/** One of your registered images (`ui.registerImage`) behind the chat area. */
export function setBackground(image: string, opacity = 1, blur = 0): void {
  raw.setBackground({ image, opacity, blur });
}

export function clearBackground(): void {
  raw.setBackground(undefined);
}
