// {{TITLE}}: recolors a few theme tokens.
//
// `theme.tokens()` (or the token table in `rekindle dev`) lists every token
// the client exposes. Overrides are undone when the plugin unloads.

import { definePlugin, hex, log } from 'rekindle';
import * as theme from 'rekindle/theme';

const PALETTE = {
  'background-base-low': hex('#1e1e2e'),
  'background-base-lower': hex('#181825'),
  'background-base-lowest': hex('#11111b'),
  'background-brand': hex('#cba6f7'),
  'text-default': hex('#cdd6f4'),
};

export default definePlugin({
  activate() {
    const unknown = theme.setMany(PALETTE);
    if (unknown.length) log.warn(`unknown tokens: ${unknown.join(', ')}`);
    log.info('{{TITLE}} applied');
  },
});
