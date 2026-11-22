// The rekindle test client, written by `rekindle test` and `rekindle build`.
//
// `rekindle test` builds the plugin, runs it in the simulated client, and then
// runs your tests with Node's test runner. Each call here does what a user
// would (run a command, click a menu item, receive a message) and returns the
// preview's new state. Tests share one simulated client, so they run one at a
// time; call `reset()` first to start from a clean client and storage.

import { test, describe, before, after, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';

export { test, describe, before, after, beforeEach, afterEach, assert };

const BASE = process.env.REKINDLE_TEST_URL;
const PLUGIN = process.env.REKINDLE_TEST_PLUGIN;

async function call(method, path, body) {
  if (!BASE) throw new Error('nothing to test against: run these tests with `rekindle test`');
  const init = { method };
  if (body !== undefined) {
    init.headers = { 'content-type': 'application/json' };
    init.body = JSON.stringify(body, (_, v) => (typeof v === 'bigint' ? v.toString() : v));
  }
  const res = await fetch(BASE + path, init);
  const json = await res.json();
  if (!res.ok) {
    const err = new Error(`${path}: ${json.error ?? `HTTP ${res.status}`}`);
    err.status = res.status;
    err.state = json.state;
    throw err;
  }
  return json;
}

const id = (v) => String(v);
const NO_MODS = { shift: false, ctrl: false, alt: false, meta: false };

/** A setting value from a plain one: `true`, `3`, `'text'`, `['a', 'b']`, or `{ kind, value }` as is. */
function settingValue(v) {
  if (v && typeof v === 'object' && !Array.isArray(v) && 'kind' in v) return v;
  if (typeof v === 'boolean') return { kind: 'boolean', value: v };
  if (typeof v === 'number') return { kind: 'number', value: v };
  if (typeof v === 'string') return { kind: 'text', value: v };
  if (Array.isArray(v)) return { kind: 'items', value: v };
  throw new TypeError(`can't use ${JSON.stringify(v)} as a setting value; pass { kind, value }`);
}

const mine = (s) => s.plugins.find((p) => p.id === PLUGIN);

export const preview = {
  plugin: PLUGIN,

  state: () => call('GET', '/api/state'),
  /** A fresh simulated client and empty storage, with the plugin loaded again. */
  reset: () => call('POST', '/api/reset', {}),
  /** Rebuild and restart the plugin. */
  reload: () => call('POST', '/api/reload', { plugin: PLUGIN }),

  command: (cmd) => call('POST', '/api/command', { plugin: PLUGIN, id: cmd }),
  keybind: (kb) => call('POST', '/api/keybind', { plugin: PLUGIN, id: kb }),
  slash: (command, options = {}, channel) =>
    call('POST', '/api/slash', { plugin: PLUGIN, command, options, ...(channel === undefined ? {} : { channel: id(channel) }) }),
  /** Click one of the plugin's slot items. `target` is what it was clicked on (default: nothing). */
  click: (item, { slot, target = 'None', mods } = {}) =>
    call('POST', '/api/slot', { plugin: PLUGIN, item, target, ...(slot ? { slot } : {}), ...(mods ? { mods: { ...NO_MODS, ...mods } } : {}) }),
  /** An interaction with one of the plugin's surfaces: `ui('main', 'save', { kind: 'clicked' })`. */
  ui: (surface, element, value) => call('POST', '/api/ui', { plugin: PLUGIN, surface, element, value }),

  receive: (channel, author, content) => call('POST', '/api/receive', { channel: id(channel), author: id(author), content }),
  send: (channel, text) => call('POST', '/api/send', { channel: id(channel), text }),
  select: (channel) => call('POST', '/api/select', { channel: id(channel) }),
  /** Any client event, as the preview's event injector takes it: `{ Typing: [channel, [user]] }`. */
  event: (ev) => call('POST', '/api/event', ev),

  setSetting: (key, value) => call('POST', '/api/setting', { plugin: PLUGIN, key, value: settingValue(value) }),
  resetSetting: (key) => call('POST', '/api/setting/reset', { plugin: PLUGIN, key }),

  /** `'active'`, `'faulted'`, `'failed'` (didn't load), ... */
  async status() {
    const p = mine(await this.state());
    if (!p) return 'missing';
    if (p.load_error) return 'failed';
    return p.info?.status?.state ?? 'unknown';
  },
  async assertActive() {
    const s = await this.state();
    const p = mine(s);
    const state = p?.info?.status?.state;
    if (state === 'active' && !p.load_error) return;
    const why = p?.load_error ?? p?.info?.status?.reason ?? state ?? 'not loaded';
    const logs = s.logs.filter((l) => l.record.plugin === PLUGIN).map((l) => `  ${l.record.level}: ${l.record.message}`);
    throw new assert.AssertionError({ message: `the plugin isn't active: ${why}${logs.length ? `\n${logs.join('\n')}` : ''}` });
  },
  /** The plugin's toasts, oldest first: `[{ text, kind }]`. */
  async toasts() {
    return (await this.state()).slots.toasts.filter((t) => t.plugin === PLUGIN).map(({ text, kind }) => ({ text, kind }));
  },
  /** A theme token's value as the client has it now (`'#cba6f7'`, `'14px'`), whoever set it. */
  async theme(token) {
    const t = (await this.state()).theme.find((t) => t.token === token || (t.aliases ?? []).includes(token));
    return t?.value;
  },
  /** One of the plugin's panels as the client shows it: `{ id, title, elements }`. */
  async panel(panelId) {
    return mine(await this.state())?.panels?.find((p) => p.id === panelId);
  },
  /** A panel's headings and text lines, and fields as `key: value`, top to bottom. */
  async panelText(panelId) {
    const p = await this.panel(panelId);
    return (p?.elements ?? []).flatMap((e) =>
      e.kind === 'heading' || e.kind === 'text' ? [e.text] : e.kind === 'field' ? [`${e.key}: ${e.value}`] : [],
    );
  },
  /** The id of a channel in the simulated client, by name (`'general'`). */
  async channel(name) {
    const all = (await this.state()).client.channels;
    const c = all.find((c) => c.name === name);
    if (!c) throw new assert.AssertionError({ message: `no channel named \`${name}\`; the simulated client has ${all.map((c) => c.name).join(', ')}` });
    return BigInt(c.id);
  },
  /** The plugin's storage, as `{ key: value }`. */
  async storage() {
    return mine(await this.state())?.storage ?? {};
  },
  /** The plugin's log lines: `[{ level, message }]`. */
  async logs() {
    return (await this.state()).logs.filter((l) => l.record.plugin === PLUGIN).map(({ record: { level, message } }) => ({ level, message }));
  },
  /** Actions the plugin performed as the user (marked read, reacted, ...). */
  async performed() {
    return (await this.state()).performed;
  },
};
