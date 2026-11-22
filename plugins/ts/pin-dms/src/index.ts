// Pin DMs: "Pin DM" in a DM's channel menu and in a user's menu puts that
// conversation in a "Pinned" section at the top of the DM list; the same
// item unpins it. The pinned channels are kept in storage.

import { definePlugin, log, RekindleError } from 'rekindle';
import { setDmSections } from 'rekindle/decorations';
import * as slots from 'rekindle/slots';
import * as state from 'rekindle/state';
import * as storage from 'rekindle/storage';
import { toast } from 'rekindle/ui';

const ITEM = 'pin-dm';

function pinned(): string[] {
  return storage.getJson<string[]>('pinned', []);
}

function show(ids: string[]): void {
  setDmSections(ids.length ? [{ id: 'pinned', name: 'Pinned', channels: ids.map(BigInt) }] : []);
}

/** Pin `channel` if it isn't, else unpin it. */
function toggle(channel: bigint): void {
  const ids = pinned();
  const key = channel.toString();
  const now = ids.includes(key) ? ids.filter((i) => i !== key) : [...ids, key];
  storage.setJson('pinned', now);
  show(now);
  toast(now.includes(key) ? 'Pinned' : 'Unpinned', 'success');
}

function add(what: string, f: () => void): void {
  try {
    f();
  } catch (e) {
    log.warn(`no ${what} on this client build: ${e instanceof RekindleError ? e.code : e}`);
  }
}

export default definePlugin({
  activate() {
    // Drop pins of DMs that are gone.
    const open = new Set(state.privateChannels().map((c) => c.id.toString()));
    const ids = pinned().filter((i) => open.has(i));
    storage.setJson('pinned', ids);
    show(ids);
    const item = (when: slots.MenuPredicate) => new slots.MenuItem(ITEM, 'Pin DM').icon('pin').when(when);
    add('channel menu', () => slots.channelMenuItem(item('in-dm'), (c) => toggle(c.id)));
    add('user menu', () =>
      slots.userMenuItem(item('target-is-friend'), (u) => {
        const dm = state.dmChannel(u.id);
        if (dm === undefined) toast(`No DM with ${u.name || 'them'} yet`, 'info');
        else toggle(dm);
      }),
    );
  },
});
