// Staff Tag: an "Owner" or "Admin" badge after the author's name, in each
// server where they are its owner or hold a role with Administrator.
//
// The badge depends on the server, so it is drawn per message by `render`,
// in the render isolate. The isolate can't query the client, so the main
// instance does: it keeps each server's owner and administrator roles in
// storage, and the isolate reads them there. Main to isolate is allowed;
// nothing flows back.

import { definePlugin, hex, type Transform } from 'rekindle';
import * as events from 'rekindle/events';
import { perms } from 'rekindle/consts';
import * as render from 'rekindle/render';
import * as state from 'rekindle/state';
import type { Guild } from 'rekindle/state';
import * as storage from 'rekindle/storage';

/** What the isolate needs about one server: its owner and admin roles. */
interface Staff {
  owner: string;
  admins: string[];
}

const key = (guild: bigint | string): string => `staff:${guild}`;

/** Store `guild`'s owner and the roles that grant Administrator. */
function remember(guild: Guild): void {
  const admins = state
    .roles(guild.id)
    .filter((r) => (r.permissions & perms.ADMINISTRATOR) !== 0n)
    .map((r) => r.id.toString());
  const staff: Staff = { owner: guild.owner.toString(), admins };
  storage.setJson(key(guild.id), staff);
}

function rememberAll(): void {
  for (const id of state.guildIds()) {
    const g = state.guild(id);
    if (g) remember(g);
  }
  render.invalidate();
}

const OWNER = { tooltip: 'Server owner', text: 'Owner', color: hex('#f0b232') };
const ADMIN = { tooltip: 'Has Administrator', text: 'Admin', color: hex('#5865f2') };

export default definePlugin({
  activate() {
    rememberAll();
    events.subscribe(['guild', 'role']);
    events.on('guild-joined', (g) => {
      remember(g);
      render.invalidate();
    });
    events.on('guild-updated', (g) => {
      remember(g);
      render.invalidate();
    });
    events.on('guild-left', ([id]) => storage.remove(key(id)));
    events.on('role-updated', () => rememberAll());
    events.on('role-deleted', () => rememberAll());
  },

  render(input): Transform[] {
    const guild = input.message.guild;
    if (guild === undefined || input.grouped) return [];
    const staff = storage.getJson<Staff | undefined>(key(guild), undefined);
    if (!staff) return [];
    const author = input.message.author.id.toString();
    if (author === staff.owner) return [render.authorBadge(OWNER)];
    if (input.authorRoles.some((r) => staff.admins.includes(r.toString()))) return [render.authorBadge(ADMIN)];
    return [];
  },
});
