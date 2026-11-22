// Quick Mention: a button in each message's hover bar, and an item in its
// menu, that puts a mention of the author into your draft at the caret.
//
// Inserting into the draft is a local action: it needs the user's click,
// so the plugin can offer the button but never type on its own.

import { definePlugin, log, RekindleError, type MessageRef } from 'rekindle';
import { insertText } from 'rekindle/local-actions';
import * as slots from 'rekindle/slots';
import { toast } from 'rekindle/ui';

const ITEM = 'mention';

/** The text inserted for `author`: a mention and a space to keep typing. */
const mention = (author: bigint): string => `<@${author}> `;

function insert(message: MessageRef): void {
  try {
    insertText(mention(message.author));
  } catch (e) {
    toast(`Couldn't mention: ${e instanceof RekindleError ? e.code : e}`, 'error');
  }
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
    add('message hover bar', () =>
      slots.button(slots.MESSAGE_HOVER_BAR, ITEM, 'at', 'Mention', {
        onClick: (target) => {
          if (target.tag === 'message') insert(target.val);
        },
      }),
    );
    add('message menu', () => slots.messageMenuItem(new slots.MenuItem(ITEM, 'Mention').icon('at'), (m) => insert(m)));
  },
});
