import assert from 'node:assert/strict';
import test from 'node:test';
import {
  AUTO_START_RECORDING_TTL_MS,
  consumeAutoStartRecordingRequest,
  queueAutoStartRecordingRequest,
} from '../../src/lib/recording-auto-start';

class MemoryStorage {
  private readonly values = new Map<string, string>();

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }

  removeItem(key: string): void {
    this.values.delete(key);
  }
}

test('consumes one fresh request from the same renderer exactly once', () => {
  const storage = new MemoryStorage();
  queueAutoStartRecordingRequest(storage, 'app-a', 1_000, 'renderer-a');
  assert.equal(consumeAutoStartRecordingRequest(storage, 'app-a', 1_001, 'renderer-a'), true);
  assert.equal(consumeAutoStartRecordingRequest(storage, 'app-a', 1_002, 'renderer-a'), false);
});

test('rejects and clears a fresh request left by a previous renderer/app process', () => {
  const storage = new MemoryStorage();
  queueAutoStartRecordingRequest(storage, 'app-a', 1_000, 'renderer-before-restart');
  assert.equal(
    consumeAutoStartRecordingRequest(storage, 'app-b', 1_001, 'renderer-after-restart'),
    false,
  );
  assert.equal(
    consumeAutoStartRecordingRequest(storage, 'app-a', 1_002, 'renderer-before-restart'),
    false,
  );
});

test('rejects a restored renderer request when the native app process changed', () => {
  const storage = new MemoryStorage();
  queueAutoStartRecordingRequest(storage, 'app-before-restart', 1_000, 'renderer-restored');
  assert.equal(
    consumeAutoStartRecordingRequest(storage, 'app-after-restart', 1_001, 'renderer-restored'),
    false,
  );
});

test('rejects an expired request even within the same renderer', () => {
  const storage = new MemoryStorage();
  queueAutoStartRecordingRequest(storage, 'app-a', 1_000, 'renderer-a');
  assert.equal(
    consumeAutoStartRecordingRequest(
      storage,
      'app-a',
      1_000 + AUTO_START_RECORDING_TTL_MS + 1,
      'renderer-a',
    ),
    false,
  );
});

test('rejects and clears legacy literal and version-one values', () => {
  const storage = new MemoryStorage();
  storage.setItem('autoStartRecording', 'true');
  assert.equal(consumeAutoStartRecordingRequest(storage, 'app-a', 1_000, 'renderer-a'), false);
  storage.setItem('autoStartRecording', JSON.stringify({ version: 1, requestedAt: 999 }));
  assert.equal(consumeAutoStartRecordingRequest(storage, 'app-a', 1_000, 'renderer-a'), false);
  storage.setItem('autoStartRecording', JSON.stringify({
    version: 2,
    requestedAt: 999,
    rendererSessionId: 'renderer-a',
  }));
  assert.equal(consumeAutoStartRecordingRequest(storage, 'app-a', 1_000, 'renderer-a'), false);
});
