// Keyword Highlight, in TypeScript: the same plugin as
// `plugins/rust/keyword-highlight` (Rust), held to the same tests. Highlights words
// you choose in every message, and optionally tints the whole row.
//
// A `render-messages` plugin: `render` runs in the render isolate, which sees
// message content but can't keep or send it, so this doesn't need
// `read-messages`. It never activates there, so it reads its settings each
// time; the keyword list is a `string-list` setting declared in package.json,
// and changing it re-renders by itself.

import { definePlugin, token, type Transform } from 'rekindle';
import * as render from 'rekindle/render';
import * as settings from 'rekindle/settings';

export default definePlugin({
  render(input) {
    const style = { color: token('text-feedback-warning'), bold: true };
    const found = settings
      .items('keywords')
      .map((k) => render.highlightAll(input.message.content, k.trim(), style))
      .filter((t): t is Transform => t !== undefined);
    if (found.length && settings.bool('tint-rows')) found.push(render.background('background-mentioned'));
    return found;
  },
});
