// {{TITLE}}: a settings panel driving the theme, saved in storage.
//
// Panels are stateless: show the panel again with current values after a
// change. Read storage in `activate`, not at the top level (which runs at
// build time).

import { definePlugin, hex, toHex, type Color } from 'rekindle';
import * as storage from 'rekindle/storage';
import * as theme from 'rekindle/theme';
import { Panel } from 'rekindle/ui';

let accent: Color = hex('#eb459e');
let enabled = true;

function update(): void {
  if (enabled) theme.set('background-brand', accent);
  else theme.reset('background-brand');

  new Panel('settings', '{{TITLE}}')
    .text('Pick an accent color for the client.')
    .toggle('enabled', 'Enabled', enabled)
    .colorPicker('accent', 'Accent', accent)
    .on('enabled', (on: boolean) => {
      enabled = on;
      storage.set('enabled', on);
      update();
    })
    .on('accent', (c: Color) => {
      accent = c;
      storage.set('accent', toHex(c));
      update();
    })
    .show();
}

export default definePlugin({
  activate() {
    accent = hex(storage.get('accent') ?? '#eb459e');
    enabled = storage.getOr('enabled', true);
    update();
  },
});
