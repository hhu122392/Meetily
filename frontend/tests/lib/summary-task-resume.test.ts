import assert from 'node:assert/strict';
import test from 'node:test';
import { setTimeout as delay } from 'node:timers/promises';
import { observeExistingSummaryTask, type NativeSummarySnapshot, type ResumedSummaryUpdate } from '../../src/lib/summary-task-resume';

const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
};

test('reopening a pending regeneration retains the old editor until the new saved result arrives', async t => {
  const updates: ResumedSummaryUpdate[] = [];
  const done = deferred<void>();
  let reads = 0;
  const stop = observeExistingSummaryTask({
    read: async () => ++reads === 1
      ? { status: 'pending', data: { markdown: 'old saved report' } }
      : { status: 'completed', data: { markdown: 'new saved report' } },
    onUpdate: update => { updates.push(update); if (update.status === 'completed') done.resolve(); },
    intervalMs: 1,
  });
  t.after(stop);
  await done.promise;
  assert.deepEqual(updates, [
    { status: 'regenerating', error: null },
    { status: 'completed', error: null, data: { markdown: 'new saved report' } },
  ]);
  await delay(10);
  assert.equal(reads, 2, 'A completed task must stop polling');
});

test('ordinary completed history is checked once without overwriting an editor or polling forever', async t => {
  const updates: ResumedSummaryUpdate[] = [];
  let reads = 0;
  t.after(observeExistingSummaryTask({
    read: async () => { reads++; return { status: 'completed', data: { markdown: 'saved' } }; },
    onUpdate: update => updates.push(update), intervalMs: 1,
  }));
  await delay(10);
  assert.equal(reads, 1);
  assert.deepEqual(updates, []);
});

test('recovery uses the recorded start time and never invents one when it is missing', async () => {
  for (const start of ['2026-09-12T22:29:10.247791700+00:00', null, 'invalid']) {
    const received = deferred<ResumedSummaryUpdate>();
    const stop = observeExistingSummaryTask({ read: async () => ({ status: 'pending', start }), onUpdate: update => received.resolve(update) });
    const update = await received.promise;
    stop();
    assert.equal(update.startedAt, start && start !== 'invalid' ? Date.parse(start) : undefined);
  }
});

test('first generation recovers processing and preserves the final needs-review state', async t => {
  const done = deferred<void>();
  const updates: ResumedSummaryUpdate[] = [];
  let reads = 0;
  const saved = { markdown: 'check this', factValidation: { status: 'needs_review' } };
  t.after(observeExistingSummaryTask({
    read: async () => ++reads === 1 ? { status: 'pending', data: null } : { status: 'completed', data: saved },
    onUpdate: update => { updates.push(update); if (update.status === 'needs_review') done.resolve(); }, intervalMs: 1,
  }));
  await done.promise;
  assert.deepEqual(updates.map(update => update.status), ['processing', 'needs_review']);
  assert.deepEqual(updates[1].data, saved);
});

for (const terminal of ['failed', 'cancelled', 'idle']) {
  test(`a resumed task ending as ${terminal} stops and retains an available backup`, async t => {
    const done = deferred<void>();
    const updates: ResumedSummaryUpdate[] = [];
    let reads = 0;
    const backup = { markdown: 'restored previous summary' };
    t.after(observeExistingSummaryTask({
      read: async () => ++reads === 1 ? { status: 'pending', data: backup } : { status: terminal, data: backup },
      onUpdate: update => { updates.push(update); if (updates.length === 2) done.resolve(); }, intervalMs: 1,
    }));
    await done.promise;
    assert.equal(updates[1].status, terminal === 'failed' ? 'error' : 'completed');
    assert.deepEqual(updates[1].data, backup);
    await delay(10);
    assert.equal(reads, 2);
  });
}

test('leaving a meeting ignores its late pending response and never cancels the native task', async () => {
  const response = deferred<NativeSummarySnapshot>();
  const updates: ResumedSummaryUpdate[] = [];
  let reads = 0;
  const stop = observeExistingSummaryTask({ read: () => { reads++; return response.promise; }, onUpdate: update => updates.push(update), intervalMs: 1 });
  stop();
  response.resolve({ status: 'pending', data: { markdown: 'meeting A' } });
  await delay(10);
  assert.deepEqual(updates, []);
  assert.equal(reads, 1);
});

test('cancelling the observer rejects an already in-flight terminal response', async () => {
  const terminal = deferred<NativeSummarySnapshot>();
  const secondRead = deferred<void>();
  const updates: ResumedSummaryUpdate[] = [];
  let reads = 0;
  const stop = observeExistingSummaryTask({
    read: async () => { if (++reads === 1) return { status: 'pending' }; secondRead.resolve(); return terminal.promise; },
    onUpdate: update => updates.push(update), intervalMs: 1,
  });
  await secondRead.promise;
  stop();
  terminal.resolve({ status: 'completed', data: { markdown: 'obsolete' } });
  await delay(10);
  assert.deepEqual(updates.map(update => update.status), ['processing']);
});

test('a newer local generation supersedes an older recovery read', async t => {
  const response = deferred<NativeSummarySnapshot>();
  const updates: ResumedSummaryUpdate[] = [];
  let superseded = false;
  t.after(observeExistingSummaryTask({ read: () => response.promise, onUpdate: update => updates.push(update), isSuperseded: () => superseded, intervalMs: 1 }));
  superseded = true;
  response.resolve({ status: 'pending', data: { markdown: 'obsolete' } });
  await delay(10);
  assert.deepEqual(updates, []);
});

test('an empty completed result and a read error end recovery without claiming success', async t => {
  for (const failure of ['empty', 'read']) {
    const done = deferred<ResumedSummaryUpdate>();
    let reads = 0;
    t.after(observeExistingSummaryTask({
      read: async () => { if (++reads === 1) return { status: 'pending' }; if (failure === 'read') throw Error('offline'); return { status: 'completed', data: null }; },
      onUpdate: update => { if (update.status === 'error') done.resolve(update); }, intervalMs: 1,
    }));
    const error = await done.promise;
    assert.equal(error.status, 'error');
    assert.equal(error.error, failure === 'empty' ? 'emptyContent' : 'generationFailed');
  }
});
