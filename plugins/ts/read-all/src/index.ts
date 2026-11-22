// Read All, in TypeScript: the same plugin as `plugins/rust/read-all` (Rust), held
// to the same tests. After Vencord's ReadAllNotificationsButton: a button on
// top of the server list that marks everything read.
//
// The smallest gesture-bound action plugin: `markAllRead` only works while the
// client is delivering the user's click (or palette command), so the plugin
// can offer the button but could never mark things read on its own. The
// button's tooltip counts unread channels from client state (`read-client`),
// kept fresh by `read-state` events.

import { definePlugin, log, RekindleError } from 'rekindle';
import * as events from 'rekindle/events';
import { markAllRead } from 'rekindle/local-actions';
import * as slots from 'rekindle/slots';
import * as state from 'rekindle/state';
import { registerCommand, toast } from 'rekindle/ui';

function tooltip(): string {
  const n = state.unreadChannels(200).length;
  return n === 0 ? 'Nothing unread' : n === 1 ? 'Mark 1 channel read' : `Mark ${n} channels read`;
}

function showButton(): void {
  slots.button(slots.GUILD_LIST, 'read-all', 'check', tooltip(), { onClick: readAll });
}

function readAll(): void {
  try {
    markAllRead();
    toast('Marked everything read', 'success');
  } catch (e) {
    toast(`Couldn't mark read: ${e instanceof RekindleError ? e.code : e}`, 'error');
  }
  showButton();
}

export default definePlugin({
  activate() {
    try {
      showButton();
    } catch (e) {
      log.warn(`no server-list button on this build: ${e instanceof RekindleError ? e.code : e}`);
    }
    registerCommand('read-all', 'Mark everything read', readAll);
    events.subscribe(['read-state']);
    events.on('read-state', () => {
      try {
        showButton();
      } catch {
        // No button on this build; nothing to refresh.
      }
    });
  },
});
