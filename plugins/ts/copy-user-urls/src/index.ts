// Copy User URLs, in TypeScript: the same plugin as `plugins/rust/copy-user-urls`
// (Rust), held to the same tests.
//
// Adds "Copy User URL" to every user's right-click menu (a `MenuItem` in the
// "copy" group). Here the click is handled in `onSlot` rather than by a
// callback, to show the event: the target says which user was clicked. The
// click is the user's gesture, which is the only time the client lets a
// plugin write the clipboard.

import { definePlugin, log, RekindleError, type UserRef } from 'rekindle';
import { writeText } from 'rekindle/clipboard';
import * as slots from 'rekindle/slots';
import { toast } from 'rekindle/ui';

const ITEM = 'copy-user-url';

/** The canonical profile link for a user id. */
const userUrl = (id: bigint): string => `https://discord.com/users/${id}`;

function copy(user: UserRef): void {
  try {
    writeText(userUrl(user.id));
  } catch (e) {
    toast(`Couldn't copy: ${e instanceof RekindleError ? e.code : e}`, 'error');
    return;
  }
  // Some clients only hand over the id; don't print an empty name.
  toast(user.name ? `Copied ${user.name}'s URL` : 'Copied user URL', 'success');
}

export default definePlugin({
  activate() {
    try {
      slots.add(slots.USER_MENU, new slots.MenuItem(ITEM, 'Copy User URL').icon('link').group('copy'));
    } catch (e) {
      if (e instanceof RekindleError && e.code === 'unavailable') log.warn("user context menus aren't available on this client build");
      else log.error(`couldn't add the menu item: ${e}`);
    }
  },

  onSlot(ev) {
    const click = slots.clicked(ev);
    if (click?.item === ITEM && click.target.tag === 'user') copy(click.target.val[0]);
  },
});
