// Permissions Viewer: "View Permissions" in the user and channel menus. For
// a member, it lists what they can do in the server; for a channel, what
// you can do in it. Administrator is shown as such, since it grants all.
//
// Everything comes from the client's own state (`read-client` for roles and
// your permissions, `read-users` for another member's).

import { definePlugin, log, RekindleError } from 'rekindle';
import { permissionNames, perms } from 'rekindle/consts';
import * as slots from 'rekindle/slots';
import * as state from 'rekindle/state';
import { closeModal, Panel } from 'rekindle/ui';
import * as users from 'rekindle/users';

const ITEM = 'view-permissions';

/** The lines the dialog lists for `bits`. */
function describe(bits: bigint): string[] {
  if ((bits & perms.ADMINISTRATOR) !== 0n) return ['Administrator (every permission)'];
  const names = permissionNames(bits);
  return names.length ? names : ['No permissions'];
}

function show(title: string, subtitle: string, bits: bigint | undefined): void {
  const p = new Panel('permissions', title).text(subtitle);
  if (bits === undefined) p.text("The client hasn't loaded this member's roles.");
  else for (const line of describe(bits)) p.text(`• ${line}`);
  p.button('close', 'Close').showModal();
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
    add('user menu', () =>
      slots.userMenuItem(new slots.MenuItem(ITEM, 'View Permissions').icon('shield'), (user, guild) => {
        if (guild === undefined) return;
        const where = state.guild(guild)?.name ?? 'this server';
        const name = user.name || state.displayName(user.id) || 'This member';
        show(`${name}'s permissions`, `In ${where}`, users.memberPermissions(user.id, guild));
      }),
    );
    add('channel menu', () =>
      slots.channelMenuItem(new slots.MenuItem(ITEM, 'View Permissions').icon('shield'), (channel) => {
        show('Your permissions', `In #${channel.name || 'this channel'}`, state.myPermissions(channel.id));
      }),
    );
  },

  onUi(ev) {
    if (ev.surface === 'permissions' && ev.element === 'close') closeModal();
  },
});
