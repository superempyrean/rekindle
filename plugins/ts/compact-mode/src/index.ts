// Compact Mode, in TypeScript: the same plugin as `plugins/rust/compact-mode`
// (Rust), held to the same tests. One palette command toggles the client's
// compact density, remembered across restarts.

import { definePlugin, log, RekindleError } from 'rekindle';
import * as storage from 'rekindle/storage';
import * as theme from 'rekindle/theme';
import { registerCommand, toast } from 'rekindle/ui';

function apply(compact: boolean): void {
  theme.setDensity(compact ? 'compact' : 'default');
}

export default definePlugin({
  activate() {
    apply(storage.getOr('compact', false));
    try {
      registerCommand('toggle', 'Toggle compact mode', () => {
        const compact = !storage.getOr('compact', false);
        storage.set('compact', compact);
        apply(compact);
        toast(compact ? 'Compact mode on' : 'Compact mode off');
      });
    } catch (e) {
      log.warn(`no command palette on this client build: ${e instanceof RekindleError ? e.code : e}`);
    }
  },
});
