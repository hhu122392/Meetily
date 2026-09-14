import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import test from 'node:test';

const feedbackModuleUrl = new URL('../../src/lib/recording-input-feedback.ts', import.meta.url);
const monitorComponentUrl = new URL('../../src/components/RecordingInputMonitor.tsx', import.meta.url);

const route = (overrides: Record<string, unknown> = {}) => ({
  active: true,
  callback_count: 0,
  sample_count: 0,
  rms_level: 0,
  peak_level: 0,
  failed: false,
  no_signal: false,
  silent: false,
  ...overrides,
});

test('derives honest recording feedback from real route data', async () => {
  assert.equal(
    existsSync(feedbackModuleUrl),
    true,
    'recording input feedback helper must exist',
  );

  const { deriveRecordingInputFeedback } = await import(feedbackModuleUrl.href);

  assert.deepEqual(
    deriveRecordingInputFeedback(route(), 2, false, false),
    { state: 'waiting', levelPercent: 0, showSettings: false },
  );
  assert.deepEqual(
    deriveRecordingInputFeedback(route(), 6, false, false),
    { state: 'missing', levelPercent: 0, showSettings: true },
  );
  assert.deepEqual(
    deriveRecordingInputFeedback(
      route({ callback_count: 4, rms_level: 0.04, peak_level: 0.3 }),
      6,
      false,
      true,
    ),
    { state: 'receiving', levelPercent: 13, showSettings: false },
  );
  assert.deepEqual(
    deriveRecordingInputFeedback(route({ callback_count: 4 }), 6, false, false),
    { state: 'quiet', levelPercent: 0, showSettings: false },
  );
  assert.deepEqual(
    deriveRecordingInputFeedback(route({ failed: true }), 6, false, false),
    { state: 'problem', levelPercent: 0, showSettings: true },
  );
  assert.deepEqual(
    deriveRecordingInputFeedback(route(), 6, true, false),
    { state: 'paused', levelPercent: 0, showSettings: false },
  );
});

test('input level follows RMS, not isolated peaks or invalid readings', async () => {
  const { deriveRecordingInputFeedback } = await import(feedbackModuleUrl.href);
  const feedback = (overrides: Record<string, unknown>) => deriveRecordingInputFeedback(
    route({ callback_count: 4, ...overrides }), 6, false, true,
  );
  assert.equal(feedback({ rms_level: 0.04, peak_level: 1 }).levelPercent, 13);
  for (const rms of [0, -1, Number.NaN, Number.POSITIVE_INFINITY]) {
    assert.deepEqual(feedback({ rms_level: rms, peak_level: 1 }), {
      state: 'quiet', levelPercent: 0, showSettings: false,
    });
  }
  assert.equal(feedback({ rms_level: 2, peak_level: 0 }).levelPercent, 100);
  assert.equal(feedback({ rms_level: 0.5, silent: true }).levelPercent, 0);
  assert.equal(feedback({ rms_level: 0.5, no_signal: true }).levelPercent, 0);
});

test('main transcript view uses the real recording monitor instead of a fixed pulse', () => {
  assert.equal(
    existsSync(monitorComponentUrl),
    true,
    'recording input monitor component must exist',
  );

  const monitor = readFileSync(monitorComponentUrl, 'utf8');
  const transcriptView = readFileSync(
    new URL('../../src/components/VirtualizedTranscriptView.tsx', import.meta.url),
    'utf8',
  );
  const settingsPage = readFileSync(
    new URL('../../src/app/settings/page.tsx', import.meta.url),
    'utf8',
  );

  assert.match(monitor, /deriveRecordingInputFeedback\(\s*systemRoute/);
  assert.match(monitor, /systemDataAdvanced/);
  assert.match(monitor, /role="meter"/);
  assert.match(monitor, /\/settings\?tab=recording/);
  assert.match(transcriptView, /<RecordingInputMonitor/);
  assert.doesNotMatch(transcriptView, /bg-blue-500 animate-pulse/);
  assert.match(settingsPage, /searchParams\.get\('tab'\)/);
});
