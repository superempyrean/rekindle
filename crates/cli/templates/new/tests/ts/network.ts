// Run with `rekindle test`, which starts the simulated client first.
import { test, assert, preview } from '#rekindle/test';

test('shows its panel', async () => {
  await preview.reset();
  assert.deepEqual(await preview.panelText('main'), ['Last response: (nothing yet)']);
  // Clicking Fetch makes a real request to httpbin.org:
  // await preview.ui('main', 'fetch', { kind: 'clicked' });
});
