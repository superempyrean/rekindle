// Run with `rekindle test`, which starts the simulated client first.
import { test, assert, preview } from '#rekindle/test';

test('counts channel switches', async () => {
  await preview.reset();
  assert.ok((await preview.panelText('status')).includes('Channel switches this session: 0'));
  await preview.select(await preview.channel('general'));
  await preview.select(await preview.channel('off-topic'));
  const text = await preview.panelText('status');
  assert.ok(text.includes('Channel switches this session: 2'), JSON.stringify(text));
  assert.ok(text.includes('Current channel: #off-topic'), JSON.stringify(text));
});
