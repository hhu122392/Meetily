import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const service = readFileSync(
  new URL('../../src/services/recordingService.ts', import.meta.url),
  'utf8',
);

test('recording device selection and meeting name use Tauri camelCase argument keys', () => {
  const invocation = service.match(
    /invoke<void>\('start_recording_with_devices_and_meeting',[\s\S]*?\}\)/,
  );

  assert.ok(invocation, 'start-recording invocation was not found');
  assert.match(invocation[0], /micDeviceName:\s*micDeviceName/);
  assert.match(invocation[0], /systemDeviceName:\s*systemDeviceName/);
  assert.match(invocation[0], /meetingName:\s*meetingName/);
  assert.match(invocation[0], /templateSelection:\s*metadata\.templateSelection/);
  assert.match(invocation[0], /meetingContextDraft:\s*metadata\.meetingContextDraft/);
  assert.doesNotMatch(invocation[0], /mic_device_name\s*:/);
  assert.doesNotMatch(invocation[0], /system_device_name\s*:/);
  assert.doesNotMatch(invocation[0], /meeting_name\s*:/);
});

test('all recording start entry points share one prepared backend invocation', () => {
  const hook = readFileSync(
    new URL('../../src/hooks/useRecordingStart.ts', import.meta.url),
    'utf8',
  );
  assert.equal((hook.match(/startRecordingWithDevices\(/g) ?? []).length, 1);
  assert.match(hook, /prepareAndStart\('home_page'\)/);
  assert.match(hook, /prepareAndStart\('sidebar_auto'\)/);
  assert.match(hook, /prepareAndStart\('sidebar_direct'\)/);
  assert.match(hook, /prepareRecordingMetadata\(\)/);
});
