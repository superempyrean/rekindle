// Run with `rekindle test`, which starts the simulated client first.
import { test, assert, preview } from '#rekindle/test';

test('recolors the client', async () => {
  await preview.reset();
  await preview.assertActive();
  assert.equal(await preview.theme('background-brand'), '#cba6f7');
});
