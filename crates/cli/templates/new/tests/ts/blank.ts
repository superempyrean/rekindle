// Run with `rekindle test`, which starts the simulated client first. Each
// test starts with `preview.reset()`: a fresh client and empty storage.
import { test, assert, preview } from '#rekindle/test';

test('activates', async () => {
  await preview.reset();
  await preview.assertActive();
  const logs = await preview.logs();
  assert.ok(logs.some((l) => l.message.includes('is up')), JSON.stringify(logs));
});
