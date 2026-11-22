// {{TITLE}}: reacts to client events (`read-client`: where you are and your
// own state, no message content, nothing about other people).

import { definePlugin, log } from 'rekindle';
import * as events from 'rekindle/events';
import * as state from 'rekindle/state';
import { Panel } from 'rekindle/ui';

let switches = 0;
let current = '(none yet)';

function show(): void {
  new Panel('status', '{{TITLE}}')
    .text(`Channel switches this session: ${switches}`)
    .text(`Current channel: ${current}`)
    .show();
}

export default definePlugin({
  activate() {
    events.on('selection-changed', (sel) => {
      if (sel.channel === undefined) return;
      switches += 1;
      // Events carry ids; the records come from client state.
      current = `#${state.channel(sel.channel)?.name ?? sel.channel}`;
      show();
    });
    events.on('my-presence', (status) => log.info(`presence is now ${status}`));
    // Only these kinds reach the plugin.
    events.subscribe(['selection-changed', 'my-presence']);
    show();
  },
});
