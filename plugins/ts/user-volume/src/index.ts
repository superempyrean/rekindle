// User Volume: "Set Volume…" in the user menu opens a dialog to set that
// user's voice volume exactly, from 0% to 200%. The client's own slider
// is coarse; this takes a typed number or steps of 5%.
//
// Setting a user's volume is a local action: only on the user's click.

import { definePlugin, log, RekindleError, type UserRef } from 'rekindle';
import { setUserVolume } from 'rekindle/local-actions';
import * as slots from 'rekindle/slots';
import { closeModal, Panel, toast } from 'rekindle/ui';

const ITEM = 'set-volume';
const MAX = 200;

/** A typed volume as a percentage in range, or `undefined`. */
function parsePercent(text: string): number | undefined {
  const n = Number(text.trim().replace(/%$/, ''));
  return Number.isFinite(n) && n >= 0 && n <= MAX ? Math.round(n) : undefined;
}

/** The user the dialog is open for, and the volume it shows. */
let open: { user: UserRef; percent: number } | undefined;

function show(): void {
  if (!open) return;
  new Panel('volume', `Volume for ${open.user.name || 'this user'}`)
    .slider('percent', 'Volume (%)', open.percent, { min: 0, max: MAX, step: 5 })
    .textInput('typed', 'Or type a percentage', String(open.percent), '100')
    .button('apply', 'Apply')
    .button('close', 'Close')
    .showModal();
}

function apply(): void {
  if (!open) return;
  try {
    setUserVolume(open.user.id, open.percent);
    const whose = open.user.name ? `${open.user.name}'s` : 'their';
    toast(`Set ${whose} volume to ${open.percent}%`, 'success');
  } catch (e) {
    toast(`Couldn't set the volume: ${e instanceof RekindleError ? e.code : e}`, 'error');
  }
}

export default definePlugin({
  activate() {
    try {
      slots.userMenuItem(new slots.MenuItem(ITEM, 'Set Volume…').icon('headphones'), (user) => {
        open = { user, percent: 100 };
        show();
      });
    } catch (e) {
      log.warn(`no user menu on this client build: ${e instanceof RekindleError ? e.code : e}`);
    }
  },

  onUi(ev) {
    if (ev.surface !== 'volume' || !open) return;
    switch (ev.element) {
      case 'percent':
        if (ev.value.tag === 'number') open.percent = Math.round(ev.value.val);
        show();
        break;
      case 'typed': {
        const p = ev.value.tag === 'text' ? parsePercent(ev.value.val) : undefined;
        if (p !== undefined) open.percent = p;
        break;
      }
      case 'apply':
        apply();
        show();
        break;
      case 'close':
        open = undefined;
        closeModal();
        break;
    }
  },
});
