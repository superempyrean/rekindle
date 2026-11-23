// Exercises the prelude: each command logs what a sandboxed global did.
import { randomUUID } from 'node:crypto';

import { get, set } from 'rekindle:plugin/storage@0.10.0';
import { registerCommand } from 'rekindle:plugin/ui@0.10.0';

console.log('hello from the top level');

const attempt = (what, f) => {
  try {
    console.info(`${what} = ${f()}`);
  } catch (e) {
    console.warn(`${what} threw: ${e.message}`);
  }
};

export function activate() {
  for (const id of ['clock', 'entropy', 'timers', 'log-shapes', 'throw']) registerCommand(id, id);
  // A different Math.random() sequence each session: remember this one's.
  const previous = get('first-random');
  const first = String(Math.random());
  console.info(`random differs from last session: ${previous !== undefined && previous !== first}`);
  set('first-random', first);
}

export function onCommand(id) {
  switch (id) {
    case 'clock':
      attempt('Date.now()', () => Date.now());
      attempt('new Date()', () => new Date());
      attempt('new Date(0)', () => new Date(0).toISOString());
      attempt('performance.now()', () => typeof performance.now());
      break;
    case 'entropy':
      attempt('crypto.getRandomValues()', () => crypto.getRandomValues(new Uint8Array(4)));
      attempt('randomUUID()', () => randomUUID());
      break;
    case 'timers': {
      setTimeout((who) => console.info(`timeout fired for ${who}`), 10, 'maple');
      const cancelled = setTimeout(() => console.info('cancelled timeout fired'), 10);
      clearTimeout(cancelled);
      let n = 0;
      const every = setInterval(() => {
        n += 1;
        console.info(`interval tick ${n}`);
        if (n === 2) clearInterval(every);
      }, 50);
      break;
    }
    case 'log-shapes':
      console.log('object', { a: [1, 2], big: 7n }, 'error', new Error('shown'));
      break;
    case 'throw':
      throw new Error('boom from onCommand');
  }
}

export function onTimer(id) {
  console.info(`plugin timer ${id}`);
}
