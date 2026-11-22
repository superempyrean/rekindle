// Friend Notifications: a desktop notification when a friend comes online,
// and optionally when they go offline. By default it watches every friend;
// the "Only these friends" setting narrows that.
//
// It keeps each friend's last status in memory only, to tell a change from
// a repeat. Nothing is stored.

import { definePlugin } from 'rekindle';
import * as events from 'rekindle/events';
import * as notify from 'rekindle/notify';
import * as settings from 'rekindle/settings';
import * as state from 'rekindle/state';
import * as users from 'rekindle/users';

/** user id -> their last status ("online", "idle", "dnd", "offline"). */
const last = new Map<bigint, string>();

function watched(user: bigint): boolean {
  if (users.relationship(user) !== 'friend') return false;
  const only = settings.ids('only');
  return only.length === 0 || only.includes(user);
}

const online = (status: string | undefined): boolean => status !== undefined && status !== 'offline' && status !== 'invisible';

export default definePlugin({
  activate() {
    for (const r of users.relationships()) {
      if (r.kind === 'friend') last.set(r.user, users.presence(r.user)?.status ?? 'offline');
    }
    events.subscribe(['presence']);
    events.on('presence', (p) => {
      const before = last.get(p.user);
      last.set(p.user, p.status);
      if (!watched(p.user) || online(before) === online(p.status)) return;
      const name = state.displayName(p.user) ?? 'A friend';
      if (online(p.status)) notify.post(`${name} is online`, '', { tag: 'user', val: [{ id: p.user, name }, undefined] });
      else if (settings.bool('offline')) notify.post(`${name} went offline`, '');
    });
  },
});
