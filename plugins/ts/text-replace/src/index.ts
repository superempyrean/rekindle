// Text Replace, in TypeScript: the same plugin as `plugins/rust/text-replace`
// (Rust), held to the same tests.
//
// You keep a list of find/replace rules in its settings panel. Each rule is a
// plain string or a regex (`$1`-style capture references in the replacement).
// Rules run in order on every message *you* send, just before it goes out: the
// client hands this plugin your draft through `onCompose` and sends whatever
// comes back. It never sees anyone else's messages, and it can't send
// anything on its own.

import { definePlugin, RekindleError } from 'rekindle';
import * as storage from 'rekindle/storage';
import { Panel, toast, uiValue } from 'rekindle/ui';

import { compile, decode, encode, MAX_RULES, type Compiled, type Rule } from './rules';

const PANEL = 'settings';
const KEY = 'rules';

let rules: Compiled[] = [];

function save(): void {
  try {
    storage.set(KEY, encode(rules.map((c) => c.rule)));
  } catch (e) {
    toast(`Couldn't save rules: ${e instanceof RekindleError ? e.code : e}`, 'error');
  }
}

function show(): void {
  const panel = new Panel(PANEL, 'Text Replace').text(
    'Rules run top to bottom on messages you send. Regex rules use $1, $2, ... for capture groups.',
  );
  rules.forEach((c, i) => {
    panel
      .heading(`Rule ${i + 1}`)
      .textInput(`find-${i}`, 'Find', c.rule.find, 'text or pattern')
      .textInput(`replace-${i}`, 'Replace', c.rule.replace, 'replacement')
      .toggle(`regex-${i}`, 'Regex', c.rule.regex);
    if (c.error) panel.text(`Invalid regex, rule skipped: ${c.error}`);
    panel.button(`delete-${i}`, 'Delete rule');
  });
  if (rules.length < MAX_RULES) {
    panel.separator().button('add', 'Add rule').on('add', () => {
      rules.push(compile({ find: '', replace: '', regex: false }));
      save();
      show();
    });
  }
  panel.show();
}

/** Strip the storage separators so a typed value can't corrupt storage. */
const clean = (s: string) => s.replace(/[\u001e\u001f]/g, '');

export default definePlugin({
  activate() {
    rules = decode(storage.get(KEY) ?? '').map(compile);
    show();
  },

  // The per-rule elements, by their `<field>-<index>` ids.
  onUi(ev) {
    if (ev.surface !== PANEL || ev.element === 'add') return;
    const dash = ev.element.lastIndexOf('-');
    const field = ev.element.slice(0, dash);
    const i = Number(ev.element.slice(dash + 1));
    if (dash < 0 || !Number.isInteger(i) || i < 0 || i >= rules.length) return;
    const wasValid = !rules[i].error;
    const update = (f: (r: Rule) => Rule) => (rules[i] = compile(f({ ...rules[i].rule })));

    const text = uiValue.text(ev.value);
    const on = uiValue.toggled(ev.value);
    if (field === 'find' && text !== undefined) update((r) => ({ ...r, find: clean(text) }));
    else if (field === 'replace' && text !== undefined) update((r) => ({ ...r, replace: clean(text) }));
    else if (field === 'regex' && on !== undefined) update((r) => ({ ...r, regex: on }));
    else if (field === 'delete' && uiValue.clicked(ev.value)) rules.splice(i, 1);
    else return;

    save();
    // Typing into a field shouldn't rebuild the panel under the cursor,
    // unless the rule's regex just became (in)valid.
    const structural = field === 'delete' || field === 'regex';
    if (structural || !rules[i].error !== wasValid) show();
  },

  onCompose(draft) {
    const out = rules.reduce((text, c) => c.apply(text), draft.text);
    return out !== draft.text ? out : undefined;
  },
});
