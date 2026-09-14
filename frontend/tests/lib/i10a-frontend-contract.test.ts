import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';

const read = (relativePath: string) => readFileSync(
  new URL(relativePath, import.meta.url),
  'utf8',
);

const deviceSelection = read('../../src/components/DeviceSelection.tsx');
const recordingService = read('../../src/services/recordingService.ts');
const recordingStart = read('../../src/hooks/useRecordingStart.ts');
const recordingState = read('../../src/contexts/RecordingStateContext.tsx');
const importDialogContext = read('../../src/contexts/ImportDialogContext.tsx');
const importDialog = read('../../src/components/ImportAudio/ImportAudioDialog.tsx');
const rootLayout = read('../../src/app/layout.tsx');
const homePage = read('../../src/app/page.tsx');
const recordingControls = read('../../src/components/RecordingControls.tsx');
const recordingStatusBar = read('../../src/components/RecordingStatusBar.tsx');
const recordingSettings = read('../../src/components/RecordingSettings.tsx');
const homePageRecordingLocaleEn = read('../../src/i18n/locales/en/recording.json');
const homePageRecordingLocaleZhCN = read('../../src/i18n/locales/zh-CN/recording.json');

const assertContains = (source: string, token: string, message: string) => {
  assert.ok(source.includes(token), message);
};

const assertMatches = (source: string, pattern: RegExp, message: string) => {
  assert.ok(pattern.test(source), message);
};

test('recording start sends one of the three explicit recording modes', () => {
  const startSurface = `${deviceSelection}\n${recordingService}\n${recordingStart}`;
  assertContains(startSurface, 'system_only', 'missing system_only mode');
  assertContains(startSurface, 'microphone_only', 'missing microphone_only mode');
  assertContains(startSurface, 'microphone_and_system', 'missing microphone_and_system mode');

  assertMatches(
    recordingService,
    /invoke<void>\('start_recording_with_devices_and_meeting',[\s\S]*?recordingMode,/,
    'recordingMode is not sent to Tauri',
  );
});

test('saved recording device settings update the active session immediately', () => {
  assertContains(
    recordingSettings,
    'setSelectedDevices',
    'saved recording settings are not copied into the active config context',
  );
  assertMatches(
    recordingSettings,
    /await invoke\('set_recording_preferences',[\s\S]*?setSelectedDevices\(\{[\s\S]*?recordingMode: prefs\.recording_mode/,
    'the active recording mode is not refreshed after the backend accepts the saved settings',
  );
});

test('selected devices keep stable endpoint IDs and send them to the backend', () => {
  const audioDevice = deviceSelection.match(/export interface AudioDevice \{[\s\S]*?\n\}/);
  const selectedDevices = deviceSelection.match(/export interface SelectedDevices \{[\s\S]*?\n\}/);
  assert.ok(audioDevice, 'AudioDevice contract was not found');
  assert.ok(selectedDevices, 'SelectedDevices contract was not found');
  assertMatches(audioDevice[0], /native_id\?:\s*string/, 'AudioDevice has no stable native endpoint ID');
  assertMatches(
    selectedDevices[0],
    /micDevice:\s*string\s*\|\s*null/,
    'selected microphone transport is missing',
  );
  assertMatches(
    selectedDevices[0],
    /systemDevice:\s*string\s*\|\s*null/,
    'selected system-audio transport is missing',
  );
  assertMatches(
    deviceSelection,
    /value=\{device\.native_id \?\?/,
    'device selectors do not prefer the stable native endpoint ID',
  );

  const invocation = recordingService.match(
    /invoke<void>\('start_recording_with_devices_and_meeting',[\s\S]*?\}\)/,
  );
  assert.ok(invocation, 'start-recording invocation was not found');
  assertMatches(invocation[0], /micDeviceName\s*:/, 'microphone endpoint transport is not sent to Tauri');
  assertMatches(invocation[0], /systemDeviceName\s*:/, 'system endpoint transport is not sent to Tauri');
});

test('recording state restores independent channel health and the durable error', () => {
  const stateSurface = `${recordingService}\n${recordingState}`;
  for (const field of [
    'recording_mode',
    'device_epoch',
    'microphone_route',
    'system_route',
    'rms_level',
    'peak_level',
    'last_error',
  ]) {
    assertMatches(stateSurface, new RegExp(`\\b${field}\\b`), `missing persisted field ${field}`);
  }

  for (const status of [
    'no_signal',
    'silent',
    'failed',
    'is_reconnecting',
  ]) {
    assertMatches(stateSurface, new RegExp(`\\b${status}\\b`), `missing channel status ${status}`);
  }

  assertMatches(
    recordingState,
    /syncWithBackend[\s\S]*?last_error/,
    'refresh sync must restore the durable backend error instead of inventing progress',
  );
});

test('recording UI displays measured channel levels instead of invented activity', () => {
  const recordingUi = `${homePage}\n${recordingControls}\n${recordingStatusBar}`;
  assert.ok(!recordingUi.includes('Math.random()'), 'recording UI still fabricates audio activity');
  assertContains(recordingUi, 'microphoneRoute.rms_level', 'microphone RMS is not displayed');
  assertContains(recordingUi, 'systemRoute.rms_level', 'system-audio RMS is not displayed');
});

test('idle system audio is shown as waiting and real failures open device settings', () => {
  assertContains(
    recordingStatusBar,
    'systemRoute.callback_count === 0',
    'idle system audio is not distinguished from a failed route',
  );
  assertContains(
    recordingStatusBar,
    "t('status.waitingForSystemAudio')",
    'idle system audio has no localized waiting state',
  );
  assertContains(
    recordingControls,
    'onOpenDeviceSettings',
    'system-audio failures cannot guide the user into device settings',
  );
  assertContains(
    homePage,
    "showModal('deviceSettings')",
    'the recording error action is not connected to the existing device settings',
  );
  assert.ok(
    !homePageRecordingLocaleEn.includes('BlackHole')
      && !homePageRecordingLocaleZhCN.includes('BlackHole'),
    'recording errors still tell Windows users to install the macOS-only BlackHole device',
  );
});

test('every frontend import entry point is disabled while recording', () => {
  assertContains(importDialogContext, 'useRecordingState', 'sidebar import has no recording-state gate');
  assertMatches(importDialogContext, /if\s*\(isRecording\)/, 'sidebar import does not reject recording');

  assertContains(importDialog, 'useRecordingState', 'open import dialog has no recording-state gate');
  assertMatches(
    importDialog,
    /handleStartImport[\s\S]*?if\s*\(isRecording\)/,
    'import submit handler does not reject recording',
  );
  assertMatches(
    importDialog,
    /disabled=\{[^}]*isRecording[^}]*\}/,
    'import submit control is not disabled while recording',
  );

  const dropHandlerStart = rootLayout.indexOf('const handleFileDrop');
  const dropHandlerEnd = rootLayout.indexOf('// Listen for drag-drop events', dropHandlerStart);
  assert.ok(dropHandlerStart >= 0 && dropHandlerEnd > dropHandlerStart, 'drag-drop handler was not found');
  const dropHandler = rootLayout.slice(dropHandlerStart, dropHandlerEnd);
  assertMatches(
    dropHandler,
    /handleFileDrop[\s\S]*?invoke<boolean>\('is_recording'\)/,
    'drag-drop import does not query the recording state',
  );
  const dropGuard = dropHandler.indexOf("invoke<boolean>('is_recording')");
  const dropOpen = dropHandler.indexOf('setShowImportDialog(true)', dropGuard);
  assert.ok(dropGuard >= 0 && dropOpen > dropGuard, 'drop import must check recording before opening');
});
