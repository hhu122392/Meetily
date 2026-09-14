import assert from 'node:assert/strict';
import test from 'node:test';

import { RecordingStartGate } from '../../src/lib/recording-start-gate';

test('coalesces concurrent start requests into one backend call', async () => {
  const gate = new RecordingStartGate();
  let active = false;
  let starts = 0;
  let releaseStart!: () => void;
  const blockedStart = new Promise<void>((resolve) => {
    releaseStart = resolve;
  });

  const first = gate.run(async () => active, async () => {
    starts += 1;
    await blockedStart;
    active = true;
  });
  const duplicate = gate.run(async () => active, async () => {
    starts += 1;
  });

  assert.equal(first, duplicate);
  assert.equal(starts, 0);
  await Promise.resolve();
  assert.equal(starts, 1);

  releaseStart();
  await Promise.all([first, duplicate]);
  assert.equal(starts, 1);
});
test('treats a request while already recording as idempotent success', async () => {
  const gate = new RecordingStartGate();
  let starts = 0;

  await gate.run(async () => true, async () => {
    starts += 1;
  });

  assert.equal(starts, 0);
});

test('allows a real retry after a failed start', async () => {
  const gate = new RecordingStartGate();
  let attempts = 0;

  await assert.rejects(
    gate.run(async () => false, async () => {
      attempts += 1;
      throw new Error('device unavailable');
    }),
    /device unavailable/,
  );

  await gate.run(async () => false, async () => {
    attempts += 1;
  });
  assert.equal(attempts, 2);
});
