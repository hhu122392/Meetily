import { afterAll, afterEach, expect, mock, test } from 'bun:test';
import { act, create, type ReactTestRenderer } from 'react-test-renderer';
const originalEvent = { ...await import('@tauri-apps/api/event') };
const originalService = { ...await import('../../src/services/recordingService') };
const handlers = new Map<string, (event: any) => void>();
let backend = { is_recording: true, is_stopping: false, is_paused: false, is_active: true, recording_duration: 3, active_duration: 3 };
const listen = mock(async (name: string, callback: (event: any) => void) => {
  handlers.set(name, callback); return () => { handlers.delete(name); };
});
mock.module('@tauri-apps/api/event', () => ({ ...originalEvent, listen }));
const register = async () => () => {};
mock.module('../../src/services/recordingService', () => ({ ...originalService, recordingService: {
  getRecordingState: async () => backend,
  onRecordingStarted: async (fn: (event: any) => void) => { handlers.set('started', fn); return () => handlers.delete('started'); },
  onRecordingStarting: register,
  onRecordingStopped: async (fn: (event: any) => void) => { handlers.set('stopped', fn); return () => handlers.delete('stopped'); },
  onRecordingPaused: register, onRecordingResumed: register,
  onMicDeviceSwitched: register, onMicSwapFailed: register, onMicUnavailable: register, onMicRecoveryExhausted: register,
} }));
const { RecordingStateProvider, useRecordingState, RecordingStatus } = await import('../../src/contexts/RecordingStateContext');
let state: ReturnType<typeof useRecordingState>;
function Probe() { state = useRecordingState(); return null; }
let renderer: ReactTestRenderer | undefined;
afterEach(async () => { if (renderer) await act(async () => renderer!.unmount()); renderer=undefined; handlers.clear(); });
afterAll(() => {
  mock.module('@tauri-apps/api/event', () => originalEvent);
  mock.module('../../src/services/recordingService', () => originalService);
});
test('audio stop progress distinguishes capture from processing while the backend remains busy', async () => {
  backend = { ...backend, is_stopping: false };
  await act(async () => { renderer = create(<RecordingStateProvider><Probe /></RecordingStateProvider>); });
  expect(state.isRecording).toBe(true);
  await act(async () => handlers.get('recording-shutdown-progress')!({ payload: { stage:'processing_transcripts', elapsed_seconds:42 } }));
  expect(state.isRecording).toBe(false);
  expect(state.isActive).toBe(false);
  expect(state.isProcessing).toBe(true);
  expect(state.statusMessage).toContain('录音已停止');
  expect(state.statusMessage).toContain('42');
});
test('refresh during shutdown restores processing rather than a recording indicator', async () => {
  backend = { ...backend, is_stopping: true };
  await act(async () => { renderer = create(<RecordingStateProvider><Probe /></RecordingStateProvider>); });
  expect(state.isRecording).toBe(false);
  expect(state.isProcessing).toBe(true);
  expect(state.statusMessage).toContain('录音已停止');
});

test('saved background job releases recording UI and a later recording remains independent', async () => {
  backend = { ...backend, is_stopping: false };
  await act(async () => { renderer = create(<RecordingStateProvider><Probe /></RecordingStateProvider>); });
  await act(async () => handlers.get('recording-shutdown-progress')!({ payload: { stage: 'saving_audio' } }));
  expect(state.isSaving).toBe(true);
  await act(async () => handlers.get('stopped')!({ background_queued: true, meeting_id: 'first' }));
  expect(state.status).toBe(RecordingStatus.IDLE);
  expect(state.isProcessing).toBe(false);
  await act(async () => handlers.get('started')!({ background_queue: true }));
  expect(state.isRecording).toBe(true);
  expect(state.status).toBe(RecordingStatus.RECORDING);
});
