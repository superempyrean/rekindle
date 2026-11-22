// Exercises the SDK: callbacks registered where things are created (panel
// elements, a command, a menu item, a click binding, a keybind, a slash
// command, a setting, an event kind, a timer), the plugin's own handlers, and
// the render isolate, each logging what it saw. (`surface.ts` beside this is
// type-checked only.)
import { definePlugin, hex, log, nowMillis, RekindleError, span, toHex } from 'rekindle';
import { writeText } from 'rekindle/clipboard';
import * as commands from 'rekindle/commands';
import * as events from 'rekindle/events';
import { markAllRead, startReply } from 'rekindle/local-actions';
import * as render from 'rekindle/render';
import * as settings from 'rekindle/settings';
import * as slots from 'rekindle/slots';
import * as state from 'rekindle/state';
import * as storage from 'rekindle/storage';
import * as timer from 'rekindle/timer';
import { Panel, registerCommand, registerKeybind, toast, uiValue } from 'rekindle/ui';

function show(): void {
  new Panel('settings', 'SDK fixture')
    .text(`count is ${storage.getOr('count', 0)}`)
    .toggle('on', 'Enabled', storage.getOr('on', false))
    .textInput('name', 'Name', storage.get('name') ?? '', 'your name')
    .button('bump', 'Bump')
    .on('bump', () => {
      storage.set('count', storage.getOr('count', 0) + 1);
      show();
    })
    .show();
}

/** Off a user gesture, an action is refused. */
function tryAction(): void {
  try {
    markAllRead();
    log.info('marked read off a gesture?!');
  } catch (e) {
    if (e instanceof RekindleError) log.warn(`caught ${e.call} ${e.code}`);
  }
}

export default definePlugin({
  activate(granted) {
    log.info(`granted ${granted.length}; t is a ${typeof nowMillis()}; me is ${state.me().username}`);
    registerCommand('copy', 'Copy the name', () => writeText(storage.get('name') ?? ''));
    registerCommand('timer', 'Timer', () =>
      timer.set('t', 60, () => {
        log.info('timer callback t');
        tryAction();
      }),
    );
    registerCommand('json', 'JSON');
    registerCommand('error', 'Error');
    slots.userMenuItem(new slots.MenuItem('greet', 'Greet').icon('smile'), (user) => toast(`hi ${user.name}`, 'success'));
    slots.bindClick('reply', { label: 'Reply on double-click', when: ['others-message'] }, (m) => startReply(m, settings.bool('loud')));
    registerKeybind('shout', 'Shout', 'Ctrl+Shift+S', () => toast('keybind'));
    commands.register(
      new commands.SlashCommand('echo', 'Echo').text('text', 'What to say', true).autocomplete(),
      (inv) => commands.insert(`${commands.text(inv, 'text') ?? ''}!`),
      (_option, partial) => [[`${partial}1`, `${partial}1`]],
    );
    settings.onChange('loud', () => log.info(`loud is ${settings.bool('loud')}`));
    events.on('message-created', (m) => log.info(`message from ${m.author.name}`));
    events.subscribe(['message']);
    show();
  },
  // Text and toggle through the handler; the button through its callback.
  onUi(ev) {
    const text = uiValue.text(ev.value);
    const on = uiValue.toggled(ev.value);
    if (text !== undefined) storage.set('name', text);
    if (on !== undefined) storage.set('on', on);
    if (text !== undefined || on !== undefined) show();
  },
  onCommand(id) {
    if (id === 'json') {
      storage.setJson('obj', { a: [1, 2] });
      log.info(`json ${JSON.stringify(storage.getJson('obj', null))} ${toHex(hex('#0af'))}`);
    }
    if (id === 'error') {
      try {
        writeText('x'.repeat(70 * 1024));
      } catch (e) {
        if (e instanceof RekindleError) log.warn(`caught ${e.call} ${e.code}`);
      }
    }
  },
  onTimer(id) {
    log.info(`timer ${id}`);
  },
  // The render isolate: reads storage and logs, but can't write.
  render(input) {
    const content = input.message.content;
    if (content.includes('write')) storage.set('leak', content);
    console.log(`rendering ${content.length} chars`);
    const found = render.highlightAll(content, 'fixture', 'text-brand');
    return [...(found ? [found] : []), render.append([span(`read ${storage.getOr('count', 0)}`, { italic: true })])];
  },
});
