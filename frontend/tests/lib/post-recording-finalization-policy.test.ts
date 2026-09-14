import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const recordingStopHook = readFileSync(
  new URL('../../src/hooks/useRecordingStop.ts', import.meta.url),
  'utf8',
);
const retranscribeDialog = readFileSync(
  new URL('../../src/components/MeetingDetails/RetranscribeDialog.tsx', import.meta.url),
  'utf8',
);

test('recording stop does not automatically run a second full-file transcription', () => {
  assert.doesNotMatch(recordingStopHook, /finalize_recording_transcript_command/);
  assert.doesNotMatch(recordingStopHook, /recording-finalizing/);
  assert.match(recordingStopHook, /const navigationSource = 'recording';/);
  assert.match(
    recordingStopHook,
    /full-file enhancement remains user initiated/,
  );
});

test('full-file enhancement remains available as an explicit user action', () => {
  assert.match(retranscribeDialog, /invoke\('start_retranscription_command'/);
  assert.match(retranscribeDialog, /cancel_retranscription_command/);
  assert.match(retranscribeDialog, /retranscription-progress/);
});
