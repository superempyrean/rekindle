// Types for the rekindle test client (`#rekindle/test`).

type TestFn = (t: unknown) => void | Promise<void>;
export declare function test(name: string, fn: TestFn): Promise<void>;
export declare function test(name: string, options: { skip?: boolean | string; only?: boolean; timeout?: number }, fn: TestFn): Promise<void>;
export declare function describe(name: string, fn: () => void | Promise<void>): Promise<void>;
export declare function before(fn: () => void | Promise<void>): void;
export declare function after(fn: () => void | Promise<void>): void;
export declare function beforeEach(fn: () => void | Promise<void>): void;
export declare function afterEach(fn: () => void | Promise<void>): void;

/** `node:assert/strict`. */
export declare const assert: {
  (value: unknown, message?: string): asserts value;
  ok(value: unknown, message?: string): asserts value;
  equal<T>(actual: unknown, expected: T, message?: string): asserts actual is T;
  notEqual(actual: unknown, expected: unknown, message?: string): void;
  deepEqual<T>(actual: unknown, expected: T, message?: string): asserts actual is T;
  notDeepEqual(actual: unknown, expected: unknown, message?: string): void;
  match(value: string, pattern: RegExp, message?: string): void;
  doesNotMatch(value: string, pattern: RegExp, message?: string): void;
  throws(fn: () => unknown, expected?: RegExp | Function | object, message?: string): void;
  rejects(fn: Promise<unknown> | (() => Promise<unknown>), expected?: RegExp | Function | object, message?: string): Promise<void>;
  fail(message?: string): never;
  AssertionError: new (options: { message?: string; actual?: unknown; expected?: unknown }) => Error;
};

type Id = bigint | number | string;
/** The preview's whole state, as `GET /api/state` returns it. */
export type State = Record<string, any>;
export type SettingValue = boolean | number | string | string[] | { kind: string; value?: unknown };
export interface Toast { text: string; kind: 'info' | 'success' | 'warning' | 'error' }
export interface LogLine { level: 'error' | 'warn' | 'info' | 'debug' | 'trace'; message: string }
export interface Mods { shift?: boolean; ctrl?: boolean; alt?: boolean; meta?: boolean }

export declare const preview: {
  /** The id of the plugin under test. */
  readonly plugin: string;
  state(): Promise<State>;
  /** A fresh simulated client and empty storage, with the plugin loaded again. */
  reset(): Promise<State>;
  /** Rebuild and restart the plugin. */
  reload(): Promise<State>;

  command(id: string): Promise<State>;
  keybind(id: string): Promise<State>;
  slash(command: string, options?: Record<string, string>, channel?: Id): Promise<State>;
  /** Click one of the plugin's slot items. `target` is what it was clicked on (default: nothing). */
  click(item: string, options?: { slot?: string; target?: unknown; mods?: Mods }): Promise<State>;
  /** An interaction with one of the plugin's surfaces: `ui('main', 'save', { kind: 'clicked' })`. */
  ui(surface: string, element: string, value: { kind: string; value?: unknown }): Promise<State>;

  receive(channel: Id, author: Id, content: string): Promise<State>;
  send(channel: Id, text: string): Promise<State>;
  select(channel: Id): Promise<State>;
  /** Any client event, as the preview's event injector takes it: `{ Typing: [channel, [user]] }`. */
  event(ev: unknown): Promise<State>;

  setSetting(key: string, value: SettingValue): Promise<State>;
  resetSetting(key: string): Promise<State>;

  /** `'active'`, `'faulted'`, `'failed'` (didn't load), ... */
  status(): Promise<string>;
  /** Fail (with the plugin's logs) unless the plugin is active. */
  assertActive(): Promise<void>;
  /** The plugin's toasts, oldest first. */
  toasts(): Promise<Toast[]>;
  /** The plugin's storage, as `{ key: value }`. */
  storage(): Promise<Record<string, string>>;
  logs(): Promise<LogLine[]>;
  /** Actions the plugin performed as the user (marked read, reacted, ...). */
  performed(): Promise<unknown[]>;
  /** A theme token's value as the client has it now (`'#cba6f7'`, `'14px'`), whoever set it. */
  theme(token: string): Promise<string | undefined>;
  /** One of the plugin's panels as the client shows it. `undefined` if it isn't showing. */
  panel(id: string): Promise<{ id: string; title: string; elements: Array<Record<string, any>> } | undefined>;
  /** A panel's headings and text lines, and fields as `key: value`, top to bottom. */
  panelText(id: string): Promise<string[]>;
  /** The id of a channel in the simulated client, by name (`'general'`). */
  channel(name: string): Promise<bigint>;
};
