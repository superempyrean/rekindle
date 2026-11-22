// Run with `rekindle test`, which starts the simulated client first.
import { test, assert, preview } from '#rekindle/test';

test('the accent starts on', async () => {
  await preview.reset();
  assert.equal(await preview.theme('background-brand'), '#eb459e');
});

test('picking a color recolors and is saved', async () => {
  await preview.reset();
  await preview.ui('settings', 'accent', { kind: 'color', value: '#0080ff' });
  assert.equal(await preview.theme('background-brand'), '#0080ff');
  assert.equal((await preview.storage()).accent, '#0080ff');
});

test('turning it off restores the theme', async () => {
  await preview.reset();
  await preview.ui('settings', 'enabled', { kind: 'toggled', value: false });
  assert.notEqual(await preview.theme('background-brand'), '#eb459e');
  // It stays off after a restart.
  await preview.reload();
  assert.notEqual(await preview.theme('background-brand'), '#eb459e');
});
