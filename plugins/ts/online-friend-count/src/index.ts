// Online Friend Count: a button on the server list whose tooltip says how
// many friends are online. A click lists them. The count follows presence
// and relationship events.

import { definePlugin, log, RekindleError } from 'rekindle';
import * as events from 'rekindle/events';
import * as slots from 'rekindle/slots';
import * as state from 'rekindle/state';
import { closeModal, Panel } from 'rekindle/ui';
import * as users from 'rekindle/users';

const ID = 'online-friends';

/** Friends whose status isn't offline. */
function onlineFriends(): bigint[] {
  return users
    .relationships()
    .filter((r) => r.kind === 'friend')
    .map((r) => r.user)
    .filter((u) => {
      const s = users.presence(u)?.status;
      return s !== undefined && s !== 'offline' && s !== 'invisible';
    });
}

function tooltip(n: number): string {
  return n === 1 ? '1 friend online' : `${n} friends online`;
}

function list(): void {
  const p = new Panel('online', 'Friends online');
  const ids = onlineFriends();
  if (ids.length === 0) p.text('Nobody is online.');
  for (const id of ids) p.user(id);
  p.button('close', 'Close').showModal();
}

function refresh(): void {
  try {
    slots.button(slots.GUILD_LIST, ID, 'users', tooltip(onlineFriends().length), { onClick: list });
  } catch (e) {
    log.warn(`no server-list button on this client build: ${e instanceof RekindleError ? e.code : e}`);
  }
}

export default definePlugin({
  activate() {
    refresh();
    events.subscribe(['presence', 'relationship']);
    events.on('presence', refresh);
    events.on('relationship', refresh);
  },

  onUi(ev) {
    if (ev.surface === 'online' && ev.element === 'close') closeModal();
  },
});
