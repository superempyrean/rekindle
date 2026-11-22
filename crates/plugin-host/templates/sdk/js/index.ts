// rekindle: the plugin SDK for TypeScript and JavaScript (core).
//
// `rekindle build` provides this module; there's nothing to install. Each
// area that needs a capability is its own module, named after the contract
// interface it uses, so importing one is what needs it:
//
//   rekindle/state (read-client)        rekindle/users (read-users)
//   rekindle/messages (read-messages)   rekindle/render (render-messages)
//   rekindle/ui, rekindle/slots, rekindle/decorations, rekindle/commands (ui)
//   rekindle/local-actions (act-local)  rekindle/reactions (react)
//   rekindle/actions (act)              rekindle/presence (presence)
//   rekindle/theme (theming)            rekindle/storage (storage)
//   rekindle/http (network)             rekindle/clipboard (clipboard)
//   rekindle/notify (notifications)     rekindle/audio, rekindle/files,
//   rekindle/tweaks, rekindle/overlay (the capability of the same name)
//   rekindle/compose-control (compose)
//
// This module and `rekindle/settings`, `rekindle/events`, `rekindle/timer`,
// `rekindle/time`, `rekindle/elements`, `rekindle/compose` and
// `rekindle/consts` need nothing.
//
//   import { definePlugin, log } from 'rekindle';
//   import { Panel } from 'rekindle/ui';
//
//   export default definePlugin({
//     activate() {
//       new Panel('main', 'My Plugin').toggle('on', 'Enabled', true).show();
//     },
//     onUi(ev) {
//       if (ev.element === 'on' && ev.value.tag === 'toggled') log.info(`now ${ev.value.val}`);
//     },
//   });

import * as host from 'rekindle:plugin/host@0.8.0';
import type { ClientInfo } from 'rekindle:plugin/host@0.8.0';
import type {
  Capability,
  ChannelRef,
  Color,
  Element,
  GuildRef,
  Icon,
  MediaRef,
  Member,
  MessageRef,
  Modifiers,
  RoleRef,
  Span,
  SpanColor,
  Target,
  UiEvent,
  UiValue,
  UserRef,
} from 'rekindle:plugin/types@0.8.0';
import type { Event, EventKind } from 'rekindle:plugin/events@0.8.0';
import type { Message, User } from 'rekindle:plugin/records@0.8.0';
import type { SlotClick, SlotEvent } from 'rekindle:plugin/ui-handler@0.8.0';
import type { CommandReply, Invocation } from 'rekindle:plugin/command-types@0.8.0';
import type { ComposeResult, Draft, Upload, UploadResult } from 'rekindle:plugin/compose-types@0.8.0';
import type { RenderInput, Transform } from 'rekindle:plugin/render-types@0.8.0';
import type { ClientNotification, NotificationDecision } from 'rekindle:plugin/notification-types@0.8.0';
import type { Verdict } from 'rekindle:plugin/event-filter@0.8.0';

export type {
  Capability,
  ChannelRef,
  ClientInfo,
  ClientNotification,
  Color,
  CommandReply,
  ComposeResult,
  Draft,
  Element,
  Event,
  EventKind,
  GuildRef,
  Icon,
  Invocation,
  MediaRef,
  Member,
  Message,
  MessageRef,
  Modifiers,
  NotificationDecision,
  RenderInput,
  RoleRef,
  SlotClick,
  SlotEvent,
  Span,
  SpanColor,
  Target,
  Transform,
  UiEvent,
  UiValue,
  Upload,
  UploadResult,
  User,
  UserRef,
};

/** A snowflake id. The client hands you `bigint`s; SDK functions also take a number. */
export type Id = bigint | number;

/**
 * The handlers a plugin can export. Every one is optional.
 *
 * The client calls a hook only if the manifest's capabilities cover it:
 * `onCompose`/`onUpload` need `compose`, `render` needs `render-messages`,
 * `onNotification` needs `notifications`, `onIntercept` needs `intercept`.
 * The build refuses such a hook without its capability, since it would never
 * run.
 */
export interface Plugin {
  /** Runs once, after load. `granted` lists the capabilities in the manifest. Throw to fail activation. */
  activate?(granted: Capability[]): void;
  /** The plugin is being unloaded: save state now. Everything it added to the client is removed afterwards. */
  deactivate?(): void;
  /** A setting from the manifest's schema changed (on the settings page, or by `settings.set`). */
  onSettingsChanged?(key: string): void;
  /** The client's features changed (a client update or re-anchor): slots may have come or gone; re-add items if needed. */
  onFeaturesChanged?(): void;
  /** Events, one client tick at a time. Without it, each event goes to `onEvent`. */
  onEvents?(events: Event[]): void;
  /** One client event. The contract adds kinds over time: ignore ones you don't know. */
  onEvent?(event: Event): void;
  /** The user changed an element on one of your surfaces: a panel or modal (by id), `settings-page`, `notice`, or `message:<id>` (an accessory). */
  onUi?(event: UiEvent): void;
  /** The user ran one of your palette commands. */
  onCommand?(id: string): void;
  /** A click on one of your slot items, a decoration click, a message click binding, or a profile with your section opening. */
  onSlot?(event: SlotEvent): void;
  /** The user pressed one of your keybinds. */
  onKeybind?(id: string): void;
  /** The user ran one of your slash commands. Return a reply (`rekindle/commands`), or nothing. */
  onSlashCommand?(inv: Invocation): CommandReply | undefined | void;
  /** Suggestions for an `autocomplete` option, as `[label, value]` pairs. Answer fast. */
  onAutocomplete?(command: string, option: string, partial: string): Array<[string, string]> | undefined | void;
  /** A timer you set with `rekindle/timer` fired. */
  onTimer?(id: string): void;
  /**
   * The user is sending (or saving an edit of) a message; needs `compose`.
   * Return a `ComposeResult` (`rekindle/compose`), a string to send instead,
   * or nothing to leave it.
   */
  onCompose?(draft: Draft): ComposeResult | string | undefined | null | void;
  /** A file is about to be uploaded (needs `compose` and `compose-uploads: true`). */
  onUpload?(upload: Upload): UploadResult | undefined | null | void;
  /**
   * How a message should look; needs `render-messages`. Runs in the **render
   * isolate**: a separate instance of your plugin that never activates (so
   * nothing `activate` set up is there), may read settings and storage, and
   * can't write, schedule, call out or keep anything. Return transforms
   * (`rekindle/render`), or nothing.
   */
  render?(input: RenderInput): Transform[] | undefined | null | void;
  /** A notification the client is about to show; needs `notifications`. Runs in the render isolate, like `render`. */
  onNotification?(n: ClientNotification): NotificationDecision | undefined | null | void;
  /**
   * An event from Discord before the client processes it; needs `intercept`,
   * and only kinds registered with `events.intercept` arrive. Return
   * `{ tag: 'drop' }`, `{ tag: 'replace', val: event }` (same kind), or
   * nothing to let it pass. Runs on the client's hot path: decide quickly.
   */
  onIntercept?(event: Event): Verdict | undefined | null | void;
}

/**
 * Type a plugin's handlers: `export default definePlugin({ ... })`. Gives each
 * handler's parameters their types; returns its argument unchanged. (Named
 * exports work too.)
 */
export function definePlugin<P extends Plugin>(plugin: P & Record<Exclude<keyof P, keyof Plugin>, never>): P {
  return plugin;
}

/** Log to the client's plugin log (`console.*` goes there too). */
export const log = {
  trace: (message: string) => host.log('trace', message),
  debug: (message: string) => host.log('debug', message),
  info: (message: string) => host.log('info', message),
  warn: (message: string) => host.log('warn', message),
  error: (message: string) => host.log('error', message),
};

/** Monotonic milliseconds since the client started. There's no wall clock. */
export function nowMillis(): number {
  return Number(host.nowMillis());
}

/** What the client is: host version, client build, platform, locale, base theme, reduced motion, streamer mode. */
export function client(): ClientInfo {
  return host.client();
}

/** The host features that resolved on this client build (slot names, decoration kinds, `surface:<name>`, ...). */
export function features(): string[] {
  return host.features();
}

/** Whether `name` is among `features()`. */
export function hasFeature(name: string): boolean {
  return host.hasFeature(name);
}

/** A bundled string (`i18n/<locale>.toml` in your project) in the user's language, else English, else `key`. */
export function tr(key: string): string {
  return host.tr(key);
}

/** A color from 0-255 channels, alpha 255 unless given. */
export function rgb(r: number, g: number, b: number, a = 255): Color {
  return { r, g, b, a };
}

/** A color from `#rrggbb`, `#rrggbbaa`, `#rgb` or a number `0xrrggbb`. */
export function hex(value: string | number): Color {
  if (typeof value === 'number') return rgb((value >> 16) & 255, (value >> 8) & 255, value & 255);
  let s = value.replace(/^#/, '');
  if (s.length === 3) s = [...s].map((c) => c + c).join('');
  if (!/^[0-9a-fA-F]{6}([0-9a-fA-F]{2})?$/.test(s)) throw new RekindleError('hex', 'invalid', `not a hex color: ${value}`);
  const n = (i: number) => parseInt(s.slice(i, i + 2), 16);
  return rgb(n(0), n(2), n(4), s.length === 8 ? n(6) : 255);
}

/** `#rrggbb` (or `#rrggbbaa` if not opaque). */
export function toHex(c: Color): string {
  const h = (n: number) => n.toString(16).padStart(2, '0');
  return `#${h(c.r)}${h(c.g)}${h(c.b)}${c.a === 255 ? '' : h(c.a)}`;
}

/** A semantic theme token as a span, highlight or background color: `token('text-muted')`. */
export function token(name: string): SpanColor {
  return { tag: 'token', val: name };
}

/** A plugin color as a span, highlight or background color. */
export function rgba(c: Color): SpanColor {
  return { tag: 'rgba', val: c };
}

export interface SpanStyle {
  bold?: boolean;
  italic?: boolean;
  code?: boolean;
  /** A theme token name (`'text-muted'`), a color, or a `SpanColor`. */
  color?: string | Color | SpanColor;
  /** What clicking it does (host-handled; links go through the client's "leaving Discord" confirm). */
  target?: Target;
}

/** A text run, for rich text, ghost messages and render transforms: `span('note', { bold: true, color: 'text-muted' })`. */
export function span(text: string, style: SpanStyle = {}): Span {
  const c = style.color;
  const color: SpanColor | undefined = c === undefined ? undefined : typeof c === 'string' ? token(c) : 'tag' in c ? c : rgba(c);
  return { text, bold: style.bold ?? false, italic: style.italic ?? false, code: style.code ?? false, color, target: style.target };
}

/**
 * Whether `granted` (from `activate`) includes `capability`, counting the
 * implied ones (`read-messages` implies `read-users`, `act` implies `react`).
 */
export function has(granted: Capability[], capability: Capability): boolean {
  return (
    granted.includes(capability) ||
    (capability === 'read-users' && granted.includes('read-messages')) ||
    (capability === 'react' && granted.includes('act'))
  );
}

/**
 * An error from the client: `code` is the contract's error value
 * (`'quota-exceeded'`, `'unavailable'`, `'no-gesture'`, `'host-not-allowed'`,
 * ...), `call` the SDK function that got it.
 */
export class RekindleError extends Error {
  constructor(
    readonly call: string,
    readonly code: string,
    message?: string,
    readonly detail?: unknown,
  ) {
    super(message ?? `${call}: ${code}`);
    this.name = 'RekindleError';
  }
}

/** @internal Run a contract call, turning its error result into a RekindleError. */
export function call<T>(name: string, f: () => T): T {
  try {
    return f();
  } catch (e) {
    const payload = (e as { payload?: unknown } | null)?.payload;
    if (payload === undefined) throw e;
    if (typeof payload === 'string') throw new RekindleError(name, payload);
    const { tag, val } = payload as { tag: string; val?: unknown };
    throw new RekindleError(name, tag, val === undefined ? `${name}: ${tag}` : `${name}: ${tag}: ${val}`, val);
  }
}

/**
 * @internal Callbacks registered through the SDK, which the build's entry
 * module calls before the plugin's own handler: `ui` by surface and element,
 * `slot` by item id, `decoration` by kind, `profile` by section, `click` by
 * binding, `slash`/`autocomplete` by command name, `event` by kind, the rest
 * by id or key.
 */
export const __registry = {
  ui: new Map<string, (value: unknown, event: UiEvent) => void>(),
  command: new Map<string, () => void>(),
  keybind: new Map<string, () => void>(),
  /** Keyed by slot and item id (`slotKey`): one id may be in several slots. */
  slot: new Map<string, (target: Target, click: SlotClick) => void>(),
  decoration: new Map<string, (key: bigint, kind: string) => void>(),
  profile: new Map<string, (user: UserRef, section: string) => void>(),
  click: new Map<string, (message: MessageRef, binding: string) => void>(),
  slash: new Map<string, (inv: Invocation) => CommandReply | undefined | void>(),
  autocomplete: new Map<string, (option: string, partial: string) => Array<[string, string]> | undefined | void>(),
  timer: new Map<string, () => void>(),
  settings: new Map<string, (key: string) => void>(),
  event: new Map<string, (value: unknown, event: Event) => void>(),
};

/** @internal u64 parameters take a number or a bigint. */
export function u64(n: Id): bigint {
  return typeof n === 'bigint' ? n : BigInt(Math.trunc(n));
}

/** @internal An optional u64. */
export function optU64(n: Id | undefined | null): bigint | undefined {
  return n === undefined || n === null ? undefined : u64(n);
}
